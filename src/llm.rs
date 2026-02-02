// LLM client using GitHub Models API.

use reqwest::StatusCode;
use reqwest::blocking::Client as HttpClient;
use serde::{Deserialize, Serialize};
use std::env;
use std::error::Error;
use std::io::Read;
use std::process::Command;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::prompt::PromptConfig;

const GITHUB_MODELS_API_VERSION: &str = "2022-11-28";
const GITHUB_MODELS_ENDPOINT: &str = "https://models.github.ai/inference/chat/completions";
const MAX_ERROR_BODY_BYTES: u64 = 8 * 1024;
const MAX_RETRY_ATTEMPTS: usize = 4;
const BASE_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(4);
const USER_AGENT: &str = concat!("gh-sparkle/", env!("CARGO_PKG_VERSION"));

#[derive(Serialize)]
struct Request {
    messages: Vec<Message>,
    model: String,
    temperature: f64,
    top_p: f64,
    stream: bool,
}

#[derive(Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct Response {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(Deserialize)]
struct ChoiceMessage {
    content: String,
}

/// LLM client for generating commit messages.
pub struct Client {
    token: String,
    http: HttpClient,
}

impl Client {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        let host = resolve_host();
        let token = resolve_token(&host)?;

        let http = HttpClient::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()?;

        Ok(Self { token, http })
    }

    pub fn generate_commit_message(
        &self,
        prompt_config: &PromptConfig,
        changes_summary: &str,
        model: &str,
        language: &str,
        examples: &str,
    ) -> Result<String, Box<dyn Error>> {
        let messages = build_messages(prompt_config, changes_summary, language, examples);

        let request = Request {
            messages,
            model: model.to_string(),
            temperature: prompt_config.model_parameters.temperature,
            top_p: prompt_config.model_parameters.top_p,
            stream: false,
        };

        let response = self.call_github_models(&request)?;

        let content = response
            .choices
            .first()
            .ok_or("no response generated from the model")?
            .message
            .content
            .trim()
            .to_string();

        Ok(content)
    }

    fn call_github_models(&self, request: &Request) -> Result<Response, Box<dyn Error>> {
        let mut attempt = 0usize;
        let mut backoff = BASE_BACKOFF;
        let mut last_error: Option<ModelsRequestError> = None;

        while attempt < MAX_RETRY_ATTEMPTS {
            attempt += 1;
            match self.call_github_models_once(request) {
                Ok(response) => return Ok(response),
                Err(err) => {
                    let retryable = err.retryable;
                    let retry_after = err.retry_after;
                    last_error = Some(err);

                    if retryable && attempt < MAX_RETRY_ATTEMPTS {
                        let delay = retry_after.map_or_else(
                            || add_jitter(backoff),
                            |delay| delay.min(Duration::from_secs(30)),
                        );
                        thread::sleep(delay);
                        backoff = (backoff + backoff).min(MAX_BACKOFF);
                        continue;
                    }

                    break;
                }
            }
        }

        let mut final_error = last_error.unwrap_or_else(|| ModelsRequestError {
            message: "GitHub Models request failed".to_string(),
            retry_after: None,
            retryable: false,
        });

        if attempt > 1 {
            final_error.message = format!(
                "GitHub Models request failed after {attempt} attempts: {}",
                final_error.message
            );
        }

        Err(Box::new(final_error))
    }

    fn call_github_models_once(&self, request: &Request) -> Result<Response, ModelsRequestError> {
        let mut response = self
            .http
            .post(GITHUB_MODELS_ENDPOINT)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", GITHUB_MODELS_API_VERSION)
            .header("Content-Type", "application/json")
            .bearer_auth(&self.token)
            .json(request)
            .send()
            .map_err(|err| ModelsRequestError {
                message: format!("network error: {err}"),
                retry_after: None,
                retryable: err.is_timeout() || err.is_connect(),
            })?;

        let status = response.status();
        if status.is_success() {
            return response
                .json::<Response>()
                .map_err(|err| ModelsRequestError {
                    message: format!("failed to decode response: {err}"),
                    retry_after: None,
                    retryable: false,
                });
        }

        let retry_after = if status == StatusCode::TOO_MANY_REQUESTS {
            parse_retry_after(response.headers())
        } else {
            None
        };

        let body = read_body_limited(&mut response);
        let message = format_models_error(status, &body);
        let retryable = status == StatusCode::TOO_MANY_REQUESTS
            || status == StatusCode::REQUEST_TIMEOUT
            || status.is_server_error();

        Err(ModelsRequestError {
            message,
            retry_after,
            retryable,
        })
    }
}

