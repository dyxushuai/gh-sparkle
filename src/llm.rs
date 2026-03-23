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

const GITHUB_MODELS_API_VERSION: &str = "2026-03-10";
const GITHUB_MODELS_ENDPOINT: &str = "https://models.github.ai/inference/chat/completions";
const MAX_ERROR_BODY_BYTES: u64 = 8 * 1024;
const MAX_RETRY_ATTEMPTS: usize = 4;
const BASE_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(4);
const USER_AGENT: &str = concat!("gh-sparkle/", env!("CARGO_PKG_VERSION"));

#[derive(Clone, Serialize)]
struct Request {
    messages: Vec<Message>,
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f64>,
    stream: bool,
}

impl Request {
    fn from_prompt_config(
        messages: Vec<Message>,
        model: &str,
        prompt_config: &PromptConfig,
    ) -> Self {
        Self {
            messages,
            model: model.to_string(),
            temperature: Some(prompt_config.model_parameters.temperature),
            top_p: Some(prompt_config.model_parameters.top_p),
            stream: false,
        }
    }

    fn without_sampling_parameters(mut self) -> Self {
        self.temperature = None;
        self.top_p = None;
        self
    }

    fn has_sampling_parameters(&self) -> bool {
        self.temperature.is_some() || self.top_p.is_some()
    }
}

#[derive(Clone, Serialize)]
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

        let mut request = Request::from_prompt_config(messages, model, prompt_config);

        let response = match self.call_github_models(&request) {
            Ok(response) => response,
            Err(err)
                if request.has_sampling_parameters() && err.is_sampling_parameter_unsupported() =>
            {
                request = request.without_sampling_parameters();
                self.call_github_models(&request)?
            }
            Err(err) => return Err(Box::new(err)),
        };

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

    fn call_github_models(&self, request: &Request) -> Result<Response, ModelsRequestError> {
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
            status: None,
            response_body: None,
            retry_after: None,
            retryable: false,
        });

        if attempt > 1 {
            final_error.message = format!(
                "GitHub Models request failed after {attempt} attempts: {}",
                final_error.message
            );
        }

        Err(final_error)
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
                status: None,
                response_body: None,
                retry_after: None,
                retryable: err.is_timeout() || err.is_connect(),
            })?;

        let status = response.status();
        if status.is_success() {
            return response
                .json::<Response>()
                .map_err(|err| ModelsRequestError {
                    message: format!("failed to decode response: {err}"),
                    status: Some(status),
                    response_body: None,
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
            status: Some(status),
            response_body: Some(body),
            retry_after,
            retryable,
        })
    }
}

#[derive(Debug)]
struct ModelsRequestError {
    message: String,
    status: Option<StatusCode>,
    response_body: Option<String>,
    retry_after: Option<Duration>,
    retryable: bool,
}

impl ModelsRequestError {
    fn is_sampling_parameter_unsupported(&self) -> bool {
        if self.status != Some(StatusCode::BAD_REQUEST) {
            return false;
        }

        let Some(body) = &self.response_body else {
            return false;
        };
        let Ok(parsed) = serde_json::from_str::<ErrorEnvelope>(body) else {
            return false;
        };
        let Some(param) = parsed.error.param.as_deref() else {
            return false;
        };

        if !matches!(param, "temperature" | "top_p" | "topP") {
            return false;
        }

        parsed.error.code.as_deref() == Some("unsupported_value")
            || parsed
                .error
                .message
                .to_lowercase()
                .contains("only the default")
    }
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: ErrorDetail,
}

#[derive(Deserialize)]
struct ErrorDetail {
    message: String,
    param: Option<String>,
    code: Option<String>,
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
    let mut example_index = 0usize;
    for line in examples.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        example_index += 1;
        lines.push(format!("Example {example_index}: {trimmed}"));
    }

    format!(
        "Here are some examples of good commit messages used previously in this project:\n{}",
        lines.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_examples_string_numbers_non_empty_lines() {
        let input = "feat: add foo\n\nfix: handle bar\n";
        let rendered = create_examples_string(input);
        assert!(rendered.contains("Example 1: feat: add foo"));
        assert!(rendered.contains("Example 2: fix: handle bar"));
    }

    #[test]
    fn create_examples_string_is_not_a_markdown_list() {
        let rendered = create_examples_string("feat: add foo\nfix: handle bar\n");
        assert!(!rendered.contains("\n- "));
    }

    #[test]
    fn request_omits_sampling_parameters_when_none() {
        let request = Request {
            messages: vec![Message {
                role: "user".to_string(),
                content: "hello".to_string(),
            }],
            model: "openai/gpt-5-mini".to_string(),
            temperature: None,
            top_p: None,
            stream: false,
        };

        let serialized = serde_json::to_string(&request).unwrap();
        assert!(!serialized.contains("temperature"));
        assert!(!serialized.contains("top_p"));
    }

    #[test]
    fn models_request_error_detects_unsupported_sampling_parameter() {
        let unsupported = ModelsRequestError {
            message: "bad request".to_string(),
            status: Some(StatusCode::BAD_REQUEST),
            response_body: Some(
                "{\"error\":{\"message\":\"Unsupported value: 'temperature' does not support 0.2 with this model. Only the default (1) value is supported.\",\"param\":\"temperature\",\"code\":\"unsupported_value\"}}"
                    .to_string(),
            ),
            retry_after: None,
            retryable: false,
        };
        let unauthorized = ModelsRequestError {
            message: "unauthorized".to_string(),
            status: Some(StatusCode::UNAUTHORIZED),
            response_body: None,
            retry_after: None,
            retryable: false,
        };

        assert!(unsupported.is_sampling_parameter_unsupported());
        assert!(!unauthorized.is_sampling_parameter_unsupported());
    }
}
