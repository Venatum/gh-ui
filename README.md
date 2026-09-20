# gh-ui

A Pull Request explorer in the terminal, `htop`-style: the list of open PRs
across all repos in a folder, with keyboard filters, colors, and optional
auto-refresh.

## Requirements

[`gh`](https://cli.github.com) installed **and authenticated** (`gh auth login`).
Every API call goes through it, so gh-ui inherits your existing GitHub login.

## Install

### As a `gh` extension

```bash
gh extension install Venatum/gh-ui
gh ui                       # scan the current folder
gh ui ~/dev/workspace             # scan a specific folder
```

This is the way in. The repo is named `gh-ui`, which is what makes `gh` expose
it as the `gh ui` subcommand. The installer downloads the prebuilt binary for
your platform, so this needs no Rust toolchain. Update with
`gh extension upgrade gh-ui`.

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
gh-ui ~/dev/workspace             # scan a specific folder holding several git repos
gh-ui --help                # usage summary
gh-ui --version             # version
```

The app discovers the subfolders that are git repos and aggregates their open
PRs. The header shows the authenticated account in its top-right
corner (`@?` if `gh` could not resolve it), and hides it when the terminal is
too narrow to fit both it and the status line.

The **Actions** tab lists the workflow runs of the same repos. Its `my PRs`
filter (`m`, on by default) keeps only the runs sitting on a branch that
carries an open PR, so gh-ui fetches a wide window of runs per repo — a busy
`main` or `develop` would otherwise fill a narrow one on its own and leave the
tab empty. With the filter off the tab shows the 20 most recent runs of each
repo, all branches together.

## Shortcuts

### List

| Key          | Action                                       |
|--------------|----------------------------------------------|
| `↑/↓`, `j/k` | navigate the list                            |
| `enter`      | open the selected PR in the browser          |
| `r`          | reload now                                    |
| `a`          | cycle auto-refresh (off, 1mn … 1h)           |
| `A`          | turn auto-refresh off                        |
| `f`          | open the filters panel                       |
| `c`          | open the columns panel                       |
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

Auto-refresh cycles through `off → 1mn → 5mn → 10mn → 30mn → 1h → off`. It
starts **off** the first time, and the pace you leave it on is remembered
between runs (in `~/.config/gh-ui/refresh.json`). `A` turns it off in one
keystroke, whatever the current pace.

### Columns panel (`c`)

| Key          | Action                                             |
|--------------|----------------------------------------------------|
| `↑/↓`, `j/k` | choose a column (or move the grabbed one, see below) |
| `enter`      | show / hide the focused column                     |
| `space`      | grab the focused column, or drop it if already grabbed |
| `esc`        | drop the grabbed column, or close the panel if none is grabbed |
| `c`          | close the panel (dropping any grabbed column)       |

Moving a column is a direct-manipulation gesture: `space` grabs the column
under the cursor, then `↑`/`↓` move it left/right in the table (the cursor
follows it), and `space`, `enter` or `esc` drops it again. The panel's own
hint line changes to match the mode.

The panel edits the columns of the **active tab**: PRs and Actions keep their
own order and their own hidden columns. At least one column always stays
visible.

## Memory

Filters are saved on every change to `~/.config/gh-ui/filters.json` and
reloaded at startup — no need to retype the same selection on each launch.

The column layout is saved the same way, to `~/.config/gh-ui/columns.json`.
A column added by a future version is appended to your saved layout instead of
resetting it.

## Development

```bash
cargo run                   # scan the current folder
cargo run -- ~/dev/workspace      # scan a specific folder
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
| `columns.rs`   | table columns: registry, order/visibility, persistence      |
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
release named `gh-ui_<version>_<os>-<arch>` — `gh` picks the asset whose name
ends with your platform, which is what `gh extension install` relies on.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your
option.
