# "Actions" tab (GitHub runs) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a 2nd tab displaying the stream of GitHub Actions runs (multi-repo), with an "only my PRs" filter checked by default and tab-based navigation.

**Architecture:** "Parallel typed state" approach: `App` keeps `prs: Vec<Pr>` AND `runs: Vec<Run>`. An `enum Tab` picks the displayed tab; an `enum Loaded` carries the background thread's result through the existing mpsc channel; an `enum Job` tells the thread what to load. No genericity and no trait. The "my PRs" filter is a derived view (local filtering by branch), not a re-fetch.

**Tech Stack:** Rust 2024 edition, ratatui 0.30 + crossterm, serde/serde_json, anyhow, `std::thread::scope`, `std::sync::mpsc`, `std::collections::HashSet`.

**Spec:** `docs/superpowers/specs/2026-09-17-onglet-actions-design.md`

## Global Constraints

- Rust 2024 edition; no new dependency (everything is in std + existing deps).
- Comments and UI labels in **English**, pedagogical tone (learning project).
- `Run` follows exactly the style of `Pr`: `#[derive(Debug, Deserialize)]` + `#[serde(rename_all = "camelCase")]`, `repo` field as `#[serde(skip)]`.
- Each task ends up **compilable** (`cargo build`) with the existing tests green.
- Commit messages ended by the 2 attribution lines (see each "Commit" step).
- After the last task: `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test` must pass.

---

### Task 1: Branch field on `Pr`

**Files:**
- Modify: `src/model.rs` (struct `Pr`, + tests module)
- Modify: `src/gh.rs:14-15` (const `JSON_FIELDS`)

**Interfaces:**
- Produces: `Pr.head_ref_name: String` (the PR's branch, e.g. `feature/x`), used by the "my PRs" filter (Task 5).

- [ ] **Step 1: Write the failing test**

In `src/model.rs`, add at the bottom of the file:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_deserializes_head_ref_name() {
        let json = r#"{
            "number": 1, "title": "t", "author": {"login": "moi"},
            "isDraft": false, "url": "u", "updatedAt": "2026-01-01T00:00:00Z",
            "additions": 0, "deletions": 0, "labels": [],
            "headRefName": "feature/x"
        }"#;
        let pr: Pr = serde_json::from_str(json).unwrap();
        assert_eq!(pr.head_ref_name, "feature/x");
    }
}
```

- [ ] **Step 2: Run the test to see it fail**

Run: `cargo test pr_deserializes_head_ref_name`
Expected: FAIL at compilation — `no field head_ref_name on type Pr`.

- [ ] **Step 3: Add the field**

In `src/model.rs`, struct `Pr`, after `pub labels: Vec<Label>,` (before the `repo` field):

```rust
    // The PR's source branch (e.g. "feature/x"). Used to link a PR to its
    // GitHub Actions runs (we cross-reference on this branch in the Actions tab).
    pub head_ref_name: String,
```

- [ ] **Step 4: Request the field from `gh`**

In `src/gh.rs`, replace the const `JSON_FIELDS` (lines 14-15) with:

```rust
const JSON_FIELDS: &str =
    "number,title,author,reviewDecision,isDraft,url,updatedAt,additions,deletions,labels,headRefName";
```

- [ ] **Step 5: Run the test to see it pass**

Run: `cargo test pr_deserializes_head_ref_name`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add src/model.rs src/gh.rs
git commit -m "$(cat <<'EOF'
feat(model): add head_ref_name (branch) to Pr

Required to cross-reference a PR with its GitHub Actions runs.

EOF
)"
```

---

### Task 2: `Run` struct

**Files:**
- Modify: `src/model.rs` (new struct + test)

**Interfaces:**
- Produces: `struct Run { workflow_name, display_title, head_branch, status, conclusion, event, created_at, number, url: String/u64, repo: String }` — all `pub`. `conclusion` is `""` if the run is not finished.

- [ ] **Step 1: Write the failing test**

In the `mod tests` of `src/model.rs`, add:

