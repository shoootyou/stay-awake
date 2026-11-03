# Contributing to Stay Awake

This guide covers local setup, checks, and contribution conventions.

## Development Prerequisites

- Rust `1.99.1`, pinned in `rust-toolchain.toml`
- Node.js 26, matching CI
- Tauri CLI: `cargo install tauri-cli`

### Linux system dependencies

On Linux, run the same prerequisite installer used by CI and release builds from the repository root:

```sh
scripts/install-linux-dependencies.sh
```

The script uses `sudo` when it is not run as root.

Runtime notes for Linux users/contributors testing the app:

- NetworkManager is required for WiFi mode.
- `xdotool` is recommended for mouse-jiggle modes. `Power Only` works without it.
- Linux `.deb` installs do not include an in-app updater.

## Run checks locally

Run the Rust checks from `src-tauri`:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

Run the release-workflow contract tests from the repository root:

```sh
npm ci && npm test
```

## Conventional Commits

All commit messages must follow the
[Conventional Commits](https://www.conventionalcommits.org/) specification:

```
type(scope): description
```

`scope` is optional. Keep the description concise and lowercase.
CI runs `commitlint` for each pull request using `commitlint.config.cjs`.

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

This project uses semantic versioning. Version bumps are determined automatically from commit messages:

- `fix:` commits trigger a **patch** bump (0.0.X)
- `feat:` commits trigger a **minor** bump (0.X.0)
- `BREAKING CHANGE` or `!` commits trigger a **major** bump (X.0.0)

Release Please updates the versions in `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, and `package.json`. A release-workflow contract test checks that the frontend and Tauri versions match.

## Releases

Release Please opens the release PR. When a release is created, the release workflow finds or creates the GitHub release for the tag without changing notes on an existing release. It then builds macOS and Linux in parallel with `tauri-apps/tauri-action` `action-v1.0.0`.

The macOS build uploads the signed, notarized DMG and updater metadata. The Linux build uploads the amd64 `.deb`; it is nonblocking, so a Linux failure leaves the release and Homebrew cask update running. The workflow adds a warning annotation and run summary when that happens. The distribution workflow updates `shoootyou/homebrew-tap` from the macOS DMG.

## Pull Requests

1. Fork and create a feature branch from `main`.
2. Make your changes with conventional commit messages.
3. Run `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`, and `cargo test --locked` in `src-tauri/`, then run `npm test` from the repository root. On Linux, first run `scripts/install-linux-dependencies.sh` from the repository root.
4. Open a PR against `main`. CI will validate your commits automatically.
