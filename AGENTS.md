# AGENTS.md

> Agent-facing documentation for **No Sleep Please** — a cross-platform mouse jiggler
> and anti-inactivity tray app built with Rust + Tauri v2.

---

## Project overview

| Field           | Value                                      |
|-----------------|--------------------------------------------|
| Name            | No Sleep Please                            |
| Identifier      | `com.shoootyou.no-sleep-please`            |
| Stack           | Rust (backend) + HTML/CSS/JS (frontend)    |
| Framework       | [Tauri v2](https://v2.tauri.app)           |
| Platforms       | macOS, Windows, Linux                      |
| License         | MIT                                        |
| Package manager | npm (frontend), Cargo (Rust)               |

The app runs as a **system tray icon** — no persistent window. Users control it
via tray menu, global hotkey, or a settings window. The Rust engine moves the
mouse cursor in tiny random patterns to simulate activity.

---

## Repository structure

```
.
├── src/                    # Frontend (HTML/CSS/JS served by Tauri webview)
│   ├── settings.html/js/css   # Settings window UI
│   ├── about.html/js/css      # About window UI
│   └── fonts/                 # Geist + Phosphor icon fonts
├── src-tauri/              # Rust backend (Tauri app)
│   ├── src/
│   │   ├── lib.rs             # App setup, commands, tray, menus, state
│   │   ├── main.rs            # Entry point (calls lib)
│   │   ├── engine.rs          # Mouse jiggler engine (schedule, movement)
│   │   ├── i18n.rs            # Internationalization (Fluent)
│   │   └── platform/          # OS-specific implementations
│   │       ├── mod.rs            # Traits: MouseDriver, PowerInhibitor
│   │       ├── macos.rs          # macOS: CGEvent, IOPMAssertionCreateWithName
│   │       ├── windows.rs        # Windows: SendInput, SetThreadExecutionState
│   │       └── linux.rs          # Linux: X11/libxdo, D-Bus inhibit
│   ├── tauri.conf.json        # Tauri config (version, bundle, plugins)
│   ├── Cargo.toml             # Rust dependencies
│   ├── Cargo.lock             # Locked dependency tree
│   ├── audit.toml             # cargo-audit ignore list (Tauri transitive deps)
│   ├── icons/                 # App icons (all sizes + .icns/.ico)
│   ├── locales/               # i18n strings (en.ftl, es.ftl, etc.)
│   └── capabilities/          # Tauri v2 capability permissions
├── .github/
│   ├── workflows/
│   │   ├── ci.yml             # PR/push: commitlint + fmt + clippy + audit + build
│   │   ├── release.yml        # Push to main: release-please + macOS DMG build
│   │   └── distribute.yml     # Manual: Homebrew tap + winget publish
│   ├── dependabot.yml         # Weekly updates: Cargo, npm, GitHub Actions
│   └── copilot-instructions.md
├── release-please-config.json     # Release-please config (package: src-tauri)
├── .release-please-manifest.json  # Current version tracking
├── rust-toolchain.toml            # Pin Rust to stable channel
├── commitlint.config.cjs         # Conventional commits enforcement
├── package.json                   # npm workspace (devDeps: Tauri CLI, commitlint)
├── choco/                         # Chocolatey package (future)
├── winget/                        # winget manifest
└── docs/                          # User guide
```

---

## Setup commands

```bash
# Install frontend dependencies
npm ci

# Install Rust (if not present)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Run in development mode (hot-reload frontend, Rust recompile)
npx tauri dev

# Build production bundle
npx tauri build
```

### Platform-specific prerequisites

**macOS:** Xcode Command Line Tools (`xcode-select --install`)

**Linux:**
```bash
sudo apt-get install -y \
  libgtk-3-dev libwebkit2gtk-4.1-dev libappindicator3-dev \
  librsvg2-dev patchelf
```

**Windows:** [Build Tools for Visual Studio](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
with "Desktop development with C++" workload, plus WebView2 runtime.

---

## Testing and linting

```bash
# Run all checks (from repo root)
cd src-tauri

# Format check
cargo fmt --check

# Lint (warnings = errors)
cargo clippy --all-targets -- -D warnings

# Unit tests
cargo test

# Security audit (uses audit.toml for ignores)
cargo audit

# Commit message lint (from repo root)
npx commitlint --from HEAD~1
```

> **Note:** There are currently 0 unit tests. The codebase has testable pure
> functions (`is_within_schedule`, `parse_shortcut_string`) ready for test coverage.

---

## CI/CD pipeline

### CI (`ci.yml`) — runs on PRs and pushes to main

```
PR push
  ├─→ Commit Style (conventional commits)  ─┐
  ├─→ Rust: Fmt & Clippy  ──────────────────┼─→ Build: {macOS, Windows, Ubuntu}
  └─→ Rust: Security Audit (CVE)  ──────────┘
```

- **Gates run in parallel** for zero added time.
- **Build only starts** when all 3 gates pass.
- Commit Style is PR-only; on push to main it's skipped (and doesn't block build).

### Release (`release.yml`) — runs on push to main

1. **release-please** creates/updates a Release PR with changelog
2. When Release PR is merged → `release_created == true` → **build-macos** job runs
3. build-macos: sign + notarize + upload DMG to GitHub Release

### Distribute (`distribute.yml`) — manual trigger only

Publishes to Homebrew tap and winget. Currently disabled until first release is validated.

---

## Commit conventions

This project uses [Conventional Commits](https://www.conventionalcommits.org/):

```
type(scope): description

body (optional)
```

Allowed types: `feat`, `fix`, `docs`, `style`, `refactor`, `perf`, `test`,
`build`, `ci`, `chore`, `revert`.

Enforced by commitlint in CI. The config ignores `Initial commit` and
`Merge branch` patterns (from unrelated history merges).

---

## Code style

### Rust
- **Edition:** 2021
- **Formatter:** `cargo fmt` (default rustfmt config)
- **Linter:** `cargo clippy` with `-D warnings` (all warnings are errors)
- **No `unwrap()`** in production code paths — use `?` or proper error handling
- **Platform abstraction:** all OS-specific code lives behind traits in `src/platform/`
  - `MouseDriver` — move cursor
  - `PowerInhibitor` — prevent sleep

### Frontend (HTML/CSS/JS)
- Vanilla JS — no framework
- Tauri IPC via `window.__TAURI__`
- Fonts: Geist (text) + Phosphor (icons)

---

## Key architecture decisions

1. **No window by default** — app is tray-only. Settings/About are on-demand windows.
2. **Trait-based platform layer** — `MouseDriver` and `PowerInhibitor` traits with
   per-OS implementations. Adding a new platform = implement the traits.
3. **Fluent i18n** — all user-facing strings in `locales/*.ftl`, auto-detected from OS.
4. **Engine runs on a background thread** — `JiggleEngine` spawns a Rust thread,
   controlled via commands from the frontend.
5. **Schedule support** — jiggling can be limited to specific days/hours.
6. **Global hotkey** — Cmd+Shift+J (macOS) / Ctrl+Shift+J (Windows/Linux) toggles.

---

## Release process

1. Push to `main` with conventional commits
2. `release-please` automatically creates a Release PR with:
   - Version bump in `Cargo.toml`, `Cargo.lock`, and `tauri.conf.json`
   - Generated CHANGELOG.md
3. Merge the Release PR → GitHub Release created → CI builds signed DMG
4. Future: distribute.yml publishes to Homebrew and winget

---

## Secrets and signing

The following secrets are configured in the GitHub `public` environment:

| Secret                             | Purpose                            |
|------------------------------------|------------------------------------|
| `APPLE_CERTIFICATE`               | Base64 .p12 Developer ID cert     |
| `APPLE_CERTIFICATE_PASSWORD`      | Password for the .p12              |
| `APPLE_SIGNING_IDENTITY`          | e.g. "Developer ID Application: …"|
| `APPLE_ID`                        | Apple ID for notarization          |
| `APPLE_PASSWORD`                  | App-specific password              |
| `APPLE_TEAM_ID`                   | 10-char Developer Team ID          |
| `TAURI_SIGNING_PRIVATE_KEY`       | Ed25519 key for updater signing    |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Password for signing key        |
| `TAP_GITHUB_TOKEN`               | Token for Homebrew tap repo        |
| `WINGET_TOKEN`                    | Token for winget submission        |

> **Never commit secrets.** All signing happens in CI via environment secrets.

---

## Known issues

- **18 cargo-audit advisories** are ignored in `audit.toml` — all transitive
  GTK/GLib deps from Tauri v2 that can't be updated until upstream migrates.
- **Zero unit tests** — cargo test passes (compiles) but tests no functions.
- **`infoPlist` in tauri.conf.json** must use string values only — Tauri v2's
  build script rejects booleans (e.g., `false`) with `invalid type: map`.

---

## Useful patterns for agents

### Adding a new Tauri command
1. Define the command in `src-tauri/src/lib.rs` with `#[tauri::command]`
2. Register it in the `.invoke_handler(tauri::generate_handler![...])` call
3. Call from frontend: `window.__TAURI__.core.invoke('command_name', { args })`

### Adding a new locale
1. Create `src-tauri/locales/{lang}.ftl` with Fluent syntax
2. The app auto-detects system locale and falls back to English

### Running only on macOS
```bash
npx tauri dev        # dev mode with hot reload
npx tauri build      # production .app bundle
```

### Debugging tray/menu issues
- macOS: check Console.app for `non-sleep` or `runningboardd` messages
- The app sets `ActivationPolicy::Accessory` (no Dock icon by default)
- Windows open temporarily switch to `ActivationPolicy::Regular`
