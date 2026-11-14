#!/usr/bin/env bash
# Installs the sd-import-rust command and app-menu entry (plus a device-notifier action on KDE Plasma).
# Works from a source checkout (builds with cargo) or from an unpacked release tarball (prebuilt binary).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"

if [[ -x "$here/sd-import-rust" && ! -f "$here/Cargo.toml" ]]; then
  bin="$here/sd-import-rust"                      # release tarball
elif command -v cargo >/dev/null; then
  (cd "$here" && cargo build --release --locked)
  bin="$here/target/release/sd-import-rust"
else
  echo "cargo not found. Install Rust (https://rustup.rs) or download a release binary from" >&2
  echo "https://github.com/Krabisko/sd-import-rust/releases" >&2
  exit 1
fi
command -v lsblk >/dev/null || echo "Note: lsblk not found, cards won't be detected automatically."

mkdir -p ~/.local/bin ~/.local/share/applications
install -m 755 "$bin" ~/.local/bin/sd-import-rust
cp "$here/sd-import-rust.desktop" ~/.local/share/applications/

# KDE Plasma shows these actions when a card is inserted; other desktops ignore them.
if command -v plasmashell >/dev/null || [[ -d ~/.local/share/solid/actions ]]; then
  mkdir -p ~/.local/share/solid/actions
  cp "$here/sd-import-rust-solid.desktop" ~/.local/share/solid/actions/sd-import-rust.desktop
fi

case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) echo "Note: add ~/.local/bin to your PATH to run 'sd-import-rust' from a terminal." ;;
esac
echo "Installed. Open 'SD Import (Rust)' from the app menu or run 'sd-import-rust'."
echo "The first launch asks for your terminal, file manager and library folder."
