# No Sleep Please!

**Keep your computer awake and active.**

No Sleep Please! is a lightweight desktop utility that prevents your computer from going idle — no admin privileges required. It simulates subtle mouse movements and system-level activity to keep your machine awake.

---

## Features

- **Dual mechanism** — combines mouse jiggling with OS-level sleep prevention for maximum reliability
- **Cross-platform** — native builds for macOS and Windows
- **System tray** — lives quietly in your menu bar / system tray, out of the way
- **Configurable** — adjust jiggle interval, movement distance, and enable/disable individual mechanisms
- **Global shortcut** — toggle on/off without opening the settings window
- **Auto-start** — optionally launch at login
- **Tiny footprint** — built with Rust and Tauri for minimal resource usage

## Installation

### macOS

**Homebrew** *(coming soon)*
```bash
brew install rodolfo/tap/non-sleep
```

**Direct download** — grab the `.dmg` from the [latest release](../../releases/latest).

### Windows

**winget** *(coming soon)*
```powershell
winget install Rodolfo.NonSleep
```

**Direct download** — grab the `.exe` installer from the [latest release](../../releases/latest).

## Usage

1. Launch No Sleep Please! — it appears in your system tray (macOS menu bar or Windows notification area)
2. Click the tray icon to open the context menu
3. Select **Toggle Active** to start keeping your computer awake
4. Choose a **Mode** (Power Only is the default and needs no extra permissions)
5. Open **Settings** to adjust the interval, autostart, and other preferences

For a full walkthrough of every feature, see the [User Guide](docs/user-guide.md).

## Development

### Prerequisites

- [Rust](https://rustup.rs/) (stable)
- [Node.js](https://nodejs.org/) 22+
- Platform-specific dependencies:
  - **macOS** — Xcode Command Line Tools (`xcode-select --install`)
  - **Windows** — [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with C++ workload

### Setup

```bash
git clone https://github.com/shoootyou/non-sleep-please.git
cd non-sleep-please
npm install
```

### Commands

```bash
# Run in development mode (hot-reload)
npm run tauri dev

# Build a release bundle
npm run tauri build

# Run Rust tests
cd src-tauri && cargo test

# Lint Rust code
cd src-tauri && cargo clippy --all-targets -- -D warnings
```

## Roadmap

- [ ] Homebrew cask distribution
- [ ] winget package submission
- [ ] Linux support (X11 + Wayland)
- [ ] Scheduled activation (e.g., only during work hours)
- [ ] Idle-time detection (auto-activate after N minutes of inactivity)
- [ ] Localization (i18n)

## License

[MIT](LICENSE) © 2025 Rodolfo Castelo
