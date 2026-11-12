#!/bin/sh
# Builds the whole static site into docs/site/out (gitignored), for GitHub
# Pages at https://vanitascaesar1.github.io/hippo/:
#
#   index.html               the landing page (docs/site/landing.html)
#   packs.html, themes.html  the registry's lists; packs/<name>.html, index.json
#   hippo.css, registry.css, icon.svg
#   landing.css              the landing page's own stylesheet
#   assets/                  docs/assets/ (the card image)
#   docs/                    the docs (mdBook)
#
#   CARGO_TARGET_DIR=<dir> docs/site/build.sh
#
# Needs cargo and mdbook. Every link is relative, so the site works under the
# /hippo/ prefix and from a folder on disk. Builds nothing that runs
# JavaScript, loads a font or calls another host, and fails if any page says
# otherwise or links a file of the site that does not exist.
set -eu

root=$(cd "$(dirname "$0")/../.." && pwd)
out="$root/docs/site/out"
cd "$root"

rm -rf "$out"
cargo run -p hippo-registry --locked -j 4 -- build --out "$out" --offline \
  --landing "$root/docs/site/landing.html"

for f in index.html packs.html hippo.css icon.svg; do
  if [ ! -f "$out/$f" ]; then
    echo "error: the registry build did not write $f" >&2
    exit 1
  fi
done

mkdir -p "$out/assets"
cp "$root"/docs/assets/* "$out/assets/"
cp "$root/docs/site/landing.css" "$out/landing.css"

mdbook build "$root/docs/site" -d "$out/docs"

# Drop every mdBook asset the docs pages do not use: scripts, fonts,
# highlight and theme files, the favicon, the 404, print and standalone
# contents pages.
(
  cd "$out/docs"
  rm -f 404.html print.html toc.html
  refs=$(grep -ohE '(href|src)="[^"#]+"' ./*.html | sed -E 's/^(href|src)="//; s/"$//' | sort -u)
  find . -type f ! -name '*.html' | sed 's|^\./||' | while read -r f; do
    printf '%s\n' "$refs" | grep -qxF "$f" || rm -f "$f"
  done
  find . -type d -empty -delete
)

# Check every page of the whole site: no script, nothing loaded from another
# host, and every local link and resource exists.
cd "$out"
fail=0
pages=$(find . -name '*.html' | sed 's|^\./||' | sort)
for page in $pages; do
  if grep -q '<script' "$page"; then
    echo "error: $page contains <script" >&2
    fail=1
  fi
  if grep -qE '(src|<link[^>]*rel="stylesheet"[^>]*href)="(https?:)?//' "$page"; then
    echo "error: $page loads a resource from another host" >&2
    fail=1
  fi
done
checked=$(mktemp)
trap 'rm -f "$checked"' EXIT
for page in $pages; do
  dir=$(dirname "$page")
  grep -oE '(href|src)="[^"]+"' "$page" | sed -E 's/^(href|src)="//; s/"$//' | while IFS= read -r ref; do
    case "$ref" in
      http://*|https://*|mailto:*|'#'*) continue ;;
    esac
    target=${ref%%#*}
    if [ -e "$dir/$target" ]; then
      echo ok >>"$checked"
    else
      echo "error: $page links $ref, which does not exist" >&2
      echo bad >>"$checked"
    fi
  done
done
links=$(wc -l <"$checked" | tr -d ' ')
if grep -q bad "$checked"; then fail=1; fi
[ "$fail" -eq 0 ] || exit 1
echo "site: $(printf '%s\n' "$pages" | wc -l | tr -d ' ') pages in $out, $links internal links checked, no script, no foreign host"
