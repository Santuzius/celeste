#!/usr/bin/env bash
# Renders Android's launcher icons from assets/syncative-icon.svg: the adaptive icon's foreground (the cloud on a transparent 108 dp square, sized to Android's 66 dp safe zone, also used as the monochrome layer for themed icons) and the plain icon for launchers without adaptive icons.
set -euo pipefail
REPO=$(cd "$(dirname "$0")/.." && pwd)
RES=$REPO/android/app/src/main/res
SVG=$REPO/assets/syncative-icon.svg
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# The cloud's centre in the icon's coordinates and the scale that keeps its farthest edge (the lower side lobes and their shadow, 60.8 units from the centre) within the safe zone's 33 dp radius.
python3 - "$SVG" "$tmp/foreground.svg" <<'EOF'
import sys
svg = open(sys.argv[1]).read()
inner = svg[svg.index(">", svg.index("<svg")) + 1:svg.rindex("</svg>")]
open(sys.argv[2], "w").write(
    '<svg xmlns="http://www.w3.org/2000/svg" width="108" height="108" viewBox="0 0 108 108">'
    '<g transform="translate(54 54) scale(0.55) translate(-64 -61.5)">' + inner + "</g></svg>\n"
)
EOF

render() { nix shell nixpkgs#librsvg -c rsvg-convert -w "$2" -h "$2" "$1" -o "$3"; }
for density in mdpi:1 hdpi:1.5 xhdpi:2 xxhdpi:3 xxxhdpi:4; do
  name=${density%%:*}; factor=${density#*:}
  dir=$RES/mipmap-$name
  render "$tmp/foreground.svg" "$(python3 -c "print(round(108*$factor))")" "$dir/ic_launcher_foreground.png"
  render "$SVG" "$(python3 -c "print(round(48*$factor))")" "$dir/ic_launcher.png"
done
ls -l "$RES"/mipmap-*/ic_launcher*.png
