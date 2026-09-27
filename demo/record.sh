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
for name in api web cli docs; do
  git init --quiet "$work/workspace/$name"
  git -C "$work/workspace/$name" remote add origin "git@github.com:acme/$name.git"
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