```rust
    #[test]
    fn run_deserializes_and_defaults_conclusion() {
        // "conclusion" is absent as long as the run is not finished.
        let json = r#"{
            "workflowName": "CI", "displayTitle": "fixes a bug",
            "headBranch": "feature/x", "status": "in_progress",
            "event": "push", "createdAt": "2026-01-01T00:00:00Z",
            "number": 42, "url": "u"
        }"#;
        let run: Run = serde_json::from_str(json).unwrap();
        assert_eq!(run.workflow_name, "CI");
        assert_eq!(run.head_branch, "feature/x");
        assert_eq!(run.conclusion, ""); // default because absent
    }
```

- [ ] **Step 2: Run the test to see it fail**

Run: `cargo test run_deserializes`
Expected: FAIL at compilation — `cannot find type Run`.

- [ ] **Step 3: Add the struct**

In `src/model.rs`, after the `Pr` struct:

```rust
/// A GitHub Actions run, as `gh run list --json ...` returns it.
/// Same style as `Pr`: `repo` is filled in by us (not in the JSON).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub workflow_name: String,
    pub display_title: String,
    pub head_branch: String,
    /// "queued" | "in_progress" | "completed".
    pub status: String,
    /// "success" | "failure" | "cancelled" | "skipped"... ; "" if not finished.
    #[serde(default)]
    pub conclusion: String,
    /// "push" | "pull_request" | "schedule"...
    pub event: String,
    pub created_at: String,
    pub number: u64,
    pub url: String,
    #[serde(skip)]
    pub repo: String,
}
```

- [ ] **Step 4: Run the test to see it pass**

Run: `cargo test run_deserializes`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/model.rs
git commit -m "$(cat <<'EOF'
feat(model): add the Run struct (GitHub Actions run)

EOF
)"
```

---

### Task 3: `gh::fetch_runs`

**Files:**
- Modify: `src/gh.rs` (import `Run`, consts `RUN_LIMIT`/`RUN_JSON_FIELDS`, fn `fetch_runs`)

**Interfaces:**
- Consumes: `model::Run`.
- Produces: `pub fn fetch_runs(repo_dir: &Path) -> Result<Vec<Run>>` — runs `gh run list --limit 20 --json ...` in `repo_dir`.

- [ ] **Step 1: Widen the model import**

In `src/gh.rs`, line 5, replace:

```rust
use crate::model::Pr;
```

with:

```rust
use crate::model::{Pr, Run};
```

- [ ] **Step 2: Add the constants**

In `src/gh.rs`, after the const `JSON_FIELDS`:

```rust
/// Number of runs fetched per repo (most recent runs, all branches).
const RUN_LIMIT: &str = "20";

/// JSON fields requested from `gh` for each run.
const RUN_JSON_FIELDS: &str =
    "workflowName,displayTitle,headBranch,status,conclusion,event,createdAt,number,url";
```

- [ ] **Step 3: Add the function**

In `src/gh.rs`, after `fetch_prs`:

```rust
/// Runs `gh run list` in `repo_dir` and parses the JSON into `Vec<Run>`.
/// No filters here: we fetch the N most recent raw runs; the cross-referencing
/// with the PRs is done on the App side (see `App::visible_runs`).
pub fn fetch_runs(repo_dir: &Path) -> Result<Vec<Run>> {
    let output = Command::new("gh")
        .args([
            "run",
            "list",
            "--limit",
            RUN_LIMIT,
            "--json",
            RUN_JSON_FIELDS,
        ])
        .current_dir(repo_dir)
        .output()
        .context("launching `gh` (is it installed and in the PATH?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("`gh run list` failed: {}", stderr.trim());
    }

    let runs: Vec<Run> = serde_json::from_slice(&output.stdout)
        .context("parsing the JSON returned by `gh run list`")?;
    Ok(runs)
}
```

- [ ] **Step 4: Check compilation**

Run: `cargo build`
Expected: compiles ("never used function" warning tolerated — it will be wired up in Task 4).

- [ ] **Step 5: Commit**

```bash
git add src/gh.rs
git commit -m "$(cat <<'EOF'
feat(gh): add fetch_runs (gh run list --json)

