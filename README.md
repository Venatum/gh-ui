# gh-ui

A Pull Request explorer in the terminal, `htop`-style: the list of open PRs
across all repos in a folder, with keyboard filters, colors, and optional
auto-refresh.

## Requirements

[`gh`](https://cli.github.com) installed **and authenticated** (`gh auth login`).
Every API call goes through it, so gh-ui inherits your existing GitHub login.

## Install

Pick whichever fits; they all end up with the same binary.

### As a `gh` extension

```bash
gh extension install Venatum/gh-ui
gh ui                       # scan the current folder
gh ui ~/dev/clm             # scan a specific folder
```

The repo is named `gh-ui`, which is what makes `gh` expose it as the `gh ui`
subcommand. The installer downloads the prebuilt binary for your platform, so
this needs no Rust toolchain. Update with `gh extension upgrade gh-ui`.

### With Homebrew

```bash
brew install Venatum/tap/gh-ui
```

Also a prebuilt binary on macOS and Linux (arm64 and x86_64); anything else
falls back to compiling from source.

> Not available yet: tap publishing is off in the release workflow. See
> [Releasing](#releasing).

### With Cargo

```bash
cargo install gh-ui
```

Compiles on your machine, so it works on any platform Rust supports. Requires
Rust 1.85+ (2024 edition).

> Not available yet: crates.io publishing is off in the release workflow. See
> [Releasing](#releasing).

### From a clone

```bash
git clone https://github.com/Venatum/gh-ui.git
cd gh-ui
cargo install --path .
```

Puts `gh-ui` in `~/.cargo/bin`, which is already on your `PATH`. Rerun after
each change to update the installed copy — `cargo run` uses the working tree,
`gh-ui` uses the installed binary, and the two drift apart otherwise.

## Usage

```bash
gh-ui                       # scan the current folder
gh-ui ~/dev/clm             # scan a specific folder holding several git repos
gh-ui --help                # usage summary
gh-ui --version             # version
```

The app discovers the subfolders that are git repos and aggregates their open
PRs.

## Shortcuts

### List

| Key          | Action                                       |
|--------------|----------------------------------------------|
| `↑/↓`, `j/k` | navigate the list                            |
| `enter`      | open the selected PR in the browser          |
| `r`          | reload now                                    |
| `a`          | enable / disable auto-refresh (60 s)         |
| `f`          | open the filters panel                       |
| `?`          | help screen                                  |
| `q`          | quit                                         |

### Filters panel (`f`)

| Key          | Action                                             |
|--------------|----------------------------------------------------|
| `↑/↓`, `j/k` | choose a filter                                    |
| `←/→`, `h/l` | change its value (cycles: mode/since/repo ; checkboxes: draft/unreviewed/not-mine) |
| `enter`      | edit `author` / `label(s)` (text input)            |
| `esc`, `f`   | close the panel                                    |

Available filters: `filter` (all/me/review-asked), `since`, `no-draft`,
`unreviewed`, `not-mine`, `repo`, `author`, `label(s)`.

Auto-refresh is **disabled by default**.

## Filter memory

Filters are saved on every change to
`~/.config/gh-ui/filters.json` and reloaded at startup — no need to
retype the same selection on each launch.

## Development

```bash
cargo run                   # scan the current folder
cargo run -- ~/dev/clm      # scan a specific folder
cargo test
cargo build --release
```

### Architecture

| File           | Role                                                        |
|----------------|-------------------------------------------------------------|
| `main.rs`      | event loop, keyboard routing, TUI setup/teardown            |
| `app.rs`       | application state and its logic                             |
| `model.rs`     | data structures (`Pr`, deserialization of `gh` JSON)        |
| `filters.rs`   | filter state, `gh` args, persistence                        |
| `gh.rs`        | repo discovery + launching `gh pr list`                     |
| `fetch.rs`     | background loading (thread + `mpsc` channel)                |
| `ui.rs`        | ratatui rendering                                           |

## Releasing

A release is one tag push. Everything else is
[`.github/workflows/release.yml`](.github/workflows/release.yml).

```bash
# 1. bump the version in Cargo.toml, then refresh the lockfile
cargo build
git commit -am "chore(release): 0.2.0"

# 2. tag and push — the tag must match Cargo.toml, the workflow checks it
git tag v0.2.0
git push && git push origin v0.2.0
```

The workflow runs the tests, builds four binaries (`darwin-arm64`,
`darwin-amd64`, `linux-amd64`, `linux-arm64`), and attaches them to the GitHub
release in two shapes:

- the raw binaries, named `gh-ui_<version>_<os>-<arch>` — `gh` picks the asset
  whose name ends with your platform, which is what `gh extension install`
  relies on;
- the same binaries as `.tar.gz`, which the Homebrew formula downloads.

### Turning on Homebrew and crates.io

Both are **off**, via two switches at the top of the workflow:

```yaml
env:
  PUBLISH_HOMEBREW: "false"
  PUBLISH_CRATES: "false"
```

Each needs a one-time setup before its switch is worth flipping to `"true"`.

**crates.io** — create a token on <https://crates.io/settings/tokens>, store it
as the `CARGO_REGISTRY_TOKEN` repository secret, then set `PUBLISH_CRATES` to
`"true"`. The first publish also reserves the `gh-ui` name.

**Homebrew tap** — Homebrew only resolves `Venatum/tap` to a repo literally
named `homebrew-tap`:

1. create the `Venatum/homebrew-tap` repo (public, empty);
2. store a personal access token with `contents: write` on it as the
   `HOMEBREW_TAP_TOKEN` secret here;
3. set `PUBLISH_HOMEBREW` to `"true"`.

The workflow then renders the formula, attaches it to the release, and commits
it to the tap as `Formula/gh-ui.rb`.

The formula lives in
[`packaging/homebrew/gh-ui.rb.tmpl`](packaging/homebrew/gh-ui.rb.tmpl); the
version and checksums are filled in at release time. Edit the template, never
the copy in the tap.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your
option.
