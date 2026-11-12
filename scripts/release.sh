#!/bin/sh
# Builds hippo's release binaries and publishes nothing.
#
#   scripts/release.sh <version> [target]…
#   scripts/release.sh <version> --no-build    # only check and fill the formula
#
# Targets: aarch64-apple-darwin and x86_64-apple-darwin (built on macOS with
# cargo), x86_64-unknown-linux-musl and aarch64-unknown-linux-musl (static,
# built with cargo zigbuild, which needs zig on PATH). All four when none is
# named. For each target it writes hippo-<version>-<target>.tar.gz (a
# folder of that name holding hippo, LICENSE, README.md and
# LICENSE-DejaVu.txt, the licence of the card's embedded font) and a .sha256
# beside it, into $HIPPO_DIST_DIR, else <repository>/../hippo-dist/<version>.
# When all four tarballs are there, it fills the Homebrew formula
# (packaging/homebrew/hippo.rb.in) into the same folder.

set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
all="aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-musl aarch64-unknown-linux-musl"
repo=https://github.com/VanitasCaesar1/hippo

die() {
  echo "release.sh: $*" >&2
  exit 1
}

[ $# -ge 1 ] || die "usage: scripts/release.sh <version> [target]… | <version> --no-build"
version=$1
shift
build=yes
if [ "${1:-}" = --no-build ]; then
  build=no
  shift
fi
targets=${*:-$all}

# The version must be the workspace's, and the plugin's when there is one.
workspace=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml")
[ "$version" = "$workspace" ] || die "version $version is not the workspace's ($workspace in Cargo.toml)"
plugin=$root/plugin/.claude-plugin/plugin.json
if [ -f "$plugin" ]; then
  pv=$(sed -n 's/^ *"version": *"\(.*\)",*$/\1/p' "$plugin")
  [ "$version" = "$pv" ] || die "version $version is not the plugin's ($pv in plugin/.claude-plugin/plugin.json)"
fi
dsh_plugin=$root/plugins/dsh/package.json
if [ -f "$dsh_plugin" ]; then
  dv=$(sed -n 's/^ *"version": *"\(.*\)",*$/\1/p' "$dsh_plugin")
  [ "$version" = "$dv" ] || die "version $version is not the dsh plugin's ($dv in plugins/dsh/package.json)"
fi
opencode_plugin=$root/plugins/opencode/package.json
if [ -f "$opencode_plugin" ]; then
  ov=$(sed -n 's/^ *"version": *"\(.*\)",*$/\1/p' "$opencode_plugin")
  [ "$version" = "$ov" ] || die "version $version is not the OpenCode plugin's ($ov in plugins/opencode/package.json)"
fi

dist=${HIPPO_DIST_DIR:-$root/../hippo-dist/$version}
mkdir -p "$dist"
dist=$(cd "$dist" && pwd)
case $dist/ in "$root"/*) die "the dist folder $dist is inside the repository" ;; esac

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1"
  else
    shasum -a 256 "$1"
  fi
}

if [ "$build" = yes ]; then
  target_dir=${CARGO_TARGET_DIR:-$root/target}
  # Strip symbols from the release binary; nothing else about the profile changes.
  CARGO_PROFILE_RELEASE_STRIP=symbols
  export CARGO_PROFILE_RELEASE_STRIP
  # Keep the build machine's paths out of the binary (panic messages carry
  # source paths). With several matches rustc uses the last, so the most
  # specific prefix comes last. Paths with spaces are not supported here.
  RUSTFLAGS="${RUSTFLAGS:-} --remap-path-prefix=$HOME=/home --remap-path-prefix=${RUSTUP_HOME:-$HOME/.rustup}=/rustup --remap-path-prefix=${CARGO_HOME:-$HOME/.cargo}=/cargo --remap-path-prefix=$root=/hippo"
  export RUSTFLAGS
  for t in $targets; do
    case " $all " in *" $t "*) ;; *) die "unknown target $t (one of: $all)" ;; esac
    case $t in
      *-apple-darwin)
        [ "$(uname -s)" = Darwin ] || die "$t builds on macOS only"
        (cd "$root" && cargo build --release --locked -p hippo --target "$t")
        ;;
      *-linux-musl)
        command -v cargo-zigbuild >/dev/null 2>&1 || die "$t needs cargo-zigbuild (cargo install --locked cargo-zigbuild)"
        command -v zig >/dev/null 2>&1 || die "$t needs zig on PATH (https://ziglang.org/download/)"
        (cd "$root" && cargo zigbuild --release --locked -p hippo --target "$t")
        ;;
    esac
    bin=$target_dir/$t/release/hippo
    [ -x "$bin" ] || die "no binary at $bin"
    name=hippo-$version-$t
    stage=$(mktemp -d "${TMPDIR:-/tmp}/hippo-release.XXXXXX")
    mkdir "$stage/$name"
    cp "$bin" "$stage/$name/hippo"
    cp "$root/LICENSE" "$root/README.md" "$root/crates/hippo/assets/fonts/LICENSE-DejaVu.txt" "$stage/$name/"
    # COPYFILE_DISABLE keeps macOS's tar from adding ._ files.
    (cd "$stage" && COPYFILE_DISABLE=1 tar -czf "$dist/$name.tar.gz" "$name")
    rm -rf "$stage"
    (cd "$dist" && sha256 "$name.tar.gz" >"$name.tar.gz.sha256")
    echo "wrote $dist/$name.tar.gz ($(wc -c <"$bin" | tr -d ' ') bytes unpacked)"
  done
fi

# The Homebrew formula, once all four are there.
missing=
for t in $all; do
  [ -f "$dist/hippo-$version-$t.tar.gz.sha256" ] || missing="$missing $t"
done
if [ -n "$missing" ]; then
  echo "formula not written: no tarball yet for$missing"
  exit 0
fi
script=
for t in $all; do
  hash=$(cut -d ' ' -f 1 "$dist/hippo-$version-$t.tar.gz.sha256")
  url=$repo/releases/download/v$version/hippo-$version-$t.tar.gz
  script="$script s|@URL_$t@|$url|; s|@SHA_$t@|$hash|;"
done
sed -e "s|@VERSION@|$version|g;$script" "$root/packaging/homebrew/hippo.rb.in" >"$dist/hippo.rb"
if grep -q '@[A-Z_]*[a-z0-9_-]*@' "$dist/hippo.rb"; then
  die "a placeholder is left in $dist/hippo.rb"
fi
echo "wrote $dist/hippo.rb"
