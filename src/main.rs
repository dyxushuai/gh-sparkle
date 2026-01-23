// CLI entrypoint for gh-sparkle.

mod git;
mod llm;
mod prompt;
mod ui;

use clap::Parser;
use crossterm::style::Stylize;
use std::error::Error;
use std::time::{Duration, Instant};

const EXTENSION_NAME: &str = "sparkle";
const DEFAULT_MODEL: &str = "auto";
const MAX_EXAMPLES: usize = 20;

#[derive(Parser)]
#[command(
    name = EXTENSION_NAME,
    about = "Generate AI-powered commit messages",
    long_about = "A GitHub CLI extension that generates commit messages using GitHub Models and staged git changes"
)]
struct Cli {
    /// Language to generate commit message in
    #[arg(short = 'l', long = "language", default_value = "english")]
    language: String,

    /// Add N examples of commit messages to context (default 3 if flag is set without value, max 20)
    #[arg(short = 'e', long = "examples", num_args = 0..=1, default_missing_value = "3")]
    examples: Option<String>,

    /// GitHub Models model to use
    #[arg(short = 'm', long = "model", default_value = DEFAULT_MODEL)]
    model: String,
}

fn main() {
    if let Err(err) = run() {
        eprintln!("Error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    if ui::Ui::is_tty() {
        return run_with_tui();
    }

    run_plain()
}

fn run_plain() -> Result<(), Box<dyn Error>> {
    let mut profile = Profile::new();
    let cli = Cli::parse();

    profile.mark("parse args");
    let staged_changes = git::get_staged_changes()?;
    if staged_changes.trim().is_empty() {
        println!("No staged changes in the repository.");
        return Ok(());
    }

    let staged_summary = git::get_staged_summary()?;

    print!("  Loading prompt configuration... ");
    let prompt_config = prompt::load_prompt_config()?;
    prompt::validate_context_policy(&prompt_config.context_policy)?;
    println!("Done");
    profile.mark("load prompt config");

    let examples_count = parse_examples_count(cli.examples)?;

    let mut latest_commit_messages = String::new();
    if examples_count > 0 {
        latest_commit_messages = git::get_commit_messages(examples_count)?;
        println!(
            "  Adding {} example(s) of previous commit messages to context",
            examples_count
        );
    }

    print!("  Checking GitHub token... ");
    let llm_client = llm::Client::new()?;
    println!("Done");
    profile.mark("init client");

    println!("  Language for commit message: {}", cli.language);

    let model_chain = resolve_model_chain(&cli.model, &prompt_config.model_policy)?;
    if cli.model == "auto" {
        println!("  Model selection: auto -> {}", model_chain.join(", "));
    } else {
        println!("  Model selection: {}", model_chain.join(", "));
    }

    let context = GenerationContext {
        prompt_config: &prompt_config,
        policy: &prompt_config.context_policy,
        staged_summary: &staged_summary,
        staged_changes: &staged_changes,
        model_chain: &model_chain,
        language: &cli.language,
        examples: &latest_commit_messages,
    };
    let commit_msg = generate_with_fallbacks(&llm_client, &context, |message| {
        println!("  {message}");
    })?;
    profile.mark("generate message");

    let mut commit_msg = sanitize_commit_message(&commit_msg);
    if commit_msg.is_empty() {
        return Err("generated commit message is empty".into());
    }

    if !commit_msg.ends_with('\n') {
        commit_msg.push('\n');
    }

    print_commit_message(&commit_msg);

    println!("  Committing staged changes...");
    git::commit_with_message(&commit_msg, false)?;
    profile.mark("commit");

    profile.print_if_enabled();
    Ok(())
}

fn run_with_tui() -> Result<(), Box<dyn Error>> {
    use std::sync::mpsc;
    use std::thread;

    let cli = Cli::parse();

    let mut ui = ui::Ui::start(vec![
        "Check GitHub auth",
        "Load prompt config",
        "Collect staged changes",
        "Select model",
        "Generate commit message",
        "Commit staged changes",
    ])?;

    let (tx, rx) = mpsc::channel::<UiEvent>();
    let worker = thread::spawn(move || {
        let result = run_pipeline(cli, tx.clone());
        match result {
            Ok((commit_msg, profile)) => {
                let _ = tx.send(UiEvent::Completed(commit_msg, profile));
            }
            Err(err) => {
                let _ = tx.send(UiEvent::Failed(err.to_string()));
            }
        }
    });

    let mut finished: Option<Result<(Option<String>, Profile), String>> = None;
    while finished.is_none() {
        while let Ok(event) = rx.try_recv() {
            match event {
                UiEvent::Step { index, status } => ui.set_step_status(index, status),
                UiEvent::Log(message) => ui.log(message),
                UiEvent::Completed(commit_msg, profile) => {
                    finished = Some(Ok((commit_msg, profile)))
                }
                UiEvent::Failed(message) => {
                    ui.set_error();
                    ui.log(message.clone());
                    finished = Some(Err(message));
                }
            }
        }
        ui.tick();
        ui.draw()?;
        thread::sleep(Duration::from_millis(40));
    }

    ui.shutdown()?;
    let _ = worker.join();

    match finished.unwrap_or_else(|| Err("unknown error".to_string())) {
        Ok((Some(commit_msg), profile)) => {
            print_commit_message(&commit_msg);
            println!("  Committed staged changes.");
            profile.print_if_enabled();
            Ok(())
        }
        Ok((None, profile)) => {
            println!("No staged changes in the repository.");
            profile.print_if_enabled();
            Ok(())
        }
        Err(message) => Err(message.into()),
    }
}

fn parse_examples_count(raw: Option<String>) -> Result<usize, Box<dyn Error>> {
    let Some(raw_value) = raw else {
        return Ok(0);
    };

    let count: usize = raw_value
        .parse()
        .map_err(|_| format!("invalid examples count: {raw_value}"))?;

    if count == 0 || count > MAX_EXAMPLES {
        return Err(format!("examples count must be between 1 and {MAX_EXAMPLES}").into());
    }

    Ok(count)
}

fn print_commit_message(commit_msg: &str) {
    let message = commit_msg.trim_end();
    if ui::Ui::is_tty() {
        println!("💬 Generated commit message:");
        println!();
        println!("{}", message.green().bold());
        println!();
    } else {
        println!("💬 Generated commit message:");
        println!();
        println!("{message}");
        println!();
    }
}

struct Profile {
    enabled: bool,
    last: Instant,
    samples: Vec<(&'static str, Duration)>,
}

impl Profile {
    fn new() -> Self {
        let enabled = std::env::var("SPARKLE_PROFILE").is_ok();
        Self {
            enabled,
            last: Instant::now(),
            samples: Vec::new(),
        }
    }

    fn mark(&mut self, label: &'static str) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        let duration = now.duration_since(self.last);
        self.samples.push((label, duration));
        self.last = now;
    }

    fn print_if_enabled(&self) {
        if !self.enabled {
            return;
        }
        println!();
        println!("Profile (SPARKLE_PROFILE=1):");
        for (label, duration) in &self.samples {
            println!("  {label}: {:.2?}", duration);
        }
    }
}

enum UiEvent {
    Step {
        index: usize,
        status: ui::StepStatus,
    },
    Log(String),
    Completed(Option<String>, Profile),
    Failed(String),
}

fn run_pipeline(
    cli: Cli,
    tx: std::sync::mpsc::Sender<UiEvent>,
) -> Result<(Option<String>, Profile), Box<dyn Error>> {
    let mut profile = Profile::new();
    let send_step = |index: usize, status: ui::StepStatus| {
        let _ = tx.send(UiEvent::Step { index, status });
    };

    send_step(0, ui::StepStatus::Running);
    let llm_client = llm::Client::new()?;
    send_step(0, ui::StepStatus::Done);
    profile.mark("init client");

    send_step(1, ui::StepStatus::Running);
    let prompt_config = prompt::load_prompt_config()?;
    prompt::validate_context_policy(&prompt_config.context_policy)?;
    send_step(1, ui::StepStatus::Done);
    profile.mark("load prompt config");

    send_step(2, ui::StepStatus::Running);
    let staged_changes = git::get_staged_changes()?;
    if staged_changes.trim().is_empty() {
        let _ = tx.send(UiEvent::Log(
            "No staged changes in the repository.".to_string(),
        ));
        send_step(2, ui::StepStatus::Done);
        profile.mark("collect changes");
        return Ok((None, profile));
    }
    let staged_summary = git::get_staged_summary()?;
    send_step(2, ui::StepStatus::Done);
    profile.mark("collect changes");

    let examples_count = parse_examples_count(cli.examples)?;
    let mut latest_commit_messages = String::new();
    if examples_count > 0 {
        latest_commit_messages = git::get_commit_messages(examples_count)?;
        let _ = tx.send(UiEvent::Log(format!(
            "Adding {} example(s) of previous commit messages to context",
            examples_count
        )));
    }

    let _ = tx.send(UiEvent::Log(format!(
        "Language for commit message: {}",
        cli.language
    )));

    send_step(3, ui::StepStatus::Running);
    let model_chain = resolve_model_chain(&cli.model, &prompt_config.model_policy)?;
    let model_display = if cli.model == "auto" {
        format!("auto -> {}", model_chain.join(", "))
    } else {
        model_chain.join(", ")
    };
    let _ = tx.send(UiEvent::Log(format!("Model selection: {model_display}")));
    send_step(3, ui::StepStatus::Done);

    send_step(4, ui::StepStatus::Running);
    let context = GenerationContext {
        prompt_config: &prompt_config,
        policy: &prompt_config.context_policy,
        staged_summary: &staged_summary,
        staged_changes: &staged_changes,
        model_chain: &model_chain,
        language: &cli.language,
        examples: &latest_commit_messages,
    };
    let commit_msg = generate_with_fallbacks(&llm_client, &context, |message| {
        let _ = tx.send(UiEvent::Log(message));
    })?;
    send_step(4, ui::StepStatus::Done);
    profile.mark("generate message");

    let mut commit_msg = sanitize_commit_message(&commit_msg);
    if commit_msg.is_empty() {
        return Err("generated commit message is empty".into());
    }
    if !commit_msg.ends_with('\n') {
        commit_msg.push('\n');
    }

    send_step(5, ui::StepStatus::Running);
    git::commit_with_message(&commit_msg, true)?;
    send_step(5, ui::StepStatus::Done);
    profile.mark("commit");

    Ok((Some(commit_msg), profile))
}

struct GenerationContext<'a> {
    prompt_config: &'a prompt::PromptConfig,
    policy: &'a prompt::ContextPolicy,
    staged_summary: &'a str,
    staged_changes: &'a str,
    model_chain: &'a [String],
    language: &'a str,
    examples: &'a str,
}

fn generate_with_fallbacks(
    llm_client: &llm::Client,
    context: &GenerationContext<'_>,
    mut log: impl FnMut(String),
) -> Result<String, Box<dyn Error>> {
    let attempts = [
        (
            context.policy.budgets.primary_tokens,
            ContextMode::Full,
            "primary",
        ),
        (
            context.policy.budgets.fallback_tokens,
            ContextMode::Full,
            "fallback",
        ),
        (
            context.policy.budgets.minimal_tokens,
            ContextMode::RequiredOnly,
            "minimal",
        ),
    ];

    let mut last_error: Option<String> = None;
    for (model_index, model) in context.model_chain.iter().enumerate() {
        for (budget_index, (budget, mode, label)) in attempts.iter().enumerate() {
            let (changes_context, truncated) = build_changes_context(
                context.staged_summary,
                context.staged_changes,
                context.policy,
                *budget,
                *mode,
            );

            if truncated {
                log(format!("Input truncated under {label} context budget."));
            }

            match llm_client.generate_commit_message(
                context.prompt_config,
                &changes_context,
                model,
                context.language,
                context.examples,
            ) {
                Ok(message) => return Ok(message),
                Err(err) if is_payload_too_large(&err.to_string()) => {
                    if let Some((_, _, next_label)) = attempts.get(budget_index + 1) {
                        log(format!(
                            "Request too large; retrying with {next_label} budget."
                        ));
                    } else if let Some(next_model) = context.model_chain.get(model_index + 1) {
                        log(format!(
                            "Request too large; retrying with model {next_model}."
                        ));
                    }
                    last_error = Some(err.to_string());
                    continue;
                }
                Err(err) => return Err(err),
            }
        }
    }

    Err(last_error
        .unwrap_or_else(|| "request failed after all retries".to_string())
        .into())
}

fn build_changes_context(
    summary: &str,
    diff: &str,
    policy: &prompt::ContextPolicy,
    budget_tokens: usize,
    mode: ContextMode,
) -> (String, bool) {
    let max_chars = budget_tokens.saturating_mul(policy.token_char_ratio);
    let mut truncated = false;
    let mut remaining = max_chars;
    let mut carry = 0usize;
    let mut context = String::new();

    let sections = policy
        .sections
        .iter()
        .filter(|section| mode == ContextMode::Full || section.required);

    for section in sections {
        if remaining == 0 {
            break;
        }

        let base_limit = ((max_chars as f64) * section.max_ratio).floor() as usize;
        let mut allowed = base_limit.saturating_add(carry);
        if allowed > remaining {
            allowed = remaining;
        }
        if allowed == 0 {
            carry = 0;
            continue;
        }

        let header_len = section.header.len();
        if header_len >= allowed {
            if section.required {
                let header_trimmed = truncate_to_len(&section.header, allowed);
                if header_trimmed.len() < section.header.len() {
                    truncated = true;
                }
                context.push_str(&header_trimmed);
                remaining = remaining.saturating_sub(header_trimmed.len());
            }
            carry = 0;
            continue;
        }

        let content_limit = allowed - header_len;
        let source = match section.source {
            prompt::ContextSource::Summary => summary,
            prompt::ContextSource::Diff => diff,
        };
        let content_trimmed = truncate_to_len(source, content_limit);
        if content_trimmed.len() < source.len() {
            truncated = true;
        }

        if content_trimmed.is_empty() && !section.required {
            carry = allowed;
            continue;
        }

        context.push_str(&section.header);
        context.push_str(&content_trimmed);

        let used = header_len + content_trimmed.len();
        remaining = remaining.saturating_sub(used);
        carry = allowed.saturating_sub(used);
    }

    (context, truncated)
}

#[derive(Copy, Clone, PartialEq, Eq)]
enum ContextMode {
    Full,
    RequiredOnly,
}

fn truncate_to_len(input: &str, max_len: usize) -> String {
    if input.len() <= max_len {
        return input.to_string();
    }

    let mut end = max_len;
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }

    input[..end].to_string()
}

