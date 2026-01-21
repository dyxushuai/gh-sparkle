# gh-sparkle

A GitHub CLI extension that generates AI-powered conventional commit messages
from staged git changes using GitHub Models, then commits automatically.

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
gh sparkle --language russian

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

## Upgrade

```bash
gh extension upgrade sparkle
```