EOF
)"
```

---

### Task 4: Generic `Loaded` channel + runs fetch layer

Refactor with no visible behavior change: the channel now carries a `enum Loaded`. The app keeps loading only the PRs; the runs layer exists but is not triggered yet.

**Files:**
- Modify: `src/fetch.rs` (import `Run`, `RunsResult`, `Loaded`, `Job`, rename `load`→`load_prs`, add `load_runs`, new `spawn` signature)
- Modify: `src/app.rs` (channel types → `Loaded`, `refresh` passes `Job::Prs`, `on_tick` does a `match`)

**Interfaces:**
- Produces:
  - `pub enum Job { Prs, Runs }`
  - `pub enum Loaded { Prs(FetchResult), Runs(RunsResult) }`
  - `pub struct RunsResult { runs: Vec<Run>, all_repos: Vec<String>, scanned: usize, errors: usize }`
  - `pub fn spawn(job: Job, root: PathBuf, filters: Filters, tx: Sender<Loaded>)`

- [ ] **Step 1: `fetch.rs` — imports and types**

In `src/fetch.rs`, widen the model import:

```rust
use crate::model::{Pr, Run};
```

and keep the channel import `use std::sync::mpsc::Sender;` (unchanged). Add, after the `FetchResult` struct:

```rust
/// Mirror of `FetchResult`, but for the runs.
pub struct RunsResult {
    pub runs: Vec<Run>,
    pub all_repos: Vec<String>,
    pub scanned: usize,
    pub errors: usize,
}

/// What the background thread returns: either PRs or runs.
pub enum Loaded {
    Prs(FetchResult),
    Runs(RunsResult),
}

/// What we ask the thread to load.
pub enum Job {
    Prs,
    Runs,
}
```

- [ ] **Step 2: `fetch.rs` — new `spawn` + dispatch**

Replace the `spawn` function with:

```rust
/// Starts loading `job` in a background thread.
pub fn spawn(job: Job, root: PathBuf, filters: Filters, tx: Sender<Loaded>) {
    thread::spawn(move || {
        let result = match job {
            Job::Prs => Loaded::Prs(load_prs(&root, &filters)),
            Job::Runs => Loaded::Runs(load_runs(&root, &filters)),
        };
        let _ = tx.send(result);
    });
}
```

- [ ] **Step 3: `fetch.rs` — rename `load` to `load_prs`**

Rename the existing `load` function to `load_prs` (signature and body unchanged, only the name changes).

- [ ] **Step 4: `fetch.rs` — add `load_runs`**

After `load_prs`, add the mirror for the runs (same `thread::scope` logic, but `gh::fetch_runs` takes no filters):

```rust
/// Like `load_prs`, but for the runs. The repo filter (if set)
/// also restricts the repos scanned here, for consistency with the PRs tab.
fn load_runs(root: &Path, filters: &Filters) -> RunsResult {
    let all_repos = gh::discover_repos(root).unwrap_or_default();

    let to_scan: Vec<&String> = match &filters.repo {
        Some(sel) => all_repos.iter().filter(|r| *r == sel).collect(),
        None => all_repos.iter().collect(),
    };

    let joined = thread::scope(|scope| {
        let handles: Vec<_> = to_scan
            .iter()
            .map(|&repo| scope.spawn(move || (repo.clone(), gh::fetch_runs(&root.join(repo)))))
            .collect();
        handles.into_iter().map(|h| h.join()).collect::<Vec<_>>()
    });

    let mut runs = Vec::new();
    let mut errors = 0;
    for result in joined {
        match result {
            Ok((repo, Ok(mut list))) => {
                for run in &mut list {
                    run.repo = repo.clone();
                }
                runs.extend(list);
            }
            Ok((_, Err(_))) | Err(_) => errors += 1,
        }
    }

    RunsResult {
        runs,
        scanned: to_scan.len(),
        errors,
        all_repos,
    }
}
```

- [ ] **Step 5: `app.rs` — channel types**

In `src/app.rs`, replace the import:

```rust
use crate::fetch::{self, FetchResult};
```

with:

```rust
use crate::fetch::{self, Job, Loaded};
```

and the two `App` fields:

```rust
    tx: Sender<Loaded>,
    rx: Receiver<Loaded>,
