# User Guide

Stay Awake is a lightweight tray utility that prevents your computer from going idle.
It runs quietly in your system tray (macOS menu bar or Windows notification area)
and keeps your machine awake using one of several configurable methods.

---

## Getting Started

After launching Stay Awake, you will see an icon in your system tray. The app does
**not** open a window by default — everything is controlled from the tray menu.

- **Left-click** or **right-click** the tray icon to open the menu.

---

## Tray Menu

The tray menu is the main control surface of the app.

### Toggle Active

Turns the anti-inactivity engine on or off.

- **Checked** = the engine is running and your computer will stay awake.
- **Unchecked** = the engine is stopped and normal system idle behavior resumes.

You can also toggle the engine with the global keyboard shortcut (default: `Cmd+Shift+J`
on macOS, `Ctrl+Shift+J` on Windows).

### Mode

Choose how Stay Awake keeps your computer awake. See [Modes](#modes) below for details.

### Accessibility (macOS only)

Shows the current state of the macOS Accessibility permission:

- **Accessibility OK** — permission granted, mouse modes will work.
- **Grant Accessibility** — click to open System Settings and grant the permission.

This only matters if you use a mouse-based mode. The default "Power Only" mode
does **not** require Accessibility.

### Settings

Opens the [Settings window](#settings-window).

### Quit

Stops the engine and exits the application.

---

## Modes

Stay Awake supports four modes. Each mode keeps your computer from going idle, but
they differ in *how* they do it.

### Power Only (default)

Uses the operating system's power management API to prevent sleep:

- **macOS** — creates an IOKit power assertion (`IOPMAssertionCreateWithName`).
- **Windows** — calls `SetThreadExecutionState` to keep the display and system awake.

No mouse movement is simulated. No special permissions are needed.

This is the recommended mode for most users.

### Mouse Subtle

Moves the cursor by 1 pixel to the right, then 1 pixel back to the left. The
movement is invisible in normal use but resets the OS idle timer.

Requires **Accessibility** permission on macOS.

### Mouse Zen

Performs a zero-delta mouse event — a synthetic movement of 0 pixels. The OS
treats it as real activity, but the cursor does not visibly move at all.

Requires **Accessibility** permission on macOS.

### Mouse Circle

Moves the cursor in a tiny square pattern: right, down, left, up (1 pixel each
direction). The cursor returns to its original position after each cycle.

Requires **Accessibility** permission on macOS.

---

## Settings Window

Open from the tray menu via **Settings**. Changes take effect immediately when saved.

| Setting | Description |
|---------|-------------|
| **Mode** | Select a mode (see [Modes](#modes)). |
| **Interval** | How often the engine acts (10–300 seconds). Default: 30 seconds. |
| **App Mode** | Reserved for future use. |
| **Autostart** | Launch Stay Awake automatically when you log in. |
| **Language** | Language preference (currently English only). |
| **Hotkey** | Shows the current global shortcut. |

Click **Save** to apply and close, or **Cancel** to discard changes.

---

## Global Shortcut

The default shortcut is:

| Platform | Shortcut |
|----------|----------|
| macOS | `Cmd + Shift + J` |
| Windows | `Ctrl + Shift + J` |

Pressing the shortcut toggles the engine on/off from anywhere, without opening
the tray menu.

---

## Idle Detection

When a mouse-based mode is active, Stay Awake checks if you have moved the cursor
since the last cycle. If you are actively using the mouse, it **skips** the
simulated movement to avoid interfering with your work.

---

## macOS Accessibility Permission

Mouse-based modes (Subtle, Zen, Circle) require the **Accessibility** permission
on macOS. This is an Apple requirement for any app that generates synthetic input
events.

To grant the permission:

1. Click **Grant Accessibility** in the tray menu (or the banner in Settings).
2. System Settings will open to **Privacy & Security > Accessibility**.
3. Enable **Stay Awake** in the list.
4. Return to Stay Awake — the status will update to "Accessibility OK".

The **Power Only** mode does **not** need this permission.

---

## Configuration File

Stay Awake stores its settings as a JSON file in your system's config directory:

| Platform | Path |
|----------|------|
| macOS | `~/Library/Application Support/stay-awake/config.json` |
| Windows | `%APPDATA%\stay-awake\config.json` |

You can edit this file directly if needed. Changes take effect on the next launch.

---

## FAQ

**Does Stay Awake appear in the Dock (macOS)?**
No. Stay Awake is a menu-bar-only utility and does not show a Dock icon.

**Does Stay Awake require admin or elevated privileges?**
No. All mechanisms work with standard user permissions. On macOS, Accessibility
is the only optional permission, and only for mouse-based modes.

**Will this keep my screen on?**
Yes. Both the power assertion mechanism and mouse movement prevent the display
from dimming or turning off.

**Can I change the keyboard shortcut?**
The shortcut is stored in the config file as `global_hotkey`. Edit it to any
combination like `"CmdOrCtrl+Shift+K"` and restart the app.
