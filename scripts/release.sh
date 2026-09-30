#!/usr/bin/env bash
# Build, publish a GitHub release, and update the Homebrew cask.
#   scripts/release.sh 0.2.0
set -euo pipefail

VERSION="${1:?usage: scripts/release.sh <version>  (e.g. 0.2.0)}"
REPO="ChrisColeAttensi/pr-review-app"
TAP="ChrisColeAttensi/homebrew-tap"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

# Network hiccups shouldn't strand a half-finished release.
retry() { for i in 1 2 3 4; do "$@" && return 0; echo "  retrying ($i)…" >&2; sleep $((i * 3)); done; "$@"; }

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must look like 1.2.3"; exit 1; }
[[ -z "$(git status --porcelain)" ]] || { echo "working tree not clean; commit first"; exit 1; }
git rev-parse "v$VERSION" >/dev/null 2>&1 && { echo "tag v$VERSION already exists"; exit 1; }

echo "→ bumping version to $VERSION"
python3 - "$VERSION" <<'PY'
import json, re, sys
v = sys.argv[1]
for p in ["package.json", "src-tauri/tauri.conf.json"]:
    d = json.load(open(p)); d["version"] = v
    open(p, "w").write(json.dumps(d, indent=2) + "\n")
p = "src-tauri/Cargo.toml"; s = open(p).read()
open(p, "w").write(re.sub(r'(?m)^version = "[^"]+"', f'version = "{v}"', s, count=1))
PY

echo "→ building"
bun run tauri build --bundles app
APP="src-tauri/target/release/bundle/macos/PR Review.app"
mkdir -p release
ZIP="release/PR-Review-$VERSION-arm64.zip"
rm -f "$ZIP"
ditto -c -k --sequesterRsrc --keepParent "$APP" "$ZIP"
SHA="$(shasum -a 256 "$ZIP" | cut -d' ' -f1)"

echo "→ tagging and publishing"
git add package.json src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock
git diff --cached --quiet || git commit -qm "Release v$VERSION"
git tag "v$VERSION"
retry git push -q origin HEAD "v$VERSION"
retry gh release create "v$VERSION" "$ZIP" -R "$REPO" --title "v$VERSION" --generate-notes >/dev/null
ASSET_ID="$(gh api "repos/$REPO/releases/tags/v$VERSION" --jq '.assets[0].id')"

echo "→ updating cask in $TAP"
TAPDIR="$(mktemp -d)"
trap 'rm -rf "$TAPDIR"' EXIT
retry gh repo clone "$TAP" "$TAPDIR" -- -q
mkdir -p "$TAPDIR/Casks"
sed -e "s/__VERSION__/$VERSION/" -e "s/__SHA256__/$SHA/" -e "s/__ASSET_ID__/$ASSET_ID/" \
  packaging/pr-review.rb.template > "$TAPDIR/Casks/pr-review.rb"
git -C "$TAPDIR" add Casks/pr-review.rb
git -C "$TAPDIR" commit -qm "pr-review $VERSION"
retry git -C "$TAPDIR" push -q

echo "✓ v$VERSION released. Update with: brew update && brew upgrade --cask pr-review"