```

- [ ] **Step 6: `app.rs` — `refresh` passes a `Job`**

In `refresh`, replace the `fetch::spawn(...)` call with:

```rust
        fetch::spawn(Job::Prs, self.root.clone(), self.filters.clone(), self.tx.clone());
```

- [ ] **Step 7: `app.rs` — `on_tick` does a `match`**

In `on_tick`, replace the `while let Ok(result) = self.rx.try_recv() { ... }` loop with a `match` on `Loaded` (the PRs body is the existing one; the Runs arm is a placeholder filled in Task 5):

```rust
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Loaded::Prs(result) => {
                    self.prs = result.prs;
                    self.repos = result.all_repos;
                    self.loading = false;
                    changed = true;

                    let errors = if result.errors > 0 {
                        format!(" — {} failed", result.errors)
                    } else {
                        String::new()
                    };
                    self.status = format!(
                        "{} PR(s) — {} repo(s){}",
                        self.prs.len(),
                        result.scanned,
                        errors
                    );

                    self.table_state
                        .select(if self.prs.is_empty() { None } else { Some(0) });
                }
                Loaded::Runs(_) => {} // filled in Task 5
            }
        }
```

- [ ] **Step 8: Check build + existing tests**

Run: `cargo build && cargo test`
Expected: build OK; all existing tests (filters, model) PASS. Behavior identical to before (only the PRs are loaded).

- [ ] **Step 9: Commit**

```bash
git add src/fetch.rs src/app.rs
git commit -m "$(cat <<'EOF'
refactor(fetch): generic Loaded channel + runs fetch layer

The background thread now returns a Loaded enum (Prs|Runs) and takes a
Job. Behavior unchanged: only the PRs are still triggered.

EOF
)"
```

---

### Task 5: Tab state + "my PRs" filter (App logic)

**Files:**
- Modify: `src/app.rs` (enum `Tab`, new fields, `set_tab`/`next_tab`, `filter_runs`, `visible_runs`, `toggle_only_pr_runs`, tab-aware navigation, `selected_url`, `on_tick` Runs arm, per-tab `refresh`)

**Interfaces:**
- Consumes: `fetch::Job`, `fetch::Loaded`, `model::Run`.
- Produces (all `pub` except `filter_runs`):
  - `pub enum Tab { Prs, Runs }` + `pub fn next(self) -> Tab`
  - fields `active_tab: Tab`, `runs: Vec<Run>`, `run_table_state: TableState`, `only_pr_runs: bool`, `runs_loaded: bool`
  - `pub fn set_tab(&mut self, tab: Tab)`, `pub fn next_tab(&mut self)`
  - `pub fn visible_runs(&self) -> Vec<&Run>`
  - `pub fn toggle_only_pr_runs(&mut self)`
  - `pub fn selected_url(&self) -> Option<String>`
  - `next`/`previous` become tab-aware (act on the active tab)

- [ ] **Step 1: Write the failing tests**

In a `#[cfg(test)] mod tests` at the bottom of `src/app.rs` (create it if absent):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Run;

    fn run(branch: &str) -> Run {
        Run {
            workflow_name: "CI".into(),
            display_title: "t".into(),
            head_branch: branch.into(),
            status: "completed".into(),
            conclusion: "success".into(),
            event: "push".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            number: 1,
            url: "u".into(),
            repo: "r".into(),
        }
    }

    #[test]
    fn tab_cycle() {
        assert_eq!(Tab::Prs.next(), Tab::Runs);
        assert_eq!(Tab::Runs.next(), Tab::Prs);
    }

    #[test]
    fn filter_runs_by_pr_branches() {
        let runs = vec![run("feature/x"), run("main"), run("feature/y")];
        let mut branches = std::collections::HashSet::new();
        branches.insert("feature/x");
        branches.insert("feature/y");

        // only = true: keep only the runs on a PR branch.
        let kept = filter_runs(&runs, &branches, true);
        assert_eq!(kept.len(), 2);
        assert!(kept.iter().all(|r| r.head_branch != "main"));

        // only = false: keep everything.
        assert_eq!(filter_runs(&runs, &branches, false).len(), 3);
    }
}
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `cargo test --lib app::tests`
Expected: FAIL at compilation (`Tab`, `filter_runs` do not exist).

