#!/usr/bin/env bash
# Launch a fully isolated Celeste instance for testing with throwaway/mock accounts.
#
# Isolation: own HOME and XDG_* dirs under $CELESTE_TEST_DIR (default /tmp/celeste-test), own D-Bus session bus with its own unlocked gnome-keyring (aborts if anything else owns org.freedesktop.secrets), no Wayland socket. It never reads or writes ~/.local/share/celeste, ~/.config/celeste, the real keyring or $XDG_RUNTIME_DIR/celeste of the real instance.
#
# Usage (from the repo root):
#   scripts/test-instance.sh [xvfb|desktop] [--release]
#     xvfb     headless X server on :99 (screenshots via scripts/test-screenshot.sh)
#     desktop  window on the real desktop via XWayland (e.g. to log in throwaway accounts)
#   CELESTE_TEST_RESET=1 wipes the test state first.
set -euo pipefail

MODE="${1:-xvfb}"
PROFILE=debug
[ "${2:-}" = "--release" ] && PROFILE=release
REPO="$(cd "$(dirname "$0")/.." && pwd)"
T="${CELESTE_TEST_DIR:-/tmp/celeste-test}"
BIN="$REPO/target/$PROFILE/celeste"

if [ -z "${IN_NIX_SHELL:-}" ]; then
  exec nix-shell "$REPO/shell.nix" --run "$(printf '%q ' "$0" "$@")"
fi

[ -x "$BIN" ] || { echo "missing $BIN — build first"; exit 1; }
pkg() { nix-build '<nixpkgs>' -A "$1" --no-out-link 2>/dev/null; }
GKR="$(pkg gnome-keyring)/bin/gnome-keyring-daemon"

[ "${CELESTE_TEST_RESET:-0}" = 1 ] && rm -rf "$T"
mkdir -p "$T/home/.local/share" "$T/home/.config" "$T/home/.cache" "$T/run"
chmod 700 "$T/run"

export HOME="$T/home"
export XDG_DATA_HOME="$T/home/.local/share"
export XDG_CONFIG_HOME="$T/home/.config"
export XDG_CACHE_HOME="$T/home/.cache"
export XDG_RUNTIME_DIR="$T/run"
unset WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS

if [ "$MODE" = xvfb ]; then
  export DISPLAY=:99
  if ! [ -e /tmp/.X11-unix/X99 ]; then
    "$(pkg xorg.xvfb)/bin/Xvfb" :99 -screen 0 1280x860x24 -nolisten tcp >"$T/xvfb.log" 2>&1 &
    sleep 1
  fi
else
  export DISPLAY="${REAL_DISPLAY:-:0}"
fi

export GKR BIN T
exec dbus-run-session -- bash -c '
  set -e
  printf test | "$GKR" --unlock --components=secrets >/dev/null
  sleep 0.5
  pid=$(dbus-send --session --print-reply --dest=org.freedesktop.DBus / org.freedesktop.DBus.GetConnectionUnixProcessID string:org.freedesktop.secrets | awk "/uint32/{print \$2}")
  exe=$(readlink /proc/$pid/exe)
  case "$exe" in /nix/store/*gnome-keyring*) echo "isolated secret service: $exe";; *) echo "ABORT: org.freedesktop.secrets is owned by $exe"; exit 1;; esac
  echo "data dir: $XDG_DATA_HOME/celeste"
  exec "$BIN" --show
'
