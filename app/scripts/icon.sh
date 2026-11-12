#!/bin/sh
# Makes app/resources/Hippocamp.icns from brand/hippocamp-icon.svg. Run once; the
# output is committed, so building the app needs no image tools.
#
#   app/scripts/icon.sh
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
svg="$root/brand/hippocamp-icon.svg"
out="$root/app/resources/Hippocamp.icns"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
set_dir="$work/Hippocamp.iconset"
mkdir "$set_dir"

for points in 16 32 128 256 512; do
    swift "$here/render-icon.swift" "$svg" "$set_dir/icon_${points}x${points}.png" "$points"
    swift "$here/render-icon.swift" "$svg" "$set_dir/icon_${points}x${points}@2x.png" "$((points * 2))"
done

iconutil -c icns "$set_dir" -o "$out"
echo "wrote $out"