- [ ] **Step 3: Add `Tab` and the `HashSet` import**

At the top of `src/app.rs`, after the existing `use`s:

```rust
use std::collections::HashSet;
```

After the `FilterField` enum (or near the other enums):

```rust
/// The application's tabs.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tab {
    Prs,
    Runs,
}

impl Tab {
    /// The next tab (cycle Prs → Runs → Prs).
    pub fn next(self) -> Tab {
        match self {
            Tab::Prs => Tab::Runs,
            Tab::Runs => Tab::Prs,
        }
    }
}
```

- [ ] **Step 4: Add the fields to `App` + initialize them**

In the `App` struct, add:

```rust
    pub active_tab: Tab,
    pub runs: Vec<Run>,
    pub run_table_state: TableState,
    /// Filter "only the runs of my PRs' branches". Checked by default.
    pub only_pr_runs: bool,
    /// Have we already loaded the runs at least once?
    runs_loaded: bool,
```

Add the import `use crate::model::{Pr, Run};` (widen the existing `Pr` import). In `App::new`, initialize:

```rust
            active_tab: Tab::Prs,
            runs: Vec::new(),
            run_table_state: TableState::default(),
            only_pr_runs: true,
            runs_loaded: false,
```

- [ ] **Step 5: Add `filter_runs` + `visible_runs` + `toggle`**

At the bottom of `src/app.rs`, outside the `impl App` (a free, pure, testable function):

```rust
/// Keeps the runs whose branch is in `pr_branches`, or all if `!only`.
fn filter_runs<'a>(runs: &'a [Run], pr_branches: &HashSet<&str>, only: bool) -> Vec<&'a Run> {
    if !only {
        return runs.iter().collect();
    }
    runs.iter()
        .filter(|r| pr_branches.contains(r.head_branch.as_str()))
        .collect()
}
```

In `impl App`, add:

```rust
    /// The displayed runs view: filtered on the PR branches if checked.
    pub fn visible_runs(&self) -> Vec<&Run> {
        let branches: HashSet<&str> = self.prs.iter().map(|p| p.head_ref_name.as_str()).collect();
        filter_runs(&self.runs, &branches, self.only_pr_runs)
    }

    /// Toggles the "my PRs" filter (no re-fetch: local filtering).
    pub fn toggle_only_pr_runs(&mut self) {
        self.only_pr_runs = !self.only_pr_runs;
        // The selection may fall outside the view → reset it to the start if needed.
        let n = self.visible_runs().len();
        self.run_table_state
            .select(if n == 0 { None } else { Some(0) });
    }
```

- [ ] **Step 6: Tab navigation**

In `impl App`, add:

```rust
    pub fn set_tab(&mut self, tab: Tab) {
        self.active_tab = tab;
        // First visit to Actions → load the runs.
        if tab == Tab::Runs && !self.runs_loaded {
            self.refresh();
        }
    }

    pub fn next_tab(&mut self) {
        self.set_tab(self.active_tab.next());
    }
```

- [ ] **Step 7: `refresh` loads the active tab**

In `refresh`, replace the `fetch::spawn(Job::Prs, ...)` line with:

```rust
        let job = match self.active_tab {
            Tab::Prs => Job::Prs,
            Tab::Runs => Job::Runs,
        };
        fetch::spawn(job, self.root.clone(), self.filters.clone(), self.tx.clone());
```

- [ ] **Step 8: Fill the `Loaded::Runs` arm of `on_tick`**

In `on_tick`, replace `Loaded::Runs(_) => {}` with:

```rust
                Loaded::Runs(result) => {
                    self.runs = result.runs;
                    self.repos = result.all_repos;
                    self.runs_loaded = true;
                    self.loading = false;
                    changed = true;

                    let errors = if result.errors > 0 {
                        format!(" — {} failed", result.errors)
                    } else {
                        String::new()
                    };
                    let n = self.visible_runs().len();
                    self.status = format!("{n} run(s) — {} repo(s){}", result.scanned, errors);
                    self.run_table_state
                        .select(if n == 0 { None } else { Some(0) });
                }
```

