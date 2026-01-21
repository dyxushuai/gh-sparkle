# :gh <✨/sparkle>

A GitHub CLI extension that brings the VS Code "Generate Commit Message"
experience to your terminal. It reads staged changes, asks GitHub Models for a
Conventional Commit message, then commits automatically.

## Why the name

The name comes from the ✨/sparkle icon used by VS Code's "Generate Commit
Message" feature.

## Why this exists

I wanted the same flow as VS Code's commit message generator, but inside `gh`.
`sparkle` keeps that loop tight: stage, generate, commit.

## Features

- Copilot-style commit message generation from staged diffs
- Defaults to `openai/gpt-4o-mini` with safe input trimming for large changes
- Supports `--language`, `--examples`, and `--model`
- Commits staged changes automatically

## Large changes handling

`sparkle` is optimized for big diffs by combining a summary with a trimmed
patch. It avoids API failures by capping input size and retrying with a smaller
budget when needed.

Input budgets are currently defined in code and can be adjusted in
`src/main.rs`.

## Prerequisites

- GitHub CLI installed and authenticated (`gh auth login`)
- A git repository with staged changes

If you use GitHub Enterprise, make sure your host is authenticated:

```bash
gh auth login --hostname <your-host>
```

## Installation

```bash
gh extension install dyxushuai/gh-sparkle
```

## Usage

Stage your changes and run:

```bash
git add .
gh sparkle
```

### Options

```bash
# Generate commit message in a different language
gh sparkle --language chinese

# Add previous commit messages as examples (default 3 when flag is present)
gh sparkle --examples

# Or specify the number of examples (max 20)
gh sparkle --examples 5

# Use a different GitHub Models model
gh sparkle --model xai/grok-3-mini
```

## Notes

- The extension commits automatically using the generated message.
- If there are no staged changes, it exits without committing.
- Large diffs are truncated to fit model input limits.

## Upgrade

```bash
gh extension upgrade sparkle
```
