# gh-ui

A Pull Request explorer in the terminal, `htop`-style: the list of open PRs
across all repos in a folder, with keyboard filters, colors, and optional
auto-refresh. Inspired by the `list-prs.sh` script.

## Prerequisites

- [Rust](https://rustup.rs) (2024 edition)
- [`gh`](https://cli.github.com) installed **and authenticated** (`gh auth login`)

## Usage

```bash
# scan the current folder
cargo run

# scan a specific folder containing several git repos
cargo run -- ~/dev/workspace

# optimized build
cargo run --release -- ~/dev/workspace
```

The app automatically discovers subfolders that are git repos and
aggregates their open PRs.

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

## Architecture

| File           | Role                                                        |
|----------------|-------------------------------------------------------------|
| `main.rs`      | event loop, keyboard routing, TUI setup/teardown            |
| `app.rs`       | application state and its logic                             |
| `model.rs`     | data structures (`Pr`, deserialization of `gh` JSON)        |
| `filters.rs`   | filter state, `gh` args, persistence                        |
| `gh.rs`        | repo discovery + launching `gh pr list`                     |
| `fetch.rs`     | background loading (thread + `mpsc` channel)                |
| `ui.rs`        | ratatui rendering                                           |

## Tests

```bash
cargo test
```
