# SD Import (Rust)

A Rust rewrite of [sd-import](https://github.com/Krabisko/sd-import). It imports photos, videos
and other files from SD cards (drones, photo and video cameras) into a photo/video library,
sorted by category, year and date. It's one static binary, needs no Python, and works on any
Linux desktop (GNOME, KDE Plasma, COSMIC, XFCE, Cinnamon, MATE, LXQt, Hyprland, Sway, …).

```
<library>/<category>/by year/<YYYY>/<YYYY-MM-DD[ name]>/
    Photo/RAW, Photo/JPEG   (split only when the card has both formats)
    Photo/                  (otherwise)
    Video/
    Other/                  (drone telemetry, proxies, unknown files)
```

- Asks which category to import into, plus an optional shoot, client or event name
- Reuses existing year folders and an existing `by year` or `yearly` folder, never duplicates them
- Never deletes anything from the card. Re-runs skip files that were already imported (same size,
  plus identical first and last MB)
- Name clashes get a `_1` suffix instead of overwriting
- Copies through a `.part` file, flushed to disk and size-checked before it's renamed. Keeps file times
- Checks free space, shows progress, notifies when done, and offers to eject the card and open the folder

It behaves exactly like the Python version and **shares its config file**, so both can be installed
side by side and you only set things up once.

## Install

**Prebuilt binary** (x86_64, static, no dependencies): download the latest
`sd-import-rust-*-x86_64-linux.tar.gz` from
[Releases](https://github.com/Krabisko/sd-import-rust/releases), unpack it and run:

```bash
./install.sh
```

**From source** (needs Rust 1.80 or newer):

```bash
git clone https://github.com/Krabisko/sd-import-rust.git
cd sd-import-rust
./install.sh
```

This installs `sd-import-rust` into `~/.local/bin` and adds an **SD Import (Rust)** app-menu entry.
On KDE Plasma it also adds an action to the device notifier that appears when a card is inserted.

You need `lsblk` (part of util-linux) to detect cards. `udisksctl` (eject) and `notify-send`
(notifications) are optional.

## First launch

The first time it runs, SD Import asks:

1. **Terminal**: the app opens itself in this terminal when started from the app menu.
2. **File manager**: used for "Open the import folder".
3. **Library folder**: where imports go, e.g. `~/Pictures` or `/mnt/data/Photo & Video`.
4. **Categories**: defaults to `01_personal`, `02_clients`, `03_events`. Existing folders
   are recognised even with a different number or case (`Personal` matches `01_personal`).
   You can add, rename or remove categories.

Each category gets a `by year` folder. If it already has `yearly` or `by year`, that one is used.
Year folders sitting directly in a category (e.g. `02_clients/2026`) are offered for moving
into `by year/`. That's a rename on the same drive, and nothing moves unless you say yes.

Settings are stored in `~/.config/sd-import/config.json`. Run `sd-import-rust --setup` to change them.

Terminals offered when installed: COSMIC Terminal, GNOME Terminal, Ptyxis, GNOME Console,
Konsole, Xfce Terminal, MATE Terminal, Tilix, Terminator, LXTerminal, QTerminal, Alacritty,
kitty, foot, WezTerm, Ghostty, xterm, `xdg-terminal-exec`, or any custom command
(`{cmd}` marks where the command goes).

File managers: system default (`xdg-open`), COSMIC Files, Nautilus, Dolphin, Nemo, Thunar,
Caja, PCManFM, PCManFM-Qt, or a custom command (`{path}` marks the folder).

## Usage

```bash
sd-import-rust                     # auto-detect inserted cards
sd-import-rust /run/media/$USER/X  # import a specific folder
sd-import-rust --dry-run           # show the plan, copy nothing
sd-import-rust --date 2026-11-09   # override the import date
sd-import-rust --setup             # change terminal, file manager, library, categories
```

`SD_IMPORT_CONFIG=/path/config.json` uses a different config file, which is handy for testing.
`--no-terminal` stops it from reopening in a terminal window when answers are piped in.

## Development

```bash
cargo test
cargo build --release   # target/release/sd-import-rust
```

The code is split into `src/ui.rs` (prompts), `desktop.rs` (terminals, file managers),
`config.rs` (config and setup), `cards.rs` (lsblk and eject) and `import.rs` (scan and copy).
Pushing a `v*` tag builds a static musl binary and publishes it as a GitHub Release.
