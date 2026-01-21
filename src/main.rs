// CLI entrypoint for gh-sparkle.

mod git;
mod llm;

use clap::Parser;
use std::error::Error;

const EXTENSION_NAME: &str = "sparkle";
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
    #[arg(short = 'm', long = "model", default_value = "openai/gpt-4o")]
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

    let commit_msg = llm_client.generate_commit_message(
        &staged_changes,
        &cli.model,
        &cli.language,
        &latest_commit_messages,
    )?;

    let mut commit_msg = commit_msg.trim().to_string();
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
