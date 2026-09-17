# Design — "Actions" tab (GitHub runs) in gh-ui

Date: 2026-09-17
Status: validated (brainstorming), to review before the implementation plan

## 1. Objective

Add a 2nd tab to `gh-ui` displaying the **stream of GitHub Actions runs**
(like GitHub's Actions tab), multi-repo, with a "only the runs linked to my
PRs" filter **checked by default**.

The app thus moves from a "PR list" to a small tabbed **`gh` dashboard**,
without breaking what exists.

## 2. Decisions from the brainstorming

| # | Decision |
|---|----------|
| Q1 | View = **run stream** (all recent runs), not "CI health of main". |
| Q2 | **"only my PRs"** filter = dedicated toggle, **checked by default**. PR↔run link via the **branch** (run's `headBranch` ↔ PR's `headRefName`). |
| Q3 | Filter unchecked → **20 most recent runs per repo**, all branches, sorted by date. `gh run list --limit 20`. No status/date filter for now (YAGNI). |
| Q4 | Tab navigation: **`Tab`** cycles **+ `1`/`2`** jump. Tab bar at the top, active one highlighted. |
| Abstraction | Approach ②: **parallel typed state + enums**. `App` keeps `prs: Vec<Pr>` AND `runs: Vec<Run>`; an `enum Tab` picks the displayed one; an `enum Loaded` carries the thread's result. No trait, no genericity (reserved for a possible 3rd tab). |

## 3. Architecture (approach ②)

No genericity. We add two `enum`s as a state machine, and we double
the concrete paths (fetch + rendering) via `match tab`.

### 3.1 `model.rs`

- **`Pr`**: add the `head_ref_name: String` field (`#[serde(rename)]` via the
  `rename_all = "camelCase"` already present → `headRefName`). Used to know a
  PR's branch for the cross-referencing.
  - Add `headRefName` to `JSON_FIELDS` in `gh.rs`.
- **`Run`** (new struct, `#[serde(rename_all = "camelCase")]`):
  | Rust field | `gh run list` JSON field |
  |---|---|
  | `workflow_name: String` | `workflowName` |
  | `display_title: String` | `displayTitle` |
  | `head_branch: String` | `headBranch` |
  | `status: String` | `status` (`queued`/`in_progress`/`completed`) |
  | `conclusion: String` (`#[serde(default)]`) | `conclusion` (empty if not finished) |
  | `event: String` | `event` |
  | `created_at: String` | `createdAt` |
  | `number: u64` | `number` |
  | `url: String` | `url` |
  | `repo: String` (`#[serde(skip)]`) | filled in on the app side like for `Pr` |

### 3.2 `gh.rs`

- Add `fetch_runs(repo_dir: &Path) -> Result<Vec<Run>>`:
  `gh run list --limit 20 --json workflowName,displayTitle,headBranch,status,conclusion,event,createdAt,number,url`,
  parse with `serde_json::from_slice`. Same structure as `fetch_prs`.
- Dedicated constants: `RUN_LIMIT`, `RUN_JSON_FIELDS`.
- **The "my PRs" filter is NOT done here**: we fetch the 20 raw runs.
  The branch cross-referencing is done on the `App` side (see 3.5), so that a
  simple toggle does not require a re-fetch.

### 3.3 `fetch.rs`

The channel now carries two forms of result via an enum:

```rust
pub enum Loaded {
    Prs(FetchResult),   // existing struct
    Runs(RunsResult),   // new: { runs, all_repos, scanned, errors }
}
```

- `Sender<Loaded>` / `Receiver<Loaded>` in place of `Sender<FetchResult>`.
- The thread chooses what to do based on an **`enum Job { Prs, Runs }`** passed to
  `spawn(job, root, filters, tx)`. The `load` in `thread::scope` (parallel per
  repo) is reused: one variant calls `fetch_prs`, the other `fetch_runs`.
- `RunsResult` = mirror of `FetchResult` but with `runs: Vec<Run>`.

### 3.4 `app.rs` — state

Additions:

```rust
enum Tab { Prs, Runs }          // active tab

active_tab: Tab,                 // default Prs
runs: Vec<Run>,                  // all fetched runs (raw, unfiltered)
run_table_state: TableState,     // selection specific to the Actions tab
only_pr_runs: bool,              // default true
runs_loaded: bool,               // have we already loaded the runs?
```

The channel becomes `Sender/Receiver<Loaded>`.

### 3.5 `app.rs` — logic

- **`refresh()`** launches the fetch of **the active tab** (`Job::Prs` or
  `Job::Runs`). The `pending_refresh` flag and auto-refresh stay identical,
  applied to the current tab.
- **Tab change** (`set_tab`): switches `active_tab`; if the target tab
  has never been loaded (`!runs_loaded` for Actions), triggers a `refresh()`.
- **`on_tick()`**: `match` on the received `Loaded` → store into `prs` or `runs`,
  update the status and the corresponding `TableState`.
- **Filtered view of the runs** (derived, not stored):
  `visible_runs() -> Vec<&Run>`:
  - if `only_pr_runs`: keep only the runs whose `head_branch` ∈
    `{ pr.head_ref_name for pr in self.prs }` (a `HashSet<&str>` built on the
    fly);
  - otherwise: all `runs`.
  - The Actions tab's rendering and navigation rely on this view.
- **`toggle_only_pr_runs()`**: inverts the boolean. No re-fetch (purely local
  filtering). Readjusts the selection if it falls outside the view.
- Assumed dependency: the cross-referencing uses `self.prs`, filled at startup
  (`app.refresh()` loads the PRs before the TUI). If `prs` is empty, the
  filtered view is empty → we will show a hint ("no PR loaded"). Known
  limitation: we filter the 20 most recent runs/repo; if none is on a PR
  branch, the list may be empty (acceptable in v1; we can raise the limit
  later).

### 3.6 `ui.rs`

- **Tab bar**: a `[ PRs ] [ Actions ]` line, the active one highlighted
  (reversed/cyan), integrated into the header (grow the header block from 4 to 5
  lines or insert a dedicated line).
- **`render_table`**: `match app.active_tab` → PRs table (existing) or runs
  table.
- **`run_to_row(run: &Run) -> Row<'_>`**, columns:
  `repo · created(10) · workflow · branch · event · status · title(Fill)`.
  - `status` colored via `run_look(status, conclusion)`:
    | state | label | color |
    |---|---|---|
    | completed + success | `✓ success` | green |
    | completed + failure | `✗ failure` | red |
    | completed + cancelled/skipped | `cancelled`/`skipped` | dark gray |
    | in_progress | `● running` | yellow |
    | queued | `queued` | gray |
- **Footer**: add the tab indicator + the toggle key (see 3.7);
  the help (`?`) documents the new shortcuts.
- The header (filters summary) stays tied to the PRs tab; on the Actions tab,
  show the state of the "my PRs: [x]" toggle instead.

### 3.7 `main.rs` — keyboard

- **Global** (normal mode, all tabs):
  - `Tab` → next tab (cycle); `1` → PRs; `2` → Actions.
  - `r` refresh, `a` auto, `q`, `?`: unchanged (act on the active tab).
  - `Enter`: opens the URL of the selected item **of the active tab** (PR or
    run).
  - `j`/`k`/`↑`/`↓`: navigation in the active tab's table.
- **Actions tab only**: `m` → `toggle_only_pr_runs()` (mnemonic
  "mine"). On the PRs tab, `m` is ignored.
- The filters panel (`f`) stays **specific to PRs** in v1 (the Actions filter
  is limited to the `m` toggle). We don't make the panel tab-aware
  now (YAGNI).

## 4. Error handling

Same as for PRs: a repo whose `gh run list` fails increments `errors`;
the status shows "… — N failed". A run with no `conclusion` (not finished) is
normal, not an error.

## 5. Tests (unit, `#[cfg(test)]`)

- `run_look`: each (status, conclusion) → right label + right color.
- `visible_runs` filter: with `only_pr_runs = true`, keep only the runs
  whose branch is in the set of PR branches; `false` → everything.
- `Tab`: `Tab` cycle (Prs→Runs→Prs).
- Serde parsing of a `gh run list` JSON sample → `Vec<Run>` (fields well
  mapped, missing `conclusion` handled by `#[serde(default)]`).

## 6. Out of scope (YAGNI, later if needed)

- Status filter (`failure`/`in_progress`) and `since` filter on the runs.
- `Listable` trait / generic rendering (justified only once a 3rd tab is
  actually duplicated).
- Issues, Releases, Notifications tabs.
- Tab-aware filters panel / Actions filters in the panel.
- Server-side re-fetch by branch (`gh run list --branch …`).

## 7. Impact on existing code

- `Sender<FetchResult>` → `Sender<Loaded>`: touches `app.rs` and `fetch.rs`.
- `Pr` gains a field + `JSON_FIELDS` gains `headRefName`: backward-compatible.
- The rest (PR filters, persistence, panel, help) is unchanged.
