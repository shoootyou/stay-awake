# Contributing to Stay Awake

Thank you for your interest in contributing! This document covers the conventions
used in this project.

## Conventional Commits

All commit messages **must** follow the
[Conventional Commits](https://www.conventionalcommits.org/) specification:

```
type(scope): description
```

`scope` is optional. Keep the description concise and lowercase.

### Allowed types

| Type       | Purpose                                         |
| ---------- | ----------------------------------------------- |
| `feat`     | A new feature                                   |
| `fix`      | A bug fix                                       |
| `docs`     | Documentation only changes                      |
| `style`    | Code style (formatting, missing semicolons, etc)|
| `refactor` | Code change that neither fixes a bug nor adds a feature |
| `perf`     | Performance improvement                         |
| `test`     | Adding or correcting tests                      |
| `build`    | Changes to the build system or dependencies     |
| `ci`       | Changes to CI configuration files and scripts   |
| `chore`    | Other changes that don't modify src or test files|
| `revert`   | Reverts a previous commit                       |

### Examples

```
feat: add scheduling feature
fix: resolve tray icon flicker
docs: update README installation section
refactor(jiggler): simplify movement pattern logic
ci: add commitlint job to CI workflow
chore: bump dependencies
```

### Breaking changes

Append `!` after the type/scope or add a `BREAKING CHANGE:` footer:

```
feat!: redesign configuration file format
```

## Versioning

This project uses **semantic versioning**. Version bumps are determined
automatically from commit messages:

- `fix:` commits trigger a **patch** bump (0.0.X)
- `feat:` commits trigger a **minor** bump (0.X.0)
- `BREAKING CHANGE` or `!` commits trigger a **major** bump (X.0.0)

## Pull Requests

1. Fork and create a feature branch from `main`.
2. Make your changes with conventional commit messages.
3. Ensure `cargo fmt --check` and `cargo clippy` pass in `src-tauri/`.
4. Open a PR against `main`. CI will validate your commits automatically.
