#!/usr/bin/env bash
# Screenshot / input helper for the headless test instance (scripts/test-instance.sh xvfb).
# Usage:
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
win() { "$XDO" search --name '^Celeste' | head -1; }
mkdir -p "$REPO/temp"
case "$1" in
  shot) "$(pkg imagemagick)/bin/import" -window "$(win)" "$REPO/temp/${2:-shot}.png"; echo "$REPO/temp/${2:-shot}.png" ;;
  click) "$XDO" mousemove --window "$(win)" "$2" "$3" click 1 ;;
  type) "$XDO" type --delay 20 "$2" ;;
  key) "$XDO" key "$2" ;;
  resize) "$XDO" windowsize "$(win)" "$2" "$3" ;;
esac
