#!/usr/bin/env bash
# Screenshot / input helper for the headless test instance (scripts/test-instance.sh xvfb).
# Usage:
#   scripts/test-screenshot.sh wait               → block until the window exists (≤ 60 s)
#   scripts/test-screenshot.sh shot [name]        → temp/<name>.png (window only)
#   scripts/test-screenshot.sh click X Y          → left click at window coordinates
#   scripts/test-screenshot.sh type "text"        → type into the focused field
#   scripts/test-screenshot.sh key Return         → send a key
#   scripts/test-screenshot.sh resize W H         → resize the Celeste window
set -euo pipefail
REPO="$(cd "$(dirname "$0")/.." && pwd)"
export DISPLAY=:99
pkg() { nix-build '<nixpkgs>' -A "$1" --no-out-link 2>/dev/null; }
XDO="$(pkg xdotool)/bin/xdotool"
win() {
  local id
  id="$("$XDO" search --name '^Celeste' 2>/dev/null | head -1)"
  [ -n "$id" ] || { echo "no Celeste window on $DISPLAY" >&2; exit 1; }
  echo "$id"
}
mkdir -p "$REPO/temp"
case "$1" in
  wait) for _ in $(seq 1 60); do "$XDO" search --name '^Celeste' >/dev/null 2>&1 && exit 0; sleep 1; done; echo "timed out" >&2; exit 1 ;;
  shot) id="$(win)"; "$(pkg imagemagick)/bin/import" -window "$id" "$REPO/temp/${2:-shot}.png"; echo "$REPO/temp/${2:-shot}.png" ;;
  click) id="$(win)"; "$XDO" mousemove --window "$id" "$2" "$3" click 1 ;;
  type) id="$(win)"; "$XDO" windowfocus --sync "$id"; "$XDO" type --delay 20 "$2" ;;
  key) id="$(win)"; "$XDO" windowfocus --sync "$id"; "$XDO" key "$2" ;;
  resize) id="$(win)"; "$XDO" windowsize "$id" "$2" "$3" ;;
esac
