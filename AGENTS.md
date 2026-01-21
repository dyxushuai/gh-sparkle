# AGENTS.md

## Purpose and scope
This repository contains `gh sparkle`, a Rust GitHub CLI extension that
generates Conventional Commit messages from staged diffs using GitHub Models and
commits automatically. In scope: Rust code, prompt YAML, docs, and CI workflows
that ship the extension. Out of scope: changing the LLM provider or removing the
auto-commit flow without explicit approval.

## Commands (copy/paste, include flags)
- Build: `cargo build`
- Run (help): `cargo run -- --help`
- Format: `cargo fmt --all --check`
- Lint: `cargo clippy --all-targets --all-features --locked -- -D warnings`
- Test: `cargo nextest run --no-fail-fast -j num-cpus`

## Tech stack (with versions)
- Language/runtime: Rust (edition 2024)
- CLI: clap v4 (derive)
- HTTP: reqwest v0.12 (blocking + rustls)
- Serialization: serde, serde_json, serde_yaml
- External tooling: GitHub CLI (`gh auth token`)

## Repo map
- `src/` - Rust sources (`main.rs`, `git.rs`, `llm.rs`)
- `assets/commitmsg.prompt.yml` - prompt template
- `.github/workflows/` - CI and release workflows
- `extension.yml` - gh extension metadata
- `README.md` - user docs

## Code style example (real snippet)
```rust
fn sanitize_commit_message(message: &str) -> String {
    let mut lines = Vec::new();
    for line in message.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            continue;
        }
        lines.push(line);
    }
    lines.join("\n").trim().to_string()
}
```

## Standards
- Keep docs and comments in English.
- Keep prompt edits in `assets/commitmsg.prompt.yml` only.
- Preserve the auto-commit flow unless explicitly requested.
- Avoid adding unnecessary dependencies.

## Change management
- Default branch: `main`.
- Commit format: Conventional Commits, English only, subject <= 72 chars.
- Stage changes atomically (`git add -p`) before committing.

## Dependencies and environment
- Requires `gh` authenticated on the target host.
- Uses `GH_TOKEN`/`GITHUB_TOKEN` or `gh auth token` for GitHub Models.
- Network access required for GitHub Models API.

## Boundaries (Always / Ask first / Never)
- Always: run `cargo fmt --all --check` before submitting changes.
- Ask first: adding new dependencies, changing default model/limits, modifying CI.
- Never: commit secrets or tokens, rewrite git history, remove auto-commit flow.
