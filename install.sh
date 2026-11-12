#!/bin/sh
# Installs hippocamp from the latest GitHub release:
#
#   curl -fsSL https://raw.githubusercontent.com/VanitasCaesar1/hippocamp/main/install.sh | sh
#
# It picks the binary for this machine, checks its sha256, and copies it to
# $HIPPOCAMP_INSTALL_DIR (default ~/.local/bin). No sudo.
#
#   HIPPOCAMP_VERSION       a version to install instead of the latest (0.1.0)
#   HIPPOCAMP_INSTALL_DIR   where the binary goes
#   HIPPOCAMP_BASE_URL      a folder holding the release files, such as
#                           file:///path/to/dist (then give HIPPOCAMP_VERSION)
#   HIPPOCAMP_INSTALL_OS, HIPPOCAMP_INSTALL_ARCH
#                           stand in for uname -s and uname -m, for testing

set -eu

repo=https://github.com/VanitasCaesar1/hippocamp
dir=${HIPPOCAMP_INSTALL_DIR:-$HOME/.local/bin}

say() {
  printf '%s\n' "$*"
}

die() {
  printf 'install.sh: %s\n' "$*" >&2
  exit 1
}

unsupported() {
  die "there is no prebuilt hippocamp for $1 $2. With Rust installed, build it:
  cargo install --locked --git $repo hippocamp"
}

os=${HIPPOCAMP_INSTALL_OS:-$(uname -s)}
arch=${HIPPOCAMP_INSTALL_ARCH:-$(uname -m)}
case $os in
  Darwin)
    case $arch in
      arm64 | aarch64) target=aarch64-apple-darwin ;;
      x86_64)
        # An x86_64 shell under Rosetta on Apple silicon still gets the native binary.
        if [ -z "${HIPPOCAMP_INSTALL_ARCH:-}" ] && [ "$(sysctl -n hw.optional.arm64 2>/dev/null || true)" = 1 ]; then
          target=aarch64-apple-darwin
        else
          target=x86_64-apple-darwin
        fi
        ;;
      *) unsupported "$os" "$arch" ;;
    esac
    ;;
  Linux)
    case $arch in
      x86_64 | amd64) target=x86_64-unknown-linux-musl ;;
      aarch64 | arm64) target=aarch64-unknown-linux-musl ;;
      *) unsupported "$os" "$arch" ;;
    esac
    ;;
  *) unsupported "$os" "$arch" ;;
esac

# fetch <url> <file>
fetch() {
  case $1 in
    file://*) cp "${1#file://}" "$2" ;;
    *)
      if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$1" -o "$2"
      elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$2" "$1"
      else
        die "needs curl or wget to download $1"
      fi
      ;;
  esac
}

# The latest release's version, from the tag GitHub redirects to.
latest() {
  if command -v curl >/dev/null 2>&1; then
    url=$(curl -fsLI -o /dev/null -w '%{url_effective}' "$repo/releases/latest") || return 1
  elif command -v wget >/dev/null 2>&1; then
    url=$(wget -S --spider "$repo/releases/latest" 2>&1 | sed -n 's/^ *[Ll]ocation: *//p' | tail -n 1) || return 1
  else
    die "needs curl or wget"
  fi
  case $url in
    */releases/tag/*) ;;
    *) return 1 ;;
  esac
  tag=${url##*/releases/tag/}
  tag=$(printf '%s' "$tag" | tr -d '\r')
  printf '%s\n' "${tag#v}"
}

version=${HIPPOCAMP_VERSION:-}
if [ -z "$version" ]; then
  [ -z "${HIPPOCAMP_BASE_URL:-}" ] || die "HIPPOCAMP_BASE_URL needs HIPPOCAMP_VERSION too"
  version=$(latest) || die "found no release at $repo/releases"
fi
base=${HIPPOCAMP_BASE_URL:-$repo/releases/download/v$version}
base=${base%/}
asset=hippocamp-$version-$target.tar.gz

tmp=$(mktemp -d "${TMPDIR:-/tmp}/hippocamp-install.XXXXXX")
trap 'rm -rf "$tmp"' EXIT INT TERM

say "Downloading hippocamp $version for $target"
fetch "$base/$asset" "$tmp/$asset" || die "could not download $base/$asset"
fetch "$base/$asset.sha256" "$tmp/$asset.sha256" || die "could not download $base/$asset.sha256"

want=$(cut -d ' ' -f 1 "$tmp/$asset.sha256")
if command -v sha256sum >/dev/null 2>&1; then
  have=$(sha256sum "$tmp/$asset" | cut -d ' ' -f 1)
elif command -v shasum >/dev/null 2>&1; then
  have=$(shasum -a 256 "$tmp/$asset" | cut -d ' ' -f 1)
else
  die "needs sha256sum or shasum to check the download"
fi
[ -n "$want" ] && [ "$want" = "$have" ] || die "the checksum of $asset does not match (expected $want, got $have); nothing was installed"

(cd "$tmp" && tar -xzf "$asset")
bin=$tmp/hippocamp-$version-$target/hippocamp
[ -f "$bin" ] || die "$asset holds no hippocamp binary"

mkdir -p "$dir"
cp "$bin" "$dir/.hippocamp.new.$$"
chmod 755 "$dir/.hippocamp.new.$$"
mv -f "$dir/.hippocamp.new.$$" "$dir/hippocamp"
say "Installed hippocamp $version to $dir/hippocamp"

next=hippocamp
case ":$PATH:" in
  *":$dir:"*) ;;
  *)
    next=$dir/hippocamp
    say ""
    say "$dir is not on your PATH. To add it, run:"
    case ${SHELL:-} in
      */zsh) say "  echo 'export PATH=\"$dir:\$PATH\"' >> ~/.zshrc && exec zsh" ;;
      */bash) say "  echo 'export PATH=\"$dir:\$PATH\"' >> ~/.bashrc && exec bash" ;;
      */fish) say "  fish_add_path $dir" ;;
      *) say "  export PATH=\"$dir:\$PATH\"   (and add that line to your shell's startup file)" ;;
    esac
    ;;
esac

say ""
say "Next, run:"
say "  $next"