- [ ] **Step 9: Tab-aware navigation + `selected_url`**

Replace `next`, `previous` and `selected_pr` with versions that take the active tab into account:

```rust
    pub fn next(&mut self) {
        let (state, len) = match self.active_tab {
            Tab::Prs => (&mut self.table_state, self.prs.len()),
            Tab::Runs => (&mut self.run_table_state, self.visible_runs().len()),
        };
        if len == 0 {
            return;
        }
        let i = match state.selected() {
            Some(i) => (i + 1).min(len - 1),
            None => 0,
        };
        state.select(Some(i));
    }

    pub fn previous(&mut self) {
        let (state, len) = match self.active_tab {
            Tab::Prs => (&mut self.table_state, self.prs.len()),
            Tab::Runs => (&mut self.run_table_state, self.visible_runs().len()),
        };
        if len == 0 {
            return;
        }
        let i = match state.selected() {
            Some(i) => i.saturating_sub(1),
            None => 0,
        };
        state.select(Some(i));
    }

    /// The URL of the selected item in the active tab (PR or run).
    pub fn selected_url(&self) -> Option<String> {
        match self.active_tab {
            Tab::Prs => self
                .table_state
                .selected()
                .and_then(|i| self.prs.get(i))
                .map(|pr| pr.url.clone()),
            Tab::Runs => self
                .run_table_state
                .selected()
                .and_then(|i| self.visible_runs().get(i).map(|r| r.url.clone())),
        }
    }
```

> Note: `selected_pr` is removed; `main.rs` (Task 7) will use `selected_url`.

- [ ] **Step 10: Run the tests**

Run: `cargo test --lib app::tests`
Expected: `tab_cycle` and `filter_runs_by_pr_branches` PASS.

- [ ] **Step 11: Check the global build**

Run: `cargo build`
Expected: probably fails in `ui.rs`/`main.rs` (`selected_pr` gone, no Runs tab displayed yet). **This is expected** — Tasks 6 and 7 fix it. If you want a clean commit point, commit only `app.rs` now (the lib tests pass in isolation with `cargo test --lib`).

- [ ] **Step 12: Commit**

```bash
git add src/app.rs
git commit -m "$(cat <<'EOF'
feat(app): tab state (Tab) + "my PRs" filter (runs)

Tab enum, runs/active_tab/only_pr_runs fields, tab-aware navigation,
visible_runs (local filtering by branch), selected_url.

EOF
)"
```

---

### Task 6: Rendering the Actions tab (UI)

**Files:**
- Modify: `src/ui.rs` (import `Run`/`Tab`, `run_look` + test, `run_to_row`, tab bar, `render_table` dispatch, header/footer)

**Interfaces:**
- Consumes: `App.active_tab`, `App.visible_runs()`, `App.run_table_state`, `App.only_pr_runs`.
- Produces: rendering of the runs table when `active_tab == Tab::Runs`; tab bar `[ PRs ] [ Actions ]`.

- [ ] **Step 1: Write the failing test**

In `src/ui.rs`, add a `#[cfg(test)] mod tests`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_look_colors() {
        assert_eq!(run_look("completed", "success").0, "✓ success");
        assert_eq!(run_look("completed", "failure").0, "✗ failure");
        assert_eq!(run_look("completed", "cancelled").0, "cancelled");
        assert_eq!(run_look("in_progress", "").0, "● running");
        assert_eq!(run_look("queued", "").0, "queued");
    }
}
```

- [ ] **Step 2: Run the test to see it fail**

Run: `cargo test --lib ui::tests`
Expected: FAIL at compilation (`run_look` does not exist).

- [ ] **Step 3: Widen the imports**

In `src/ui.rs`, add to the model import and the app import:

```rust
use crate::app::{App, FILTER_FIELDS, FilterField, InputKind, Tab};
use crate::model::{Pr, Run};
```

- [ ] **Step 4: Add `run_look` + `run_to_row`**

After `review_look` in `src/ui.rs`:

```rust
/// Label + color of a run based on (status, conclusion).
fn run_look(status: &str, conclusion: &str) -> (&'static str, Style) {
    match (status, conclusion) {
        ("completed", "success") => ("✓ success", Style::new().fg(Color::Green)),
        ("completed", "failure") => ("✗ failure", Style::new().fg(Color::Red)),
        ("completed", "cancelled") => ("cancelled", Style::new().fg(Color::DarkGray)),
        ("completed", "skipped") => ("skipped", Style::new().fg(Color::DarkGray)),
        ("in_progress", _) => ("● running", Style::new().fg(Color::Yellow)),
        ("queued", _) => ("queued", Style::new().fg(Color::Gray)),
        _ => ("-", Style::new().fg(Color::DarkGray)),
    }
}

