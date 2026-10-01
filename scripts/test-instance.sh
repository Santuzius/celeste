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
#   CELESTE_TEST_BIN=/path/to/celeste runs another build (e.g. an older version for comparisons).
#   CELESTE_TEST_SHOW=0 starts hidden (tray mode) instead of passing --show.
set -euo pipefail

MODE="${1:-xvfb}"
PROFILE=debug
[ "${2:-}" = "--release" ] && PROFILE=release
REPO="$(cd "$(dirname "$0")/.." && pwd)"
T="${CELESTE_TEST_DIR:-/tmp/celeste-test}"
BIN="${CELESTE_TEST_BIN:-$REPO/target/$PROFILE/celeste}"
SHOW_ARG=--show
[ "${CELESTE_TEST_SHOW:-1}" = 0 ] && SHOW_ARG=

if [ -z "${IN_NIX_SHELL:-}" ]; then
  exec nix-shell "$REPO/shell.nix" --run "$(printf '%q ' "$0" "$@")"
fi

[ -x "$BIN" ] || { echo "missing $BIN — build first"; exit 1; }
pkg() { nix-build '<nixpkgs>' -A "$1" --no-out-link 2>/dev/null; }
GKR="$(pkg gnome-keyring)/bin/gnome-keyring-daemon"

# A previous run's processes (Celeste itself, services D-Bus-activated on its private bus) can outlive it; stop the ones whose HOME is our test home — never anything of the real session — and wait until they are gone.
test_pids() {
  for pid in $(pgrep -u "$USER"); do
    if { tr '\0' '\n' < "/proc/$pid/environ"; } 2>/dev/null | grep -qx "HOME=$T/home"; then echo "$pid"; fi
  done
}
pids="$(test_pids)"
if [ -n "$pids" ]; then
  kill $pids 2>/dev/null || true
  for _ in $(seq 1 50); do [ -z "$(test_pids)" ] && break; sleep 0.1; done
fi
# The X server is started with the caller's environment so the cleanup above never takes it down; it is reused across runs.
if [ "$MODE" = xvfb ]; then
  DISPLAY_TO_USE=:99
  xvfb_running() { for p in $(pgrep -x Xvfb); do grep -qa ":99" "/proc/$p/cmdline" && return 0; done; return 1; }
  if ! xvfb_running; then
    rm -f /tmp/.X11-unix/X99 /tmp/.X99-lock
    "$(pkg xorg.xvfb)/bin/Xvfb" :99 -screen 0 1280x860x24 -nolisten tcp >/tmp/celeste-test-xvfb.log 2>&1 &
    sleep 1
  fi
else
  DISPLAY_TO_USE="${REAL_DISPLAY:-:0}"
fi

[ "${CELESTE_TEST_RESET:-0}" = 1 ] && rm -rf "$T"
mkdir -p "$T/home/.local/share" "$T/home/.config" "$T/home/.cache" "$T/run"
chmod 700 "$T/run"

export HOME="$T/home"
export XDG_DATA_HOME="$T/home/.local/share"
export XDG_CONFIG_HOME="$T/home/.config"
export XDG_CACHE_HOME="$T/home/.cache"
export XDG_RUNTIME_DIR="$T/run"
unset WAYLAND_DISPLAY DBUS_SESSION_BUS_ADDRESS
export DISPLAY="$DISPLAY_TO_USE"

export GKR BIN T SHOW_ARG
exec dbus-run-session -- bash -c '
  set -e
  printf test | "$GKR" --unlock --components=secrets >/dev/null
  sleep 0.5
  pid=$(dbus-send --session --print-reply --dest=org.freedesktop.DBus / org.freedesktop.DBus.GetConnectionUnixProcessID string:org.freedesktop.secrets | awk "/uint32/{print \$2}")
  exe=$(readlink /proc/$pid/exe)
  case "$exe" in /nix/store/*gnome-keyring*) echo "isolated secret service: $exe";; *) echo "ABORT: org.freedesktop.secrets is owned by $exe"; exit 1;; esac
  echo "data dir: $XDG_DATA_HOME/celeste"
  exec "$BIN" $SHOW_ARG
'
