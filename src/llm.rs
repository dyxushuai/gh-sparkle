// LLM client using GitHub Models API.

use reqwest::blocking::Client as HttpClient;
use serde::{Deserialize, Serialize};
use std::env;
use std::error::Error;
use std::process::Command;
use std::time::Duration;

const COMMITMSG_PROMPT_YAML: &str = include_str!("../assets/commitmsg.prompt.yml");

#[derive(Default, Deserialize)]
struct PromptConfig {
    #[serde(default)]
    model_parameters: ModelParameters,
    #[serde(default)]
    messages: Vec<PromptMessage>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelParameters {
    temperature: f64,
    top_p: f64,
}

#[derive(Deserialize)]
struct PromptMessage {
    role: String,
    content: String,
}

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

pub struct Client {
    token: String,
    http: HttpClient,
}

impl Client {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        print!("  Checking GitHub token... ");

        let host = resolve_host();
        let token = resolve_token(&host)?;

        println!("Done");

        let http = HttpClient::builder()
            .timeout(Duration::from_secs(30))
            .build()?;

        Ok(Self { token, http })
    }

    pub fn generate_commit_message(
        &self,
        changes_summary: &str,
        model: &str,
        language: &str,
        examples: &str,
    ) -> Result<String, Box<dyn Error>> {
        print!("  Loading prompt configuration... ");
        let prompt_config = load_prompt_config()?;
        println!("Done");

        let messages = build_messages(&prompt_config, changes_summary, language, examples);

        let request = Request {
            messages,
            model: model.to_string(),
            temperature: prompt_config.model_parameters.temperature,
            top_p: prompt_config.model_parameters.top_p,
            stream: false,
        };

        println!("  Calling GitHub Models API ({})...", model);
        let response = self.call_github_models(&request)?;

        let content = response
            .choices
            .get(0)
            .ok_or("no response generated from the model")?
            .message
            .content
            .trim()
            .to_string();

        Ok(content)
    }

    fn call_github_models(&self, request: &Request) -> Result<Response, Box<dyn Error>> {
        let response = self
            .http
            .post("https://models.github.ai/inference/chat/completions")
            .header("Content-Type", "application/json")
            .bearer_auth(&self.token)
            .json(request)
            .send()?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().unwrap_or_default();
            return Err(format!("API request failed with status {}: {}", status, body).into());
        }

        Ok(response.json::<Response>()?)
    }
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

fn load_prompt_config() -> Result<PromptConfig, Box<dyn Error>> {
    Ok(serde_yaml::from_str(COMMITMSG_PROMPT_YAML)?)
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

    format!(
        "Here are some examples of good commit messages used previously in project:\n{}",
        examples
    )
}