fn run_to_row(run: &Run) -> Row<'_> {
    let date = run.created_at.get(..10).unwrap_or("").to_string();
    let (status_label, status_style) = run_look(&run.status, &run.conclusion);

    Row::new(vec![
        Cell::from(run.repo.clone()),
        Cell::from(date),
        Cell::from(run.workflow_name.clone()),
        Cell::from(run.head_branch.clone()),
        Cell::from(run.event.clone()),
        Cell::from(Span::styled(status_label, status_style)),
        Cell::from(run.display_title.clone()),
    ])
}
```

- [ ] **Step 5: Dispatch in `render_table`**

Replace the body of `render_table` to choose the table based on the tab. Extract the existing PRs one into `render_pr_table` and add `render_run_table`:

```rust
fn render_table(frame: &mut Frame, app: &mut App, area: Rect) {
    match app.active_tab {
        Tab::Prs => render_pr_table(frame, app, area),
        Tab::Runs => render_run_table(frame, app, area),
    }
}
```

Keep the current body of `render_table` under the name `render_pr_table` (identical signature). Add:

```rust
fn render_run_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let header = Row::new(["repo", "created", "workflow", "branch", "event", "status", "title"])
        .style(Style::new().bold());

    let runs = app.visible_runs();
    let rows: Vec<Row> = runs.iter().map(|r| run_to_row(r)).collect();

    let widths = [
        Constraint::Length(16),
        Constraint::Length(10),
        Constraint::Length(18),
        Constraint::Length(22),
        Constraint::Length(14),
        Constraint::Length(11),
        Constraint::Fill(1),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::new().reversed())
        .highlight_symbol("▌ ")
        .block(Block::bordered());

    frame.render_stateful_widget(table, area, &mut app.run_table_state);
}
```

- [ ] **Step 6: Tab bar in the header**

In `render_header`, add a tabs line. After building `top` and before `filters`, insert a `tabs` line and include it in the `Paragraph`. Replace the end of `render_header` with:

```rust
    // Tabs line: [ PRs ] [ Actions ], the active one highlighted.
    let tab_span = |label: &str, active: bool| {
        if active {
            Span::styled(
                format!(" {label} "),
                Style::new().fg(Color::Black).bg(Color::Cyan).bold(),
            )
        } else {
            Span::styled(format!(" {label} "), Style::new().fg(Color::DarkGray))
        }
    };
    let tabs = Line::from(vec![
        tab_span("PRs", app.active_tab == Tab::Prs),
        Span::raw(" "),
        tab_span("Actions", app.active_tab == Tab::Runs),
    ]);

    // Line 2: depending on the tab, PRs filters summary OR runs toggle state.
    let subtitle = match app.active_tab {
        Tab::Prs => Span::styled(app.filters.summary(), Style::new().fg(Color::DarkGray)),
        Tab::Runs => Span::styled(
            format!(
                "my PRs: {}   (m to toggle)",
                if app.only_pr_runs { "[x]" } else { "[ ]" }
            ),
            Style::new().fg(Color::DarkGray),
        ),
    };

    let header =
        Paragraph::new(vec![Line::from(top), tabs, Line::from(subtitle)]).block(Block::bordered());
    frame.render_widget(header, area);
