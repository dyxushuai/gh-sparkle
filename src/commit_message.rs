use std::error::Error;
use std::fmt;

const ALLOWED_TYPES: [&str; 10] = [
    "feat", "fix", "refactor", "docs", "test", "chore", "perf", "build", "ci", "revert",
];

#[derive(Debug)]
pub(crate) struct CommitMessageError {
    message: String,
}

impl CommitMessageError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for CommitMessageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for CommitMessageError {}

#[derive(Debug, Clone)]
pub(crate) struct NormalizedCommitMessage {
    pub(crate) message: String,
    pub(crate) warnings: Vec<String>,
}

pub(crate) fn normalize_commit_message(
    raw: &str,
) -> Result<NormalizedCommitMessage, CommitMessageError> {
    let sanitized = sanitize_commit_message(raw);

    let mut last_error: Option<CommitMessageError> = None;
    for line in sanitized.lines() {
        let candidate = strip_wrapping_quotes(line.trim());
        if candidate.is_empty() {
            continue;
        }

        let mut normalized = normalize_subject_line(candidate);
        if normalized.is_err() {
            if let Some(stripped) = strip_bullet_prefix(candidate) {
                normalized = normalize_subject_line(stripped);
            }
        }

        match normalized {
            Ok((subject, warnings)) => {
                let mut warnings = warnings;
                let subject_len = subject.chars().count();
                if subject_len > 72 {
                    warnings.push(format!(
                        "Commit subject is {subject_len} characters (recommended <= 72)."
                    ));
                }
                return Ok(NormalizedCommitMessage {
                    message: format!("{subject}\n"),
                    warnings,
                });
            }
            Err(err) => {
                last_error = Some(err);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        CommitMessageError::new("model output did not contain a valid Conventional Commit subject")
    }))
}

fn normalize_subject_line(line: &str) -> Result<(String, Vec<String>), CommitMessageError> {
    let line = strip_wrapping_quotes(line.trim());
    if line.is_empty() {
        return Err(CommitMessageError::new("commit subject is empty"));
    }

    if line.starts_with(['-', '*', '•']) {
        return Err(CommitMessageError::new(
            "commit subject must not be a bullet list item",
        ));
    }
    if line.contains("```") {
        return Err(CommitMessageError::new(
            "commit subject must not contain markdown fences",
        ));
    }

    let colon_index = line
        .find(':')
        .ok_or_else(|| CommitMessageError::new("commit subject is missing ':' delimiter"))?;
    let raw_header = line[..colon_index].trim();
    let raw_description = line[colon_index + 1..].trim();

    if raw_header.is_empty() {
        return Err(CommitMessageError::new(
            "commit subject is missing header before ':'",
        ));
    }
    if raw_description.is_empty() {
        return Err(CommitMessageError::new(
            "commit subject is missing description after ':'",
        ));
    }

    let (commit_type, scope, breaking) = parse_header(raw_header)?;
    let description = normalize_description(raw_description);

    let mut subject = String::new();
    subject.push_str(&commit_type);
    if let Some(scope) = scope {
        subject.push('(');
        subject.push_str(&scope);
        subject.push(')');
    }
    if breaking {
        subject.push('!');
    }
    subject.push_str(": ");
    subject.push_str(&description);

    Ok((subject, Vec::new()))
}

fn parse_header(raw: &str) -> Result<(String, Option<String>, bool), CommitMessageError> {
    let mut header = raw.trim();
    let mut breaking = false;
    if let Some(stripped) = header.strip_suffix('!') {
        breaking = true;
        header = stripped.trim_end();
    }

    if header.contains(')') && !header.contains('(') {
        return Err(CommitMessageError::new(
            "commit header contains ')' without '('",
        ));
    }

    let (ty, scope) = match header.split_once('(') {
        Some((ty, rest)) => {
            let rest = rest.trim();
            if !rest.ends_with(')') {
                return Err(CommitMessageError::new(
                    "commit header has an unterminated scope",
                ));
            }
            let scope = rest[..rest.len() - 1].trim();
            if scope.is_empty() {
                return Err(CommitMessageError::new("commit scope must not be empty"));
            }
            if scope.chars().any(char::is_whitespace) {
                return Err(CommitMessageError::new(
                    "commit scope must not contain whitespace",
                ));
            }
            (ty.trim(), Some(scope.to_string()))
        }
        None => (header, None),
    };

    let commit_type = normalize_type(ty)
        .ok_or_else(|| CommitMessageError::new(format!("unsupported commit type: {ty}")))?;

    Ok((commit_type.to_string(), scope, breaking))
}

fn normalize_type(raw: &str) -> Option<&'static str> {
    let lower = raw.trim().to_ascii_lowercase();
    ALLOWED_TYPES
        .iter()
        .copied()
        .find(|allowed| *allowed == lower)
}

fn normalize_description(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_bullet_prefix(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    for prefix in ["- ", "* ", "• "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return Some(rest.trim_start());
        }
    }
    None
}

fn strip_wrapping_quotes(input: &str) -> &str {
    let trimmed = input.trim();
    if trimmed.len() < 2 {
        return trimmed;
    }

    let bytes = trimmed.as_bytes();
    let first = bytes[0];
    let last = bytes[bytes.len() - 1];

    if first == last && matches!(first, b'"' | b'\'' | b'`') {
        return trimmed[1..trimmed.len() - 1].trim();
    }

    trimmed
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
    fn normalize_commit_message_accepts_simple_subject() {
        let normalized = normalize_commit_message("feat(cli): add spinner").unwrap();
        assert_eq!(normalized.message, "feat(cli): add spinner\n");
    }

    #[test]
    fn normalize_commit_message_finds_subject_after_leading_text() {
        let normalized = normalize_commit_message("Here you go:\nfeat: add tests\n").unwrap();
        assert_eq!(normalized.message, "feat: add tests\n");
    }

    #[test]
    fn normalize_commit_message_rejects_translated_type() {
        assert!(normalize_commit_message("修复: 修正 bug").is_err());
    }

    #[test]
    fn normalize_commit_message_accepts_bullet_prefixed_subject() {
        let normalized = normalize_commit_message("- feat: add tests").unwrap();
        assert_eq!(normalized.message, "feat: add tests\n");
    }

    #[test]
    fn normalize_commit_message_lowercases_header_type() {
        let normalized = normalize_commit_message("FIX: handle edge cases").unwrap();
        assert_eq!(normalized.message, "fix: handle edge cases\n");
    }

    #[test]
    fn normalize_commit_message_warns_when_subject_too_long() {
        let raw = "feat: this subject is intentionally made very long so it exceeds seventy two characters for warning coverage";
        let normalized = normalize_commit_message(raw).unwrap();
        assert_eq!(
            normalized.message,
            "feat: this subject is intentionally made very long so it exceeds seventy two characters for warning coverage\n"
        );
        assert_eq!(normalized.warnings.len(), 1);
        assert!(normalized.warnings[0].contains("recommended <= 72"));
    }

    #[test]
    fn parse_header_rejects_scope_with_whitespace() {
        assert!(parse_header("feat(api core)").is_err());
    }

    #[test]
    fn parse_header_supports_breaking_change_marker() {
        let (ty, scope, breaking) = parse_header("feat(api)!").unwrap();
        assert_eq!(ty, "feat");
        assert_eq!(scope.as_deref(), Some("api"));
        assert!(breaking);
    }
}
