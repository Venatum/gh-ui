#!/usr/bin/env bash
#
# Cuts a release from the working tree: bumps the version, runs the same three
# gates as CI, then commits, tags and pushes — the tag push is what triggers
# `.github/workflows/release.yml` and builds the binaries.
#
# Nothing is committed before the confirmation prompt, and answering anything
# but `y` puts Cargo.toml and Cargo.lock back the way they were.
set -euo pipefail

cd "$(dirname "$0")/.."

BRANCH=main

die() {
	printf '\033[31merror:\033[0m %s\n' "$1" >&2
	exit 1
}

step() {
	printf '\n\033[1m==>\033[0m %s\n' "$1"
}

# The version lives in Cargo.toml's `[package]` section, which comes first, so
# the first `version = ` line in the file is the one to read and to write.
manifest_version() {
	awk -F'"' '/^version = "/ { print $2; exit }' Cargo.toml
}

set_manifest_version() {
	local new=$1 tmp
	tmp=$(mktemp)
	awk -v v="$new" '
		!done && /^version = "/ { sub(/"[^"]*"/, "\"" v "\""); done = 1 }
		{ print }
	' Cargo.toml >"$tmp"
	mv "$tmp" Cargo.toml
}

# Restores both files on a refused confirmation or a failed gate, so a stopped
# release leaves nothing behind.
restore_manifest() {
	git checkout -- Cargo.toml Cargo.lock
}

# 1. The tree has to be releasable before anything is touched.
step 'Checking the working tree'
[ "$(git rev-parse --abbrev-ref HEAD)" = "$BRANCH" ] \
	|| die "not on $BRANCH — release from $BRANCH only"
[ -z "$(git status --porcelain --untracked-files=no)" ] \
	|| die 'uncommitted changes — commit or stash them first'
git fetch --quiet --tags
[ "$(git rev-parse HEAD)" = "$(git rev-parse "origin/$BRANCH")" ] \
	|| die "$BRANCH and origin/$BRANCH disagree — pull or push first"
echo "clean, on $BRANCH, up to date with origin"

# 2. Ask for the new version, suggesting the next patch.
current=$(manifest_version)
[ -n "$current" ] || die 'no version found in Cargo.toml'
latest_tag=$(git tag --list 'v*' --sort=-v:refname | head -1)

printf '\nCargo.toml version : \033[1m%s\033[0m\n' "$current"
printf 'latest tag         : \033[1m%s\033[0m\n' "${latest_tag:-none}"

suggested=$(echo "$current" | awk -F. '{ printf "%s.%s.%s", $1, $2, $3 + 1 }')
printf '\nNew version [%s]: ' "$suggested"
read -r version
version=${version:-$suggested}
version=${version#v}

echo "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$' \
	|| die "'$version' is not a version number like 1.2.3"
[ "$version" != "$current" ] || die "$version is already the current version"
if git rev-parse --verify --quiet "refs/tags/v$version" >/dev/null; then
	die "tag v$version already exists"
fi

# 3. Bump, refresh the lockfile, then run the gates CI runs.
step "Bumping to $version"
set_manifest_version "$version"
trap restore_manifest EXIT
cargo check --quiet          # rewrites Cargo.lock with the new version
grep -q "^version = \"$version\"$" Cargo.toml || die 'the bump did not take'

step 'Formatting'
cargo fmt --check
step 'Lints'
cargo clippy --all-targets --quiet -- -D warnings
step 'Tests'
cargo test --locked --quiet

# 4. Everything below is what the confirmation buys.
step 'Ready to release'
git --no-pager diff --stat
cat <<EOF

This will:
  git commit -m "chore(release): $version"   (Cargo.toml, Cargo.lock)
  git tag v$version
  git push origin $BRANCH v$version

Pushing the tag starts the release workflow and publishes the binaries.
EOF

printf '\nRelease %s? [y/N] ' "$version"
read -r answer
case "$answer" in
	y | Y | yes) ;;
	*)
		echo 'aborted — Cargo.toml and Cargo.lock restored'
		exit 1
		;;
esac

# From here the bump is meant to stay, so the restore trap is dropped.
trap - EXIT

step 'Releasing'
git add Cargo.toml Cargo.lock
git commit --message "chore(release): $version"
git tag "v$version"
git push origin "$BRANCH" "v$version"

printf '\n\033[32mv%s pushed.\033[0m Follow the build:\n  gh run watch\n' "$version"
