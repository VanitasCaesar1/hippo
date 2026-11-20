#!/bin/sh
# Builds Hippo.app for a release and publishes nothing.
#
#   app/scripts/release-app.sh <version>
#
# Builds hippo-app for aarch64-apple-darwin and x86_64-apple-darwin (or
# only those in HIPPO_APP_TARGETS; `native` builds for this Mac alone,
# reusing an existing release build), joins them with lipo, bundles the
# result as bundle.sh does (Info.plist with the version, the icon), signs
# it ad hoc (codesign --sign -: no identity, no certificate), and writes
# Hippo-<version>-macos.tar.gz, holding one Hippo.app folder, and its
# .sha256 into $HIPPO_DIST_DIR, else <repository>/../hippo-dist/<version>.
# Not notarised: docs/designs/app-download.md says why it still opens when
# `hippo app install` fetches it.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
app=$(cd "$here/.." && pwd)
root=$(cd "$app/.." && pwd)
die() { echo "release-app.sh: $*" >&2; exit 1; }

[ $# -eq 1 ] || die "usage: app/scripts/release-app.sh <version>"
version=$1
[ "$(uname -s)" = Darwin ] || die "Hippo.app builds on macOS only"
have=$(sed -n 's/^version = "\(.*\)"/\1/p' "$app/hippo-app/Cargo.toml" | head -n 1)
[ "$version" = "$have" ] || die "version $version is not the app's ($have in app/hippo-app/Cargo.toml)"

targets=${HIPPO_APP_TARGETS:-aarch64-apple-darwin x86_64-apple-darwin}
target_dir=${CARGO_TARGET_DIR:-$app/target}
dist=${HIPPO_DIST_DIR:-$root/../hippo-dist/$version}
mkdir -p "$dist"
dist=$(cd "$dist" && pwd)
case $dist/ in "$root"/*) die "the dist folder $dist is inside the repository" ;; esac

# As release.sh: no symbols, and no build machine paths in the binary.
CARGO_PROFILE_RELEASE_STRIP=symbols
RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=/home --remap-path-prefix=${RUSTUP_HOME:-$HOME/.rustup}=/rustup --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo --remap-path-prefix=$root=/hippo"
export CARGO_PROFILE_RELEASE_STRIP RUSTFLAGS

bins=
for t in $targets; do
  if [ "$t" = native ]; then
    (cd "$app" && cargo ${TOOLCHAIN:-} build --release --locked -p hippo-app)
    bin=$target_dir/release/hippo-app
  else
    (cd "$app" && cargo ${TOOLCHAIN:-} build --release --locked -p hippo-app --target "$t")
    bin=$target_dir/$t/release/hippo-app
  fi
  [ -x "$bin" ] || die "no binary at $bin"
  bins="$bins $bin"
done

stage=$(mktemp -d "${TMPDIR:-/tmp}/hippo-app-release.XXXXXX")
trap 'rm -rf "$stage"' EXIT INT TERM
bundle=$stage/Hippo.app
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources"
# shellcheck disable=SC2086
lipo -create -output "$bundle/Contents/MacOS/hippo-app" $bins
sed "s/@VERSION@/$version/g" "$app/resources/Info.plist" >"$bundle/Contents/Info.plist"
cp "$app/resources/Hippo.icns" "$bundle/Contents/Resources/Hippo.icns"
codesign --force --sign - "$bundle"
codesign --verify --strict "$bundle"

name=Hippo-$version-macos.tar.gz
# COPYFILE_DISABLE keeps macOS's tar from adding ._ files.
(cd "$stage" && COPYFILE_DISABLE=1 tar -czf "$dist/$name" Hippo.app)
(cd "$dist" && shasum -a 256 "$name" >"$name.sha256")
echo "wrote $dist/$name ($(lipo -archs "$bundle/Contents/MacOS/hippo-app"))"
