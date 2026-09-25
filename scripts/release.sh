#!/bin/sh
# Cut a release: bump fael/Cargo.toml, commit + tag on main, push.
# The tag runs .github/workflows/release.yml → GitHub Release + npm @inonix/fael + Homebrew tap.
# usage: scripts/release.sh [patch|minor|major]   (default patch)
set -eu
cd "$(git rev-parse --show-toplevel)"

part=${1:-patch}
case $part in patch | minor | major) ;; *) echo "usage: $0 [patch|minor|major]" >&2; exit 2 ;; esac

[ -z "$(git status --porcelain)" ] || { echo "release: working tree is dirty — commit or stash first" >&2; exit 1; }
git checkout -q main
git pull -q --ff-only

old=$(sed -n 's/^version = "\(.*\)"$/\1/p' fael/Cargo.toml | head -n1)
IFS=. read -r major minor patch <<V
$old
V
case $part in
patch) patch=$((patch + 1)) ;;
minor) minor=$((minor + 1)); patch=0 ;;
major) major=$((major + 1)); minor=0; patch=0 ;;
esac
new=$major.$minor.$patch

git rev-parse -q --verify "refs/tags/v$new" >/dev/null && { echo "release: tag v$new already exists" >&2; exit 1; }

# first `version = ` line only — the [package] one
perl -0pi -e "s/^version = \"\Q$old\E\"/version = \"$new\"/m" fael/Cargo.toml
cargo metadata --format-version 1 >/dev/null # rewrites Cargo.lock for the new version

git commit -q -am "release v$new"
git tag -a "v$new" -m "release v$new"
git push -q origin main --follow-tags

echo "v$old -> v$new pushed. Watch: gh run watch \$(gh run list --workflow release.yml -L1 --json databaseId --jq '.[0].databaseId')"
