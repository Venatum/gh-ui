#!/usr/bin/env bash
# Records assets/demo.gif, the README's GIF, with vhs (brew install vhs).
#
# Everything on screen is canned: `demo/gh` stands in for `gh` and answers
# from demo/fixtures, in a throwaway folder of empty repos whose origin is
# acme/<name>. HOME points to a throwaway folder too, so the recording never
# reads nor writes the real gh-ui settings.
#
#   ./demo/record.sh           # record assets/demo.gif
#   ./demo/record.sh --shell   # the same setup, in a shell, to try things out
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo"
cargo build --release --quiet

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/home" "$work/workspace"

# `git` in one demo repo, whatever the user's git config says about
# identity, signing or hooks.
demo_git() {
  git -C "$work/workspace/$1" -c user.name=demo -c user.email=demo@example.com \
    -c commit.gpgsign=false -c tag.gpgsign=false -c core.hooksPath=/dev/null "${@:2}"
}

for name in api web cli docs; do
  git init --quiet "$work/workspace/$name"
  demo_git "$name" remote add origin "git@github.com:acme/$name.git"
  # A history for the Releases tab's Unreleased column, which counts the
  # commits between the latest release's tag (the fixture's first entry)
  # and origin/HEAD, from local git only.
  case "$name" in
    api) since=3 ;; web) since=27 ;; cli) since=8 ;; *) since=5 ;;
  esac
  demo_git "$name" commit --quiet --allow-empty -m "initial commit"
  tag="$(jq -r '.[0].tagName // empty' "demo/fixtures/releases/$name.json" 2>/dev/null || true)"
  [[ -n "$tag" ]] && demo_git "$name" tag "$tag"
  for ((i = 1; i <= since; i++)); do
    demo_git "$name" commit --quiet --allow-empty -m "change $i"
  done
  demo_git "$name" update-ref refs/remotes/origin/HEAD HEAD
done

export DEMO_PATH="$repo/demo:$repo/target/release:$PATH"
export DEMO_HOME="$work/home"
export DEMO_WORKSPACE="$work/workspace"

if [[ "${1:-}" == "--shell" ]]; then
  cd "$DEMO_WORKSPACE"
  PATH="$DEMO_PATH" HOME="$DEMO_HOME" bash --norc
else
  mkdir -p assets
  vhs demo/demo.tape
fi