#[derive(Debug)]
struct ModelsRequestError {
    message: String,
    retry_after: Option<Duration>,
    retryable: bool,
}

impl std::fmt::Display for ModelsRequestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for ModelsRequestError {}

fn read_body_limited(response: &mut reqwest::blocking::Response) -> String {
    let mut body = String::new();
    let _ = response
        .take(MAX_ERROR_BODY_BYTES)
        .read_to_string(&mut body);
    body
}

fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    let raw = headers.get("retry-after")?.to_str().ok()?.trim();
    let seconds: u64 = raw.parse().ok()?;
    Some(Duration::from_secs(seconds))
}

fn format_models_error(status: StatusCode, body: &str) -> String {
    let body_trimmed = body.trim();
    match status {
        StatusCode::UNAUTHORIZED => {
            let mut message = format!(
                "GitHub Models authentication failed (HTTP {status}). \
Ensure your token can access GitHub Models. \
It must grant `models: read` (fine-grained PAT) or `models` scope (classic PAT)."
            );

            if !body_trimmed.is_empty() {
                message.push_str(" Response body: ");
                message.push_str(body_trimmed);
            }

            message
        }
        StatusCode::FORBIDDEN => {
            let mut message = format!(
                "GitHub Models access forbidden (HTTP {status}). \
Your token may be missing the required permission, or GitHub Models may not be enabled. \
It must grant `models: read` (fine-grained PAT) or `models` scope (classic PAT)."
            );

            if !body_trimmed.is_empty() {
                message.push_str(" Response body: ");
                message.push_str(body_trimmed);
            }

            message
        }
        _ => {
            if body_trimmed.is_empty() {
                format!("GitHub Models API request failed with HTTP {status}.")
            } else {
                format!("GitHub Models API request failed with HTTP {status}: {body_trimmed}")
            }
        }
    }
}

fn add_jitter(delay: Duration) -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .subsec_nanos();
    let jitter = Duration::from_millis(u64::from(nanos % 100));
    (delay + jitter).min(Duration::from_secs(30))
}

fn resolve_host() -> String {
    env::var("GH_HOST")
        .or_else(|_| env::var("GITHUB_HOST"))
        .unwrap_or_else(|_| "github.com".to_string())
}

fn resolve_token(host: &str) -> Result<String, Box<dyn Error>> {
    for key in ["GH_TOKEN", "GITHUB_TOKEN", "GITHUB_OAUTH_TOKEN"] {
        if let Ok(token) = env::var(key) {
            let trimmed = token.trim();
            if !trimmed.is_empty() {
                return Ok(trimmed.to_string());
            }
        }
    }

    let output = Command::new("gh")
        .args(["auth", "token", "--hostname", host])
        .output()?;

    if !output.status.success() {
        return Err("no GitHub token found, please run 'gh auth login' to authenticate".into());
    }

    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if token.is_empty() {
        return Err("no GitHub token found, please run 'gh auth login' to authenticate".into());
    }

    Ok(token)
}

fn build_messages(
    prompt_config: &PromptConfig,
    changes_summary: &str,
    language: &str,
    examples: &str,
) -> Vec<Message> {
    let mut messages = Vec::with_capacity(prompt_config.messages.len());

    for msg in &prompt_config.messages {
        let mut content = msg.content.replace("{{changes}}", changes_summary);
        content = content.replace("{{language}}", language);

        if !examples.is_empty() && content.contains("{{examples}}") {
            content = content.replace("{{examples}}", &create_examples_string(examples));
        } else {
            content = content.replace("{{examples}}", "");
        }

        messages.push(Message {
            role: msg.role.clone(),
            content,
        });
    }

    messages
}

fn create_examples_string(examples: &str) -> String {
    if examples.is_empty() {
        return String::new();
    }

    let mut lines = Vec::new();
    for line in examples.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        lines.push(format!("- {trimmed}"));
    }

    format!(
        "Here are some examples of good commit messages used previously in project:\n{}",
        lines.join("\n")
    )
}