```

Grow the header height from 4 to 5 lines: in `render`, `Constraint::Length(4)` → `Constraint::Length(5)`.

- [ ] **Step 7: Run the tests + build**

Run: `cargo test --lib ui::tests && cargo build`
Expected: `run_look_colors` PASS. The build may still fail on `main.rs` (`selected_pr`), fixed in Task 7.

- [ ] **Step 8: Commit**

```bash
git add src/ui.rs
git commit -m "$(cat <<'EOF'
feat(ui): runs table + tab bar + run_look

EOF
)"
```

---

### Task 7: Keyboard & final wiring (main.rs)

**Files:**
- Modify: `src/main.rs` (keys `Tab`/`1`/`2`/`m`, `open_selected` via `selected_url`)
- Modify: `src/ui.rs` (footer: tabs mention) and `render_help` (shortcuts)

**Interfaces:**
- Consumes: `App.next_tab`, `App.set_tab`, `App.toggle_only_pr_runs`, `App.selected_url`, `Tab`.

- [ ] **Step 1: Import `Tab` into main**

In `src/main.rs`, add to the app import:

```rust
use app::{App, Tab};
```

- [ ] **Step 2: Tab keys in normal mode**

In `handle_normal_key`, add the arms (before the `_ => {}`):

```rust
        KeyCode::Tab => app.next_tab(),
        KeyCode::Char('1') => app.set_tab(Tab::Prs),
        KeyCode::Char('2') => app.set_tab(Tab::Runs),
        KeyCode::Char('m') => app.toggle_only_pr_runs(),
```

> `m` only has a visible effect on the Actions tab; on PRs it toggles a boolean ignored by the rendering, with no consequence.

- [ ] **Step 3: `open_selected` via `selected_url`**

Replace `open_selected`:

```rust
fn open_selected(app: &App) {
    if let Some(url) = app.selected_url() {
        open_url(&url);
    }
}
```

- [ ] **Step 4: Footer + help**

In `src/ui.rs`, `render_footer`, add a tabs hint in the `hints` array (after `("↑↓", "nav")`):

```rust
        ("tab/1/2", "tab"),
```

In `render_help`, add after the `help_row("f", ...)` line:

```rust
        help_row("tab, 1/2", "switch tab (PRs / Actions)"),
        help_row("m", "Actions tab: filter on my PRs"),
```

- [ ] **Step 5: Build + lint + format + tests**

Run: `cargo build && cargo test && cargo clippy -- -D warnings && cargo fmt --check`
Expected: everything PASS, zero clippy warning, format OK. If `cargo fmt --check` reports diffs, run `cargo fmt` then re-check.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs src/ui.rs
git commit -m "$(cat <<'EOF'
feat(main): tab navigation (tab/1/2), toggle m, per-tab opening

Wires the Actions tab end to end: PRs/Actions switch, "my PRs" filter
(m), opening the selected PR or run.

EOF
)"
```

---

## Self-Review (done at writing time)

**Spec coverage:** §2 decisions → Task 1 (Q2 branch), Task 3 (Q3 limit 20), Task 5 (Q2 default toggle, Q4 nav), Task 6 (tab bar Q4), Task 7 (keys Q4 + m). §3.1 model → Tasks 1-2. §3.2 gh → Task 3. §3.3 fetch/Loaded/Job → Task 4. §3.4-3.5 state/logic → Task 5. §3.6 ui → Task 6. §3.7 keyboard → Task 7. §4 errors → errors arm in load_runs (Task 4) + on_tick (Task 5). §5 tests → Task 1/2 (serde), Task 5 (filter_runs, Tab), Task 6 (run_look). ✅ No gap.

**Placeholders:** the `Loaded::Runs(_) => {}` arm of Task 4 is deliberate and explicitly replaced in Task 5 (not an orphan TODO). No other.

**Type consistency:** `Loaded`/`Job`/`RunsResult` (Task 4) reused identically in Task 5. `filter_runs(&[Run], &HashSet<&str>, bool)` defined and tested in Task 5. `run_look(&str,&str)` defined and tested in Task 6. `selected_url` (Task 5) replaces `selected_pr` and is consumed in Task 7. ✅
