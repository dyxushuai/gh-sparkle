// Prompt configuration loading and validation.

use serde::Deserialize;
use std::error::Error;

const COMMITMSG_PROMPT_YAML: &str = include_str!("../assets/commitmsg.prompt.yml");

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptConfig {
    #[serde(default)]
    pub model_parameters: ModelParameters,
    #[serde(default)]
    pub model_policy: ModelPolicy,
    pub context_policy: ContextPolicy,
    #[serde(default)]
    pub messages: Vec<PromptMessage>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelParameters {
    pub temperature: f64,
    pub top_p: f64,
}

impl Default for ModelParameters {
    fn default() -> Self {
        Self {
            temperature: 0.2,
            top_p: 0.9,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ModelPolicy {
    #[serde(default)]
    pub auto_models: Vec<String>,
}

#[derive(Deserialize)]
pub struct PromptMessage {
    pub role: String,
    pub content: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextPolicy {
    pub token_char_ratio: usize,
    pub budgets: ContextBudgets,
    pub sections: Vec<ContextSection>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_field_names)]
pub struct ContextBudgets {
    pub primary_tokens: usize,
    pub fallback_tokens: usize,
    pub minimal_tokens: usize,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSection {
    pub source: ContextSource,
    pub header: String,
    pub max_ratio: f64,
    #[serde(default)]
    pub required: bool,
}

#[derive(Deserialize, Copy, Clone)]
#[serde(rename_all = "lowercase")]
pub enum ContextSource {
    Summary,
    Diff,
}

pub fn load_prompt_config() -> Result<PromptConfig, Box<dyn Error>> {
    Ok(serde_yaml::from_str(COMMITMSG_PROMPT_YAML)?)
}

pub fn validate_context_policy(policy: &ContextPolicy) -> Result<(), Box<dyn Error>> {
    if policy.token_char_ratio == 0 {
        return Err("contextPolicy.tokenCharRatio must be greater than 0".into());
    }
    if policy.budgets.primary_tokens == 0
        || policy.budgets.fallback_tokens == 0
        || policy.budgets.minimal_tokens == 0
    {
        return Err("contextPolicy.budgets must be greater than 0".into());
    }
    if policy.sections.is_empty() {
        return Err("contextPolicy.sections must not be empty".into());
    }

    let mut ratio_sum = 0.0_f64;
    let mut required_count = 0usize;
    for section in &policy.sections {
        if section.header.is_empty() {
            return Err("contextPolicy.sections.header must not be empty".into());
        }
        if !(0.0 < section.max_ratio && section.max_ratio <= 1.0) {
            return Err("contextPolicy.sections.maxRatio must be within (0, 1]".into());
        }
        ratio_sum += section.max_ratio;
        if section.required {
            required_count += 1;
        }
    }

    if ratio_sum > 1.0 + f64::EPSILON {
        return Err("contextPolicy.sections.maxRatio sum must be <= 1.0".into());
    }
    if required_count == 0 {
        return Err("contextPolicy must include at least one required section".into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_policy() -> ContextPolicy {
        ContextPolicy {
            token_char_ratio: 4,
            budgets: ContextBudgets {
                primary_tokens: 4000,
                fallback_tokens: 2000,
                minimal_tokens: 1000,
            },
            sections: vec![
                ContextSection {
                    source: ContextSource::Summary,
                    header: "Summary:\n".to_string(),
                    max_ratio: 0.7,
                    required: true,
                },
                ContextSection {
                    source: ContextSource::Diff,
                    header: "\nDiff:\n".to_string(),
                    max_ratio: 0.3,
                    required: false,
                },
            ],
        }
    }

    #[test]
    fn validate_context_policy_accepts_valid_policy() {
        let policy = valid_policy();
        assert!(validate_context_policy(&policy).is_ok());
    }

    #[test]
    fn validate_context_policy_requires_required_section() {
        let mut policy = valid_policy();
        for section in &mut policy.sections {
            section.required = false;
        }
        assert!(validate_context_policy(&policy).is_err());
    }

    #[test]
    fn validate_context_policy_rejects_invalid_ratio_sum() {
        let mut policy = valid_policy();
        policy.sections[0].max_ratio = 0.8;
        policy.sections[1].max_ratio = 0.4;
        assert!(validate_context_policy(&policy).is_err());
    }
}
