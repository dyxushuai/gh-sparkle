// Git helpers for staged changes and commits.

use std::error::Error;
use std::io::Write;
use std::process::{Command, Stdio};

pub fn get_staged_changes() -> Result<String, Box<dyn Error>> {
    if !is_git_repository() {
        return Err("current directory is not a git repository".into());
    }

    let output = Command::new("git").args(["diff", "--staged"]).output()?;

    if !output.status.success() {
        return Err(format!("error executing git diff --staged: {}", output.status).into());
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn get_commit_messages(count: usize) -> Result<String, Box<dyn Error>> {
    if !is_git_repository() {
        return Err("current directory is not a git repository".into());
    }

    let output = Command::new("git")
        .args(["log", "-n", &count.to_string()])
        .output()?;

    if !output.status.success() {
        return Err(format!("error executing git log: {}", output.status).into());
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn commit_with_message(message: &str) -> Result<(), Box<dyn Error>> {
    if !is_git_repository() {
        return Err("current directory is not a git repository".into());
    }

    let mut child = Command::new("git")
        .args(["commit", "-F", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or("failed to open git commit stdin")?;
        stdin.write_all(message.as_bytes())?;
    }

    let status = child.wait()?;
    if !status.success() {
        return Err(format!("git commit failed with status {}", status).into());
    }

    Ok(())
}

fn is_git_repository() -> bool {
    Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}
