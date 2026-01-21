// CLI entrypoint for gh-sparkle.

mod git;
mod llm;

use clap::Parser;
use std::error::Error;

const EXTENSION_NAME: &str = "sparkle";
const DEFAULT_MODEL: &str = "openai/gpt-4o-mini";
const MAX_EXAMPLES: usize = 20;
const PRIMARY_INPUT_BUDGET_TOKENS: usize = 12_000;
const FALLBACK_INPUT_BUDGET_TOKENS: usize = 6_000;
const TOKEN_CHAR_RATIO: usize = 4;

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
    let cli = Cli::parse();

    let staged_changes = git::get_staged_changes()?;
    if staged_changes.trim().is_empty() {
        println!("No staged changes in the repository.");
        return Ok(());
    }

    let staged_summary = git::get_staged_summary()?;

    let examples_count = parse_examples_count(cli.examples)?;

    let mut latest_commit_messages = String::new();
    if examples_count > 0 {
        latest_commit_messages = git::get_commit_messages(examples_count)?;
        println!(
            "  Adding {} example(s) of previous commit messages to context",
            examples_count
        );
    }

    let llm_client = llm::Client::new()?;

    println!("  Language for commit message: {}", cli.language);

    let (changes_context, truncated) = build_changes_context(
        &staged_summary,
        &staged_changes,
        PRIMARY_INPUT_BUDGET_TOKENS,
    );

    if truncated {
        println!("  Input too large; diff truncated to fit token budget.");
    }

    let commit_msg = match llm_client.generate_commit_message(
        &changes_context,
        &cli.model,
        &cli.language,
        &latest_commit_messages,
    ) {
        Ok(message) => message,
        Err(err) if is_payload_too_large(&err.to_string()) => {
            println!("  Request too large; retrying with a smaller input budget.");
            let (fallback_context, fallback_truncated) = build_changes_context(
                &staged_summary,
                &staged_changes,
                FALLBACK_INPUT_BUDGET_TOKENS,
            );
            if fallback_truncated {
                println!("  Diff truncated for retry.");
            }
            llm_client.generate_commit_message(
                &fallback_context,
                &cli.model,
                &cli.language,
                &latest_commit_messages,
            )?
        }
        Err(err) => return Err(err),
    };

    let mut commit_msg = sanitize_commit_message(&commit_msg);
    if commit_msg.is_empty() {
        return Err("generated commit message is empty".into());
    }

    if !commit_msg.ends_with('\n') {
        commit_msg.push('\n');
    }

    println!("💬 Generated commit message:\n{}", commit_msg.trim_end());

    println!("  Committing staged changes...");
    git::commit_with_message(&commit_msg)?;

    Ok(())
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

fn build_changes_context(summary: &str, diff: &str, budget_tokens: usize) -> (String, bool) {
    let max_chars = budget_tokens.saturating_mul(TOKEN_CHAR_RATIO);
    let summary_header = "Summary of staged changes:\n";
    let diff_header = "\n\nStaged diff (truncated if necessary):\n";

    let mut truncated = false;
    let mut context = String::new();
    context.push_str(summary_header);

    let remaining_for_summary = max_chars.saturating_sub(context.len());
    let summary_trimmed = truncate_to_len(summary, remaining_for_summary);
    if summary_trimmed.len() < summary.len() {
        truncated = true;
    }
    context.push_str(&summary_trimmed);

    let remaining_for_diff = max_chars.saturating_sub(context.len() + diff_header.len());
    if remaining_for_diff == 0 {
        return (context, truncated);
    }

    let diff_trimmed = truncate_to_len(diff, remaining_for_diff);
    if diff_trimmed.len() < diff.len() {
        truncated = true;
    }

    context.push_str(diff_header);
    context.push_str(&diff_trimmed);

    (context, truncated)
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
