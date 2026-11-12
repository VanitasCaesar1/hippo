#!/bin/sh
# Builds the release binary and assembles Hippocamp.app, with no bundling tool.
#
#   CARGO_TARGET_DIR=/path/to/target app/scripts/bundle.sh
#
# The bundle is written to ${CARGO_TARGET_DIR:-app/target}/Hippocamp.app and signed
# ad hoc (an Apple-silicon Mac runs only signed code). The script prints its path.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
app="$here/.."
target="${CARGO_TARGET_DIR:-$app/target}"

# Stable Rust must be no older than the pinned Zed commit's toolchain.
# TOOLCHAIN=+1.99.1 picks one when the default is older.
(cd "$app" && cargo ${TOOLCHAIN:-} build --release --locked)

version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$app/hippocamp-app/Cargo.toml" | head -n 1)
[ -n "$version" ] || { echo "no version in hippocamp-app/Cargo.toml" >&2; exit 1; }

bundle="$target/Hippocamp.app"
rm -rf "$bundle"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
sed "s/@VERSION@/$version/g" "$app/resources/Info.plist" > "$bundle/Contents/Info.plist"
cp "$target/release/hippocamp-app" "$bundle/Contents/MacOS/hippocamp-app"
cp "$app/resources/Hippocamp.icns" "$bundle/Contents/Resources/Hippocamp.icns"

codesign --force --sign - "$bundle"
echo "$bundle"
