# gh-ui

`htop` for your GitHub work: the open PRs, their CI runs, the issues, the
repos and the releases of every git repo in a folder, in one terminal UI,
with keyboard filters and optional auto-refresh.

![gh-ui: the PRs, Actions, Issues and Repos tabs](assets/demo.gif)

- **PRs**: every open PR across the folder's repos, with review state, size
  and labels. `m` narrows to yours.
- **Actions**: the workflow runs sitting on those PRs' branches.
- **Issues**: the open issues, who is on them and the PRs that close them.
- **Repos**: an owner's repositories, which ones you already have, and a
  batch clone for the others.
- **Releases**: each repo's latest release, how many commits wait for the
  next one, and the repos that never had one.

## Requirements

[`gh`](https://cli.github.com) installed **and authenticated** (`gh auth login`).
Every call goes through it, so gh-ui inherits your existing GitHub login.

## Install

### As a `gh` extension

```bash
gh extension install Venatum/gh-ui
gh ui                       # scan the current folder
gh ui ~/dev/workspace       # scan a specific folder
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
gh-ui ~/dev/workspace       # scan a specific folder holding several git repos
gh-ui --help                # usage summary
gh-ui --version             # version
```

The app discovers the subfolders that are git repos and aggregates what
GitHub has for them. The header shows the authenticated account in its
top-right corner (`@?` if `gh` could not resolve it), and hides it when the
terminal is too narrow to fit both it and the status line.

Two ways to narrow a list. `/` searches what is on screen as you type,
without re-querying anything (see [Search](#search-)). The filters panel
(`f`) does the opposite: it changes what gh-ui asks GitHub for, and every
change costs a reload.

## The tabs

### PRs

Every open PR of the scanned repos (at most 50 per repo): repo, number,
title, author, review decision, `+/-` size and labels, repo by repo. A
draft reads `[D]` and grayed. `m` shows only
yours (`author:@me`), and pressing it again shows everyone's.

### Actions

The workflow runs of the same repos. Its `my PRs` filter (`m`, on by
default) keeps only the runs sitting on a branch that carries an open PR, so
gh-ui fetches a wide window of runs per repo — a busy `main` or `develop`
would otherwise fill a narrow one on its own and leave the tab empty. With
the filter off the tab shows the 20 most recent runs of each repo, all
branches together.

### Issues

The open issues of every repo in the folder, newest update first. The `PRs`
column shows the pull requests linked to the issue (GitHub's "Development"
section, or a `closes #N` in the PR): `#12` in the same repo,
`owner/name#12` elsewhere, `3 PRs` when there are several. `m` shows the
issues assigned to you, and pressing it again shows everyone's.

There is no comments column on purpose: `gh` only gives the comments with
their full text, which would make every load several times slower.
`Created` is there but hidden; show it from the columns panel (`c`).

### Repos

The repositories of an owner — your account, or one of your orgs, picked
with `owner` in the filters panel. Each row says where it stands against the
scanned folder, judged by the folder's `origin` remote rather than its name:
`✓ cloned`, `name taken → other/api` when a folder of that name holds
another repo, `folder taken (not a repo)` when something else sits there
(neither is clonable: `gh repo clone` would refuse an existing
destination), or blank when it can be cloned. Archived repos and forks are hidden unless their box is
ticked, and show dimmed with `[A]` / `[F]` when they are.

`space` ticks clonable rows; a `[ Clone N repos ]` row then appears under the
list — go down to it and press `enter`, confirm with `y`, and gh-ui clones
them one after the other into the scanned folder, each row showing `queued`,
`cloning…`, then `✓ cloned` or `✗ failed` (tick a failed row again to
retry). When the batch is over, the new repos show up in the other tabs.
The owner stays put while a batch runs, and quitting asks first. Started
in a folder that holds no
git repo, gh-ui opens on this tab.

### Releases

The latest release of every repo in the folder, one row per repo, the most
recently published first: tag, name and date. "Latest" is the release
GitHub badges so, or — for a repo that only ever shipped pre-releases — the
most recent one that is not a draft; a pre-release reads `[P]` after its
tag. A repo that never released keeps its row, grayed, reading
`no release`, at the bottom of the list. `enter` opens the release on
GitHub, or the repo's releases page on a `no release` row.

`Unreleased` counts the commits on the default branch since the release's
tag — what the next release would ship. It is asked of your local clone
(`git rev-list --count <tag>..origin/HEAD`), never of the network, so it is
as fresh as your last `git fetch`. A gray `?` means the clone cannot tell:
the tag was never fetched, or there is no `origin/HEAD`
(`git remote set-head origin --auto` sets it).

There are no filters and no `m` here: one row per repo is short enough, and
`/` narrows it. Like the issues, the releases load on the first visit to
the tab, and the auto-refresh keeps them fresh from then on.

## Shortcuts

### List

| Key          | Action                                       |
|--------------|----------------------------------------------|
| `↑/↓`, `j/k` | navigate the list                            |
| `PgUp/PgDn`  | move a page up / down                        |
| `Home/End`   | jump to the first / last row                 |
| `enter`      | open the selected PR, run, issue, repo or release in the browser (Repos: on the clone button, clone the ticked repos) |
| `space`      | Repos tab: tick / untick the repo            |
| `tab`, `1-5` | switch tab (PRs / Actions / Issues / Repos / Releases) |
| `/`          | search the visible list (live, client-side)  |
| `esc`        | clear the search                             |
| `m`          | mine on/off: my PRs · their runs · issues assigned to me |
| `r`          | reload now                                   |
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
| `←/→`, `h/l` | change its value (cycles its values, or ticks a box) |
| `enter`      | edit `author` / `label(s)` (text input)            |
| `esc`, `f`   | close the panel                                    |

Each tab has its own filters:

- **PRs**: `repo`, `author`, `review asked`, `since`, `no-draft`,
  `unreviewed`, `label(s)`. `author` cycles through `any`, `@me` and
  `not @me` with `←/→`; `enter` types any other login, `-login` to exclude
  it. `m` sets `author:@me` and unticks `review asked`.
- **Actions**: `repo` (shared with the PRs tab), plus `only my PRs`,
  `status` (all/failed/running/success), `event` and `workflow`. Unlike the
  PR filters these never reach `gh` — they narrow the runs already in
  memory, so they cost no request and apply instantly. `event` and
  `workflow` cycle over the values present in the loaded runs, so a repo
  with no cron never offers `event:schedule`. They apply *before* the
  20-runs-per-repo cut, which is what lets `status:failed` surface a failure
  well past that limit. They are not saved: the tab reopens on `my PRs`.
- **Issues**: `repo`, `author`, `assignee` (`any`, `@me`, or `nobody` for
  the unassigned ones), `since` and `label(s)` — independent of the PR ones.
- **Repos**: `owner` (your account, then your orgs), `archived`, `forks` and
  `hide cloned` (keeps only what is left to clone).
- **Releases**: none; the panel says so.

### Columns panel (`c`)

| Key          | Action                                             |
|--------------|----------------------------------------------------|
| `↑/↓`, `j/k` | choose a column (or move the grabbed one, see below) |
| `enter`      | show / hide the focused column                     |
| `space`      | grab the focused column, or drop it if already grabbed |
| `esc`        | drop the grabbed column, or close the panel if none is grabbed |
| `c`          | close the panel (dropping any grabbed column)      |

Moving a column is a direct-manipulation gesture: `space` grabs the column
under the cursor, then `↑`/`↓` move it left/right in the table (the cursor
follows it), and `space`, `enter` or `esc` drops it again. The panel's own
hint line changes to match the mode.

The panel edits the columns of the **active tab**: each tab keeps its own
order and its own hidden columns. At least one column always stays visible.

## Search (`/`)

`/` opens a prompt in the footer and narrows the table **as you type**, k9s
style. `enter` keeps the search and hands the keyboard back to the list, `esc`
clears it — from the prompt or from the list. The active search is shown in the
header, with the counts it hides: `search:"api" 3/57`.

It is a **client-side** filter: it matches the rows already fetched and never
calls GitHub, which is what makes it instantaneous. The flip side is that it
searches the fetched window — at most 50 open PRs per repo — so to look wider,
narrow the fetch itself from the filters panel (`since`, `author`, `label`,
`repo`).

The match is a case-insensitive substring over a fixed set of fields,
whether or not their column is visible:

- a PR: repo, number, title, author, branch and labels;
- a run: repo, number, workflow, branch, title and event;
- an issue: repo, number, title, author, assignees and labels;
- a repo: `owner/name` and description;
- a release: repo, tag and name.

It applies to the active tab, and it is deliberately **not** saved between
runs: a search is a lookup, not a setting.

## Auto-refresh

`a` cycles through `off → 1mn → 5mn → 10mn → 30mn → 1h → off`, and `A` turns
it off in one keystroke, whatever the current pace. It starts **off** the
first time, and the pace you leave it on is remembered between runs.

While it is on, the header carries the pace **and the time left** before the
next reload: `⟳ auto 5mn · 4:12`. It sits at `0:00` while a reload is running,
or while a prompt holds one back.

## Memory

gh-ui saves its settings in its own folder, `~/.config/gh-ui` on Linux and
`~/Library/Application Support/gh-ui` on macOS, on every change, and reloads
them at startup:

| File                | What                                              |
|---------------------|---------------------------------------------------|
| `filters.json`      | the PR filters                                    |
| `issuefilters.json` | the issue filters                                 |
| `repos.json`        | the Repos tab's owner and boxes                   |
| `columns.json`      | every tab's column order and hidden columns       |
| `refresh.json`      | the auto-refresh pace                             |

A column added by a future version is appended to your saved layout instead of
resetting it.

That memory is **global**, while the `repo` filter names a folder inside the
one you are scanning. So gh-ui checks it against what is actually there before
every load: a selection that this folder does not hold is dropped, and the
status line says which one (`repo "api" is not here, showing all`) instead of
leaving you with an empty table. The saved file is left untouched, so the
selection still applies in the folder you made it for.

## Development

```bash
cargo run                   # scan the current folder
cargo run -- ~/dev/workspace  # scan a specific folder
cargo test
cargo build --release
```

The GIF at the top is recorded by `./demo/record.sh` with
[vhs](https://github.com/charmbracelet/vhs): `demo/gh` stands in for `gh` and
answers from `demo/fixtures`, so the recording needs no network and shows no
real account. `./demo/record.sh --shell` opens the same setup in a shell.

### Architecture

| File            | Role                                                        |
|-----------------|-------------------------------------------------------------|
| `main.rs`       | event loop, keyboard routing, TUI setup/teardown            |
| `app.rs`        | application state and its logic                             |
| `model.rs`      | data structures (`Pr`, `Run`, deserialization of `gh` JSON) |
| `filters.rs`    | the PR filters: state, `gh` args, persistence               |
| `runfilters.rs` | the Actions filters, applied in memory                      |
| `issues.rs`     | the Issues tab: issue model, filters, linked PRs            |
| `repos.rs`      | the Repos tab: clone states, ticks, the clone batch         |
| `releases.rs`   | the Releases tab: the latest release of each repo           |
| `search.rs`     | the `/` search: what a row matches on                       |
| `columns.rs`    | table columns: registry, cells, order/visibility, persistence |
| `refresh.rs`    | the auto-refresh pace                                       |
| `config.rs`     | where the settings files live                               |
| `gh.rs`         | repo discovery, and every `gh` / `git` command              |
| `fetch.rs`      | background loading (threads + `mpsc` channel)               |
| `ui.rs`         | ratatui rendering                                           |

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your
option.
