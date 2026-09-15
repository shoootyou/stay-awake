# Contributing to Stay Awake

Thank you for your interest in contributing! This document covers the conventions
used in this project.

## Development Prerequisites

- Rust (stable) — install via [rustup](https://rustup.rs)
- Node.js 22+
- Tauri CLI — `cargo install tauri-cli`

### Linux system dependencies

Building on Linux requires the following system libraries (needed to compile Tauri's
GTK/WebKit stack — `cargo check`/`cargo test` will fail to compile without them):

```sh
sudo apt-get install -y \
  libgtk-3-dev \
  libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf
```

Runtime notes for Linux users/contributors testing the app:

- **NetworkManager** is required for WiFi mode.
- **`xdotool`** is recommended (not required) for the mouse-jiggle modes; `Power Only` mode
  works without it. The `.deb` package declares it as a `Recommends`.
- Linux updates ship via the `.deb` package on GitHub Releases, not the in-app updater.

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
