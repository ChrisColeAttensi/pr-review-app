#!/usr/bin/env bash
# Build one of the two apps, publish it on the tap, and update its Homebrew cask.
#   scripts/release.sh review 0.4.0   # PR Review, the desktop app   (tags v0.4.0)
#   scripts/release.sh bar 0.1.0      # PR Bar, the menu bar app     (tags bar-v0.1.0)
set -euo pipefail

USAGE="usage: scripts/release.sh <review|bar> <version>  (e.g. review 0.4.0)"
APP_KIND="${1:?$USAGE}"
VERSION="${2:?$USAGE}"
REPO="ChrisColeAttensi/pr-review-app"
TAP="ChrisColeAttensi/homebrew-tap"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

case "$APP_KIND" in
  review) NAME="PR Review"; CASK="pr-review"; TAURI_DIR="src-tauri";     TAG_PREFIX="v" ;;
  bar)    NAME="PR Bar";    CASK="pr-bar";    TAURI_DIR="bar/src-tauri"; TAG_PREFIX="bar-v" ;;
  *) echo "$USAGE"; exit 1 ;;
esac
TAG="$TAG_PREFIX$VERSION"

# Network hiccups shouldn't strand a half-finished release.
retry() { for i in 1 2 3 4; do "$@" && return 0; echo "  retrying ($i)…" >&2; sleep $((i * 3)); done; "$@"; }

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must look like 1.2.3"; exit 1; }
[[ -z "$(git status --porcelain)" ]] || { echo "working tree not clean; commit first"; exit 1; }
git rev-parse "$TAG" >/dev/null 2>&1 && { echo "tag $TAG already exists"; exit 1; }

echo "→ bumping version to $VERSION"
python3 - "$VERSION" "$TAURI_DIR" "$APP_KIND" <<'PY'
import json, re, sys
v, tauri_dir, kind = sys.argv[1:4]
files = [f"{tauri_dir}/tauri.conf.json"] + (["package.json"] if kind == "review" else [])
for p in files:
    d = json.load(open(p)); d["version"] = v
    open(p, "w").write(json.dumps(d, indent=2) + "\n")
p = f"{tauri_dir}/Cargo.toml"; s = open(p).read()
open(p, "w").write(re.sub(r'(?m)^version = "[^"]+"', f'version = "{v}"', s, count=1))
PY

echo "→ building"
TAURI_APP_PATH="$TAURI_DIR" bun run tauri build --bundles app
APP="target/release/bundle/macos/$NAME.app"
mkdir -p release
ZIP="release/${NAME// /-}-$VERSION-arm64.zip"
rm -f "$ZIP"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"
SHA="$(shasum -a 256 "$ZIP" | cut -d' ' -f1)"

echo "→ tagging $REPO"
PREV_TAG="$(git describe --tags --abbrev=0 --match "$TAG_PREFIX[0-9]*" 2>/dev/null || true)"
git add package.json "$TAURI_DIR/tauri.conf.json" "$TAURI_DIR/Cargo.toml" Cargo.lock
git diff --cached --quiet || git commit -qm "Release $NAME $VERSION"
git tag "$TAG"
# Push the branch first: if it is rejected (someone else pushed), the tag isn't published either.
retry git push -q origin HEAD
retry git push -q origin "$TAG"

# The build is published on the tap, not here: installing then needs read access to the
# tap only, and this repo (the source) stays private to whoever edits it.
echo "→ publishing the build on $TAP"
NOTES="$(git log --format='- %s' ${PREV_TAG:+$PREV_TAG..}HEAD | grep -v '^- Release ' || true)"
RELEASE="$CASK-v$VERSION"
# A retried create can find the release already made by an attempt that timed out late.
retry sh -c 'gh release view "$1" -R "$2" >/dev/null 2>&1 || gh release create "$1" "$3" -R "$2" --title "$4" --notes "$5" >/dev/null' _ \
  "$RELEASE" "$TAP" "$ZIP" "$NAME $VERSION" "${NOTES:-Release $VERSION}"
ASSET_ID="$(gh api "repos/$TAP/releases/tags/$RELEASE" --jq '.assets[0].id')"
[[ -n "$ASSET_ID" ]] || { echo "release $RELEASE has no asset; upload $ZIP to it by hand"; exit 1; }

echo "→ updating cask in $TAP"
TAPDIR="$(mktemp -d)"
trap 'rm -rf "$TAPDIR"' EXIT
retry gh repo clone "$TAP" "$TAPDIR" -- -q
mkdir -p "$TAPDIR/Casks"
sed -e "s/__VERSION__/$VERSION/" -e "s/__SHA256__/$SHA/" -e "s/__ASSET_ID__/$ASSET_ID/" \
  "packaging/$CASK.rb.template" > "$TAPDIR/Casks/$CASK.rb"
git -C "$TAPDIR" add "Casks/$CASK.rb"
git -C "$TAPDIR" commit -qm "$CASK $VERSION"
retry git -C "$TAPDIR" push -q

echo "✓ $NAME $VERSION released. Install: brew install --cask chriscoleattensi/tap/$CASK · update: brew update && brew upgrade --cask $CASK"