fn is_payload_too_large(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("status 413")
        || lower.contains("payload too large")
        || lower.contains("tokens_limit_reached")
}

fn resolve_model_chain(
    requested: &str,
    policy: &prompt::ModelPolicy,
) -> Result<Vec<String>, Box<dyn Error>> {
    if requested == "auto" {
        if policy.auto_models.is_empty() {
            return Err("auto model list is empty in prompt config".into());
        }
        return Ok(policy.auto_models.clone());
    }

    Ok(vec![requested.to_string()])
}

fn sanitize_commit_message(message: &str) -> String {
    let mut lines = Vec::new();
    for line in message.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            let rest = trimmed.trim_start_matches("```").trim_start();
            if !rest.is_empty() && !is_fence_language(rest) {
                lines.push(rest);
            }
            continue;
        }
        lines.push(line);
    }

    let mut sanitized = lines.join("\n").trim().to_string();
    if sanitized.starts_with("```") {
        sanitized = sanitized.trim_start_matches("```").trim_start().to_string();
    }
    if sanitized.ends_with("```") {
        sanitized = sanitized.trim_end_matches("```").trim_end().to_string();
    }

    sanitized
}

fn is_fence_language(tag: &str) -> bool {
    tag.chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_examples_count_accepts_valid_values() {
        assert_eq!(parse_examples_count(None).unwrap(), 0);
        assert_eq!(parse_examples_count(Some("3".to_string())).unwrap(), 3);
    }

    #[test]
    fn parse_examples_count_rejects_invalid_values() {
        assert!(parse_examples_count(Some("0".to_string())).is_err());
        assert!(parse_examples_count(Some("21".to_string())).is_err());
        assert!(parse_examples_count(Some("nope".to_string())).is_err());
    }

    #[test]
    fn build_changes_context_keeps_content_when_budget_allows() {
        let policy = prompt::ContextPolicy {
            token_char_ratio: 1,
            budgets: prompt::ContextBudgets {
                primary_tokens: 10,
                fallback_tokens: 5,
                minimal_tokens: 2,
            },
            sections: vec![
                prompt::ContextSection {
                    source: prompt::ContextSource::Summary,
                    header: "Summary of staged changes:\n".to_string(),
                    max_ratio: 0.5,
                    required: true,
                },
                prompt::ContextSection {
                    source: prompt::ContextSource::Diff,
                    header: "\n\nStaged diff (truncated if necessary):\n".to_string(),
                    max_ratio: 0.5,
                    required: false,
                },
            ],
        };
        let summary = "summary";
        let diff = "diff";
        let (context, truncated) =
            build_changes_context(summary, diff, &policy, 200, ContextMode::Full);
        assert!(!truncated);
        assert!(context.contains(summary));
        assert!(context.contains(diff));
    }

    #[test]
    fn build_changes_context_marks_truncation_when_budget_is_small() {
        let policy = prompt::ContextPolicy {
            token_char_ratio: 1,
            budgets: prompt::ContextBudgets {
                primary_tokens: 10,
                fallback_tokens: 5,
                minimal_tokens: 2,
            },
            sections: vec![
                prompt::ContextSection {
                    source: prompt::ContextSource::Summary,
                    header: "Summary of staged changes:\n".to_string(),
                    max_ratio: 1.0,
                    required: true,
                },
                prompt::ContextSection {
                    source: prompt::ContextSource::Diff,
                    header: "\n\nStaged diff (truncated if necessary):\n".to_string(),
                    max_ratio: 0.1,
                    required: false,
                },
            ],
        };
        let summary = "summary";
        let diff = "diff";
        let (context, truncated) =
            build_changes_context(summary, diff, &policy, 1, ContextMode::Full);
        assert!(truncated);
        assert!(!context.is_empty());
    }

    #[test]
    fn sanitize_commit_message_removes_code_fences() {
        let input = "```\nfeat: add tests\n```\n";
        assert_eq!(sanitize_commit_message(input), "feat: add tests");
    }

    #[test]
    fn sanitize_commit_message_preserves_inline_message_after_fence() {
        let input = "```feat: add tests\n";
        assert_eq!(sanitize_commit_message(input), "feat: add tests");
    }

    #[test]
    fn is_payload_too_large_detects_error_signals() {
        assert!(is_payload_too_large("status 413"));
        assert!(is_payload_too_large("Payload Too Large"));
        assert!(is_payload_too_large("tokens_limit_reached"));
        assert!(!is_payload_too_large("other error"));
    }
}
