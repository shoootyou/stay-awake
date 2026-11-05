# Stay Awake user guide

Stay Awake prevents your computer from sleeping or going idle on macOS and Linux.
This guide is written around macOS. See [Linux](#linux) for the Linux-specific notes.

## Linux

- The default hotkey is `Ctrl+Shift+J`.
- Power Only is the default mode and uses `systemd-inhibit`.
- Mouse Subtle, Mouse Zen, and Mouse Circle need `xdotool`.
- The tray icon requires the `libayatana-appindicator3-1` library. The `.deb` installs it;
  source builds need `libayatana-appindicator3-dev`.
- WiFi app mode uses NetworkManager.
- Read the current state from the tray menu text. Tooltips are not guaranteed by every
  Linux tray implementation.

---

## macOS requirements

macOS 12 Monterey or later on Apple Silicon.

---

## Getting Started

### First launch

After installation, launch Stay Awake from your Applications folder. The app icon
appears in the menu bar immediately. On first launch macOS may show a security
prompt — open **System Settings → Privacy & Security** and click **Open Anyway**.

### Tray icon

Click the menu bar icon to open the tray menu. Left-click and right-click both
open the same menu.

The icon shape shows the current state. A check circle means active, a power
ring means inactive, and a calendar means waiting for a schedule. Active manual
overrides use the check circle. A paused window uses the power ring.

On macOS, the template images follow the light or dark menu bar. Linux and
Windows use explicit state images. The tray menu text always names the state.
On Linux, use that menu text because tooltips and native tray titles are not
guaranteed by every implementation.

### Permissions overview

| Feature | Permission needed |
|---------|-------------------|
| Power Only mode | None |
| Mouse modes (Subtle, Zen, Circle) | Accessibility |
| WiFi activation mode | Location Services (macOS 26+) |
| Launch at Login | None (registered as a Login Item) |

---

## Tray Menu

### Status item

The status item shows the current engine and schedule state. Click it to toggle the engine.

- **Active**: the engine is running. Click to stop it.
- **Inactive**: the engine is stopped. Click to start it.
- **Waiting for schedule**: a checked schedule is outside its window. Click to start it
  until the next window opens.
- **Paused for this window**: you stopped the engine during the current window. Click
  to start it again.
- **Active (manual override)**: you started the engine outside a scheduled window.
  Click to stop it.

You can also toggle the engine with the [global hotkey](#global-hotkey) (default: `⌘+Shift+J`).

### Mode

Shows the current keep-awake method and lets you switch between methods.
See [Modes](#modes) for descriptions of each.

### Accessibility

Shows the macOS Accessibility permission status. Only relevant for mouse-based modes.

- **Accessibility OK** — permission granted; mouse modes will work.
- **Grant Accessibility** — click to open System Settings and grant the permission.

### Settings

Opens the [Settings window](#settings-window).

### Quit

Stops the engine and exits Stay Awake.

---

## Modes

Stay Awake has two kinds of modes:

- **Keep-awake methods** — *how* the engine prevents sleep (Power Only, Mouse Subtle,
  Mouse Zen, Mouse Circle). Configured via the **Mode** menu item or **Settings → Jiggle Mode**.
- **App Mode** — *when* the engine activates (Manual, Always On, or WiFi). Configured
  in **Settings → App Mode**.

### Power Only

Uses a macOS IOKit power assertion (`IOPMAssertionCreateWithName`) to prevent the
system from sleeping. No mouse movement is simulated. No special permissions are needed.
The display stays on and the screensaver is suppressed for as long as the engine is active.

**Best for:** most users who want to prevent sleep without any visible side effects.

### Mouse Subtle

Moves the cursor 1 pixel to the right, then 1 pixel back. The nudge is imperceptible
in normal use but resets the OS idle timer.

Requires **Accessibility** permission.

**Best for:** apps that watch mouse movement (not just power assertions) to detect
user activity.

### Mouse Zen

Fires a zero-delta mouse event — a synthetic movement of 0 pixels. The OS treats
it as real input and the idle timer resets, but the cursor does not move at all.

Requires **Accessibility** permission.

**Best for:** presentations or screen-sharing sessions where even a 1-pixel jump
would be visible.

### Mouse Circle

Traces a small square pattern — right, down, left, up (1 pixel per step) — returning
the cursor to its original position after each cycle.

Requires **Accessibility** permission.

**Best for:** applications that require sustained, varied mouse activity rather than
a single synthetic event.

### WiFi (App Mode)

WiFi mode changes *when* the engine activates, not how it keeps the system awake.
When **App Mode** is set to **WiFi** in Settings, Stay Awake monitors your network
connection and automatically starts the anti-inactivity engine whenever you join a
registered network — and stops it when you disconnect or join an unregistered one.

**How it works:** a background monitor watches for AirPort interface state changes
via macOS `SCDynamicStore` (event-driven, with a periodic 5-second poll as a safety
net). On macOS 26+, SSID detection uses CoreWLAN via Location Services. On
macOS 12–25 it falls back to the `networksetup` CLI, which requires no location
permission.

**Setting it up:**

1. Open **Settings** and set **App Mode** to **WiFi**.
2. Grant **Location Services** when prompted (required on macOS 26 and later).
3. While connected to the network you want to register, click **Register this network**.
4. The engine will now start automatically whenever that network is detected.

You can register multiple networks. Remove one with the **×** button next to its name.

**Caveats:**

- WiFi mode does **not** toggle or interfere with your WiFi connection in any way —
  it only reads the current SSID.
- The keep-awake method (Power Only, Mouse Subtle, etc.) still applies once the engine
  activates; configure it separately via **Mode** in the tray menu.
- On macOS 26+, if Location Services is denied, SSID detection will not work. Enable
  it in **System Settings → Privacy & Security → Location Services → Stay Awake**.

---

## Settings Window

Open from the tray menu via **Settings**. Changes save automatically as you make them.
Settings use icons instead of emoji prefixes.

### General

| Setting | Description |
|---------|-------------|
| **App Mode** | When the engine activates: Manual, Always On, or WiFi. First Settings control. |
| **Jiggle Mode** | How the engine keeps the computer awake (see [Modes](#modes)). |
| **Interval** | How often the engine acts (10–300 s). Default: 30 s. |
| **Launch at Login** | On macOS, start Stay Awake automatically at login as a Login Item. |
| **Language** | UI display language (8 languages available). |
| **Hotkey** | Displays the current global shortcut. Click **Record** to change it. |

### Scheduling

Enable a time window and active days for automatic keep-awake. See [Scheduling](#scheduling).

### Profiles

Save and load named settings snapshots. See [Profiles](#profiles).

### WiFi networks

Visible only when **App Mode** is set to **WiFi**. Lists registered SSIDs and
shows the currently connected network. See [WiFi App Mode](#wifi-app-mode) above.

---

## Profiles

Profiles let you save your current settings as a named snapshot and switch between
configurations instantly — for example, a "Work" profile with scheduling enabled and
a "Presentation" profile with Mouse Zen and a short interval.

### Creating a profile

1. Configure Stay Awake the way you want (mode, interval, schedule, etc.).
2. Open **Settings** and scroll to **Profiles**.
3. Click **Save As** and enter a name (e.g. `Work`, `Presentation`, `Home`).

### Switching profiles

Select a profile from the **Profiles** dropdown. Settings reload immediately and
the selection persists across app restarts.

### Deleting a profile

Select the profile in the dropdown and click **Delete**. The **Default** profile
cannot be deleted.

**What profiles store:** jiggle mode, interval, and all schedule settings (enabled
state, start/end time, active days). App Mode, language, and hotkey are global and
are not saved per profile.

---

## Scheduling

When **Enable schedule** is checked, Manual and Always On start the engine when a
scheduled window opens and stop it when the window closes. Stopping releases the
sleep inhibitor. Outside a window, the engine waits for the next one.

WiFi keeps its registered-network policy. With a checked schedule, WiFi can keep the
engine and sleep inhibitor active on a registered network, but it only sends mouse
jiggles during a scheduled window. Power Only follows the Manual and Always On
lifecycle without sending mouse input.

### Configuring a schedule

1. Open **Settings** and enable **Schedule**.
2. Set **Start time** and **End time** (24-hour format, e.g. `09:00`–`17:00`).
3. Check the days the schedule should apply (Mon–Sun). Weekdays are checked by default.

### Overnight spans

If the end time is earlier than the start time (e.g. `22:00`–`06:00`), the schedule
wraps midnight. An overnight window belongs to the day on which it starts. For
example, a Monday `22:00`–`06:00` window includes early Tuesday, even when Tuesday
is not selected.

Windows include their start time and exclude their end time. No days selected, or
matching start and end times, means there are no automatic windows.

### Manual changes during a schedule

If you stop the engine during a scheduled window, Stay Awake pauses that window. It
starts normally at the next window. If you start the engine outside a scheduled
window, it runs until the next window starts. These manual overrides are not saved
and are cleared when you quit or change the schedule policy.

### Sleep, time changes, and daylight saving time

Stay Awake evaluates the schedule against current local time. After the computer
resumes, or local time or its timezone changes, it applies the current window rather
than replaying missed transitions. It cannot wake the computer for a scheduled
window.

### Upgrade note

When **Enable schedule** is checked in Manual or Always On, Stay Awake now starts
automatically when a scheduled window opens and stops when it closes, releasing its
sleep inhibitor. Previously, the checkbox only filtered mouse jiggles. WiFi mode
keeps that previous filtering behavior. Existing Manual users with the checkbox
already enabled will get automatic starts and stops after upgrading.

---

## Global Hotkey

The default hotkey is `⌘+Shift+J`. Press it from anywhere to toggle the engine
on or off without opening the tray menu.

### Changing the hotkey

1. Open **Settings** and scroll to **Hotkey**.
2. Click **Record**, then press your desired key combination (at least one modifier
   key — ⌘, ⌃, ⌥, or ⇧ — is required).
3. The new shortcut takes effect immediately and is saved.

---

## Updates

**Homebrew install:** run `brew upgrade shoootyou/tap/stay-awake`.

**Manual DMG install:** macOS releases publish signed update metadata. To update, install the
new DMG.

**Linux `.deb`:** Download each new `.deb` from GitHub Releases and install it with
`sudo apt install ./<downloaded-file>.deb`. Linux `.deb` installs do not use the
in-app updater.

---

## Troubleshooting

### Accessibility permission not sticking

1. Open **System Settings → Privacy & Security → Accessibility**.
2. Remove Stay Awake from the list, then re-add it.
3. Restart Stay Awake.

If the banner in Settings still shows after granting permission, click **Recheck** —
the app polls automatically for up to 30 seconds after you click **Grant Accessibility**.

### Screen still dims with Power Only mode

Power Only prevents system sleep and screensaver activation, but it does not override
the **display sleep** timeout set in **System Settings → Displays → Advanced** (or
**Battery**). If the screen still dims, increase or disable that display sleep setting
in macOS.

### Mouse mode cursor isn't moving

Confirm Accessibility is granted: tray menu should show **Accessibility OK**. If it
shows **Grant Accessibility**, follow the steps in the tray or Settings banner.

### Engine is active but ignoring the schedule

- Confirm the **Schedule** checkbox is enabled in Settings.
- Verify the correct days and the start/end times are set.
- Overnight schedules (e.g. `22:00`–`06:00`) are supported — the end time must be
  intentionally earlier than the start time.
- In Manual or Always On, the engine stops outside a checked schedule. The tray shows
  **Waiting for schedule** until the next window.

### WiFi mode not activating

- Confirm the current network SSID appears in the registered networks list in Settings.
- Check Location Services: **System Settings → Privacy & Security → Location Services →
  Stay Awake** must be enabled (required on macOS 26+).
- If not on macOS 26+, SSID detection uses `networksetup` and no location permission
  is needed — verify Stay Awake is not blocked by a firewall or corporate MDM policy.

---

## Internationalization

Stay Awake is available in **8 languages**: English, Spanish, French, German,
Portuguese (Brazil), Japanese, Chinese (Simplified), and Korean.

Change the language in **Settings → Language**. The UI updates immediately — no
restart required.

---

## FAQ

**Does Stay Awake appear in the Dock?**
No. Stay Awake is a menu bar-only utility and does not show a Dock icon.

**Does Stay Awake require admin or elevated privileges?**
No. All keep-awake mechanisms work with standard user permissions. Accessibility
(mouse modes) and Location Services (WiFi mode on macOS 26+) are the only optional
permissions, and both are user-level grants.

**Will this keep my screen on?**
Yes. Power assertions and mouse movement both prevent the display from dimming or
turning off during normal inactivity.

**Can I use a schedule and WiFi mode together?**
Yes. When App Mode is WiFi, the engine activates on registered networks and then
uses the schedule only to filter mouse jiggles. The engine and its sleep inhibitor
remain controlled by the registered-network policy.
