//! The application state and its logic (independent of rendering).

use crate::columns::Columns;
use crate::fetch::{self, FetchResult, Job, Loaded, RunsResult};
use crate::filters::Filters;
use crate::gh::RUN_DISPLAY_LIMIT;
use crate::model::{Pr, Run};
use crate::refresh::{AutoRefresh, RefreshSettings};
use crate::search;
use ratatui::widgets::TableState;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

/// The GitHub account shown in the header. Three states rather than an
/// `Option`: while the call is in flight the corner must stay empty, not flash
/// `@?` at every launch before settling on the real login.
#[derive(Debug, Clone, PartialEq)]
pub enum Login {
    Loading,
    Known(String),
    Unknown,
}

impl Login {
    /// What the header should print, or `None` while we are still asking.
    pub fn label(&self) -> Option<String> {
        match self {
            Login::Loading => None,
            Login::Known(name) => Some(format!("@{name}")),
            Login::Unknown => Some("@?".to_string()),
        }
    }
}

/// Which text field we are currently entering (prompt mode).
#[derive(Clone, Copy, PartialEq)]
pub enum InputKind {
    Author,
    Label,
    /// The `/` search. Unlike the two above it applies live, on every
    /// keystroke, and is never sent to `gh`.
    Search,
}

/// The rows of the filter panel, in display order.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FilterField {
    Mode,
    Since,
    NoDraft,
    Unreviewed,
    NotMine,
    Repo,
    Author,
    Label,
    /// Actions tab only: show the runs of my PRs' branches only.
    OnlyPrRuns,
}

/// Panel rows on the PRs tab. `Repo` comes first: it is the shared filter.
const PR_FIELDS: [FilterField; 8] = [
    FilterField::Repo,
    FilterField::Mode,
    FilterField::Since,
    FilterField::NoDraft,
    FilterField::Unreviewed,
    FilterField::NotMine,
    FilterField::Author,
    FilterField::Label,
];

/// Panel rows on the Actions tab: the shared filter, plus its own toggle.
const RUN_FIELDS: [FilterField; 2] = [FilterField::Repo, FilterField::OnlyPrRuns];

/// The panel rows for `tab`. The cursor is an index into THIS slice, so its
/// length changes with the tab (hence the clamping in `set_tab`).
pub fn fields_for(tab: Tab) -> &'static [FilterField] {
    match tab {
        Tab::Prs => &PR_FIELDS,
        Tab::Runs => &RUN_FIELDS,
    }
}

/// Does this filter feed BOTH flows? Today only the repo filter does: it
/// restricts the repos scanned by `gh pr list` AND by `gh run list`.
pub fn is_common(field: FilterField) -> bool {
    matches!(field, FilterField::Repo)
}

/// The section a field is displayed under, in the panel.
pub fn section_of(field: FilterField) -> &'static str {
    if is_common(field) {
        return "Common";
    }
    match field {
        FilterField::OnlyPrRuns => "Actions",
        _ => "PRs",
    }
}

/// The application's tabs.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tab {
    Prs,
    Runs,
}

impl Tab {
    /// The next tab (cycle Prs -> Runs -> Prs).
    pub fn next(self) -> Tab {
        match self {
            Tab::Prs => Tab::Runs,
            Tab::Runs => Tab::Prs,
        }
    }
}

pub struct App {
    pub root: PathBuf,
    pub prs: Vec<Pr>,
    pub table_state: TableState,
    pub status: String,
    pub loading: bool,
    pub spinner_frame: usize,
    pub should_quit: bool,

    pub filters: Filters,
    /// Column order and visibility, per tab (key `c`).
    pub columns: Columns,
    pub repos: Vec<String>,
    /// A refresh asked for while another one was still running (widened with
    /// `Job::merge` if several pile up).
    pending_job: Option<Job>,

    /// The authenticated GitHub account, resolved once at startup.
    pub login: Login,
    /// Auto-refresh pace, cycled with key `a`. `Off` = disabled.
    pub auto_refresh: AutoRefresh,
    /// Time of the last load start (to pace the auto-refresh).
    last_refresh: Instant,

    /// Input in progress (author/label): `None` = not in input mode.
    pub input_kind: Option<InputKind>,
    pub input_buffer: String,
    /// Free-text search (key `/`), applied client-side to the fetched rows.
    /// Empty = no search. Transient on purpose: never saved, never restored.
    pub search: String,

    /// Filter panel open (key `f`).
    pub filter_panel_open: bool,
    /// Currently focused panel row (index into `FILTER_FIELDS`).
    pub filter_cursor: usize,

    /// Column panel open (key `c`).
    pub column_panel_open: bool,
    /// Currently focused panel row (index into the active tab's entries).
    pub column_cursor: usize,
    /// Whether the column under the cursor is "grabbed" (key `space`): while
    /// true, `↑`/`↓` move that column instead of the cursor. There is no
    /// separate "which column" state — the grabbed column is always the one
    /// under `column_cursor`.
    pub column_grabbed: bool,

    /// Whether the help screen is shown (key `?`).
    pub show_help: bool,

    /// The currently displayed tab.
    pub active_tab: Tab,
    pub runs: Vec<Run>,
    pub run_table_state: TableState,
    /// Filter "only the runs of my PRs' branches". Checked by default.
    pub only_pr_runs: bool,
    /// Have we already loaded the runs at least once?
    runs_loaded: bool,

    tx: Sender<Loaded>,
    rx: Receiver<Loaded>,
}

impl App {
    pub fn new(root: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            root,
            prs: Vec::new(),
            table_state: TableState::default(),
            status: String::from("Starting…"),
            loading: false,
            spinner_frame: 0,
            should_quit: false,
            filters: Filters::load(),
            columns: Columns::load(),
            repos: Vec::new(),
            pending_job: None,
            login: Login::Loading,
            auto_refresh: RefreshSettings::load().auto_refresh,
            last_refresh: Instant::now(),
            input_kind: None,
            input_buffer: String::new(),
            search: String::new(),
            filter_panel_open: false,
            filter_cursor: 0,
            column_panel_open: false,
            column_cursor: 0,
            column_grabbed: false,
            show_help: false,
            active_tab: Tab::Prs,
            runs: Vec::new(),
            run_table_state: TableState::default(),
            only_pr_runs: true,
            runs_loaded: false,
            tx,
            rx,
        }
    }

    /// Reloads what the active tab shows.
    pub fn refresh(&mut self) {
        self.refresh_job(self.active_job());
    }

    /// The flow the active tab displays.
    fn active_job(&self) -> Job {
        match self.active_tab {
            Tab::Prs => Job::Prs,
            Tab::Runs => Job::Runs,
        }
    }

    /// Starts `job` in the background — or remembers it if a load is already in
    /// flight. Two jobs waiting are merged, never replaced: asking for the PRs
    /// then the runs must not lose the PRs.
    fn refresh_job(&mut self, job: Job) {
        if self.loading {
            self.pending_job = Some(match self.pending_job {
                Some(pending) => pending.merge(job),
                None => job,
            });
            return;
        }
        self.loading = true;
        self.pending_job = None;
        self.last_refresh = Instant::now();
        self.status = String::from("Loading…");
        fetch::spawn(
            job,
            self.root.clone(),
            self.filters.clone(),
            self.tx.clone(),
        );
    }

    /// Asks for the authenticated account. Called once, at startup.
    pub fn load_login(&mut self) {
        fetch::spawn_login(self.tx.clone());
    }

    /// Keeps the state alive on every loop iteration. Returns `true` if the
    /// display must be refreshed (a result arrived, or a load is animating the
    /// spinner) — to avoid redrawing a frozen screen 10×/s for nothing.
    pub fn on_tick(&mut self) -> bool {
        let mut changed = false;

        while let Ok(msg) = self.rx.try_recv() {
            // `None` = the message carries no status line, so it must leave
            // `status` and `loading` alone. Only the login does that: it shares
            // the channel without being a load, and clearing `loading` here
            // would cut short a fetch still in flight.
            let status = match msg {
                Loaded::Prs(result) => Some(self.apply_prs(result)),
                Loaded::Runs(result) => Some(self.apply_runs(result)),
                // PRs FIRST: `apply_runs` counts the visible runs, which are
                // cross-referenced against `self.prs`.
                Loaded::Both(prs, runs) => {
                    let left = self.apply_prs(prs);
                    let right = self.apply_runs(runs);
                    Some(format!("{left} · {right}"))
                }
                Loaded::User(login) => {
                    self.login = match login {
                        Some(name) => Login::Known(name),
                        None => Login::Unknown,
                    };
                    None
                }
            };
            if let Some(status) = status {
                self.status = status;
                self.loading = false;
            }
            changed = true;
        }

        if !self.loading
            && let Some(job) = self.pending_job
        {
            self.refresh_job(job);
        }

        // Auto-refresh: if a pace is set, no load is in progress and the
        // interval has elapsed, we relaunch.
        if let Some(interval) = self.auto_refresh.interval()
            && !self.loading
            && self.last_refresh.elapsed() >= interval
        {
            self.refresh();
        }

        if self.loading {
            self.spinner_frame = self.spinner_frame.wrapping_add(1);
            changed = true; // the spinner is spinning → we must redraw
        }

        changed
    }

    /// Stores a PR load and returns the status line describing it.
    fn apply_prs(&mut self, result: FetchResult) -> String {
        self.prs = result.prs;
        self.repos = result.all_repos;
        // The search survives a reload: the new rows go through it before the
        // selection is placed, so a refresh can never select a hidden row.
        self.reset_selection();
        format!(
            "{} PR(s) — {} repo(s){}",
            self.prs.len(),
            result.scanned,
            errors_suffix(result.errors)
        )
    }

    /// Same for a run load. Call it AFTER `apply_prs` when both arrive together.
    fn apply_runs(&mut self, result: RunsResult) -> String {
        self.runs = result.runs;
        self.repos = result.all_repos;
        self.runs_loaded = true;
        let n = self.visible_runs().len();
        self.reset_selection();
        format!(
            "{n} run(s) — {} repo(s){}",
            result.scanned,
            errors_suffix(result.errors)
        )
    }

    // --- auto-refresh ---

    /// Advances to the next pace (`off → 1mn → … → 1h → off`) and remembers it.
    pub fn cycle_auto_refresh(&mut self) {
        let was_off = self.auto_refresh == AutoRefresh::Off;
        self.auto_refresh = self.auto_refresh.next();
        RefreshSettings {
            auto_refresh: self.auto_refresh,
        }
        .save();
        // Leaving `Off` refreshes right away, to start clean. Moving from one
        // pace to another only changes the tempo: `last_refresh` is untouched,
        // so shortening the interval can make the next tick fire immediately.
        if was_off && self.auto_refresh != AutoRefresh::Off {
            self.refresh();
        }
    }

    /// Turns the auto-refresh off in one keystroke (key `A`), whatever the
    /// current pace — cycling all the way round with `a` would take up to five
    /// presses. A no-op when it is already off, so we skip the pointless save.
    pub fn disable_auto_refresh(&mut self) {
        if self.auto_refresh == AutoRefresh::Off {
            return;
        }
        self.auto_refresh = AutoRefresh::Off;
        RefreshSettings {
            auto_refresh: self.auto_refresh,
        }
        .save();
    }

    // --- filter panel ---

    pub fn toggle_filter_panel(&mut self) {
        self.filter_panel_open = !self.filter_panel_open;
    }
    pub fn close_filter_panel(&mut self) {
        self.filter_panel_open = false;
    }

    /// The panel rows of the active tab.
    pub fn active_fields(&self) -> &'static [FilterField] {
        fields_for(self.active_tab)
    }

    /// Moves the panel cursor (clamped, without wrapping).
    pub fn filter_cursor_next(&mut self) {
        self.filter_cursor = (self.filter_cursor + 1).min(self.active_fields().len() - 1);
    }
    pub fn filter_cursor_prev(&mut self) {
        self.filter_cursor = self.filter_cursor.saturating_sub(1);
    }

    /// Changes the value of the focused field. `forward` = cycle direction (←/→).
    pub fn filter_change(&mut self, forward: bool) {
        let field = self.active_fields()[self.filter_cursor];
        match field {
            FilterField::Mode => {
                if forward {
                    self.filters.cycle_filter();
                } else {
                    self.filters.cycle_filter_back();
                }
            }
            FilterField::Since => {
                if forward {
                    self.filters.cycle_since();
                } else {
                    self.filters.cycle_since_back();
                }
            }
            FilterField::NoDraft => self.filters.toggle_no_draft(),
            FilterField::Unreviewed => self.filters.toggle_unreviewed(),
            FilterField::NotMine => self.filters.toggle_not_mine(),
            FilterField::Repo => {
                if forward {
                    self.filters.cycle_repo(&self.repos);
                } else {
                    self.filters.cycle_repo_prev(&self.repos);
                }
            }
            // A view filter: local, so no re-fetch and nothing saved to disk.
            FilterField::OnlyPrRuns => {
                self.toggle_only_pr_runs();
                return;
            }
            // text fields are edited with Enter, not with ←/→
            FilterField::Author | FilterField::Label => return,
        }
        self.apply_filter_change(field);
    }

    /// Enter on the focused field: opens the input (text) or advances (others).
    pub fn filter_activate(&mut self) {
        match self.active_fields()[self.filter_cursor] {
            FilterField::Author => self.start_input(InputKind::Author),
            FilterField::Label => self.start_input(InputKind::Label),
            _ => self.filter_change(true),
        }
    }

    fn apply_filter_change(&mut self, field: FilterField) {
        self.filters.save();
        self.refresh_job(self.job_after_change(field));
    }

    /// What to reload after `field` changed. A common filter narrows both
    /// flows, so both must be reloaded: otherwise the tab we are not looking at
    /// keeps the previous scope — and on the Actions tab `visible_runs` would
    /// cross-reference PR branches that no longer match the filter.
    /// Exception: as long as the runs have never been loaded we stay lazy, the
    /// first visit to the Actions tab will fetch them with the current filters.
    fn job_after_change(&self, field: FilterField) -> Job {
        if is_common(field) && self.runs_loaded {
            Job::Both
        } else {
            self.active_job()
        }
    }

    // --- column panel ---

    pub fn toggle_column_panel(&mut self) {
        self.column_panel_open = !self.column_panel_open;
        // Always reopen at the top: the cursor means nothing while closed.
        self.column_cursor = 0;
        // A stale grab would let the next opening move columns when the user
        // expects to move the cursor.
        self.column_grabbed = false;
    }
    pub fn close_column_panel(&mut self) {
        self.column_panel_open = false;
        self.column_grabbed = false;
    }

    /// Number of rows in the panel = number of columns of the ACTIVE tab.
    fn column_count(&self) -> usize {
        match self.active_tab {
            Tab::Prs => self.columns.prs.entries.len(),
            Tab::Runs => self.columns.runs.entries.len(),
        }
    }

    /// Moves the panel cursor (clamped, without wrapping), like
    /// `filter_cursor_next` / `filter_cursor_prev` do.
    pub fn column_cursor_next(&mut self) {
        self.column_cursor = (self.column_cursor + 1).min(self.column_count() - 1);
    }
    pub fn column_cursor_prev(&mut self) {
        self.column_cursor = self.column_cursor.saturating_sub(1);
    }

    /// Shows/hides the focused column, then saves.
    pub fn column_toggle(&mut self) {
        let i = self.column_cursor;
        match self.active_tab {
            Tab::Prs => self.columns.prs.toggle(i),
            Tab::Runs => self.columns.runs.toggle(i),
        }
        self.columns.save();
    }

    /// Moves the focused column (`up` = towards the left of the table). The
    /// cursor follows the entry, so moves can be chained.
    pub fn column_move(&mut self, up: bool) {
        let i = self.column_cursor;
        self.column_cursor = match (self.active_tab, up) {
            (Tab::Prs, true) => self.columns.prs.move_up(i),
            (Tab::Prs, false) => self.columns.prs.move_down(i),
            (Tab::Runs, true) => self.columns.runs.move_up(i),
            (Tab::Runs, false) => self.columns.runs.move_down(i),
        };
        self.columns.save();
    }

    /// Grabs the column under the cursor, or drops it if it is already
    /// grabbed (key `space`). Grabbing/dropping changes no layout — unlike
    /// `column_toggle` / `column_move` it must NOT call `self.columns.save()`.
    pub fn column_grab_toggle(&mut self) {
        self.column_grabbed = !self.column_grabbed;
    }

    /// Drops a grabbed column without moving it (`enter` or `esc` while
    /// grabbed). Same "no save" rule as `column_grab_toggle`.
    pub fn column_drop(&mut self) {
        self.column_grabbed = false;
    }

    // --- search ---

    /// The PRs the table shows: the fetched list, narrowed by the search.
    /// The PRs tab's counterpart of `visible_runs`.
    pub fn visible_prs(&self) -> Vec<&Pr> {
        search::keep_prs(&self.prs, &self.search)
    }

    /// Opens the search prompt, pre-filled with the active query so it can be
    /// refined rather than retyped.
    pub fn start_search(&mut self) {
        self.start_input(InputKind::Search);
    }

    /// Drops the search (esc). A no-op when there is nothing to drop, so esc
    /// in normal mode costs nothing when no search is active.
    pub fn clear_search(&mut self) {
        if self.search.is_empty() {
            return;
        }
        self.search.clear();
        self.reset_selection();
    }

    /// Mirrors the prompt into the live query — k9s-style, the list narrows at
    /// every keystroke and there is nothing to confirm.
    fn sync_search(&mut self) {
        if self.input_kind != Some(InputKind::Search) {
            return;
        }
        self.search = self.input_buffer.trim().to_string();
        self.reset_selection();
    }

    /// Puts both selections back on the first VISIBLE row (or on nothing when
    /// the view is empty). Every path that changes what is visible goes
    /// through here — a load, a search edit, the Actions toggle — so the
    /// selection can never point at a row the filter just hid.
    fn reset_selection(&mut self) {
        let prs = self.visible_prs().len();
        self.table_state
            .select(if prs == 0 { None } else { Some(0) });
        let runs = self.visible_runs().len();
        self.run_table_state
            .select(if runs == 0 { None } else { Some(0) });
    }

    // --- input mode (author / label / search) ---

    pub fn is_input_mode(&self) -> bool {
        self.input_kind.is_some()
    }

    /// Opens the prompt, pre-filled with the filter's current value.
    pub fn start_input(&mut self, kind: InputKind) {
        self.input_buffer = match kind {
            InputKind::Author => self.filters.author.clone().unwrap_or_default(),
            InputKind::Label => self.filters.labels.join(" "),
            InputKind::Search => self.search.clone(),
        };
        self.input_kind = Some(kind);
    }

    pub fn input_push(&mut self, c: char) {
        self.input_buffer.push(c);
        self.sync_search();
    }
    pub fn input_backspace(&mut self) {
        self.input_buffer.pop();
        self.sync_search();
    }
    pub fn input_cancel(&mut self) {
        // Esc on the search prompt clears the search itself (k9s semantics): a
        // live filter that survived "cancel" would be a trap.
        if self.input_kind == Some(InputKind::Search) {
            self.search.clear();
            self.input_kind = None;
            self.input_buffer.clear();
            self.reset_selection();
            return;
        }
        self.input_kind = None;
        self.input_buffer.clear();
    }

    /// Commits the input: applies it to the matching filter, then reloads.
    pub fn input_commit(&mut self) {
        let field = match self.input_kind {
            // The search is applied keystroke by keystroke: enter only hands
            // the keyboard back to the list — no save, no refetch.
            Some(InputKind::Search) => {
                self.input_kind = None;
                self.input_buffer.clear();
                return;
            }
            Some(InputKind::Author) => {
                self.filters.set_author(&self.input_buffer);
                FilterField::Author
            }
            Some(InputKind::Label) => {
                self.filters.set_labels(&self.input_buffer);
                FilterField::Label
            }
            None => return,
        };
        self.input_cancel();
        self.apply_filter_change(field);
    }

    // --- help ---

    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    // --- navigation ---

    /// Moves the selection down, in the ACTIVE tab's list.
    pub fn next(&mut self) {
        // Two steps on purpose: `visible_runs()` borrows the whole `self`, so we
        // must be done with it BEFORE taking `&mut self.run_table_state`.
        let len = self.active_len();
        if len == 0 {
            return;
        }
        let state = self.active_table_state();
        let i = match state.selected() {
            Some(i) => (i + 1).min(len - 1),
            None => 0,
        };
        state.select(Some(i));
    }

    /// Moves the selection up, in the ACTIVE tab's list.
    pub fn previous(&mut self) {
        let len = self.active_len();
        if len == 0 {
            return;
        }
        let state = self.active_table_state();
        let i = match state.selected() {
            Some(i) => i.saturating_sub(1),
            None => 0,
        };
        state.select(Some(i));
    }

    /// Number of rows displayed in the active tab.
    fn active_len(&self) -> usize {
        match self.active_tab {
            Tab::Prs => self.visible_prs().len(),
            Tab::Runs => self.visible_runs().len(),
        }
    }

    /// The active tab's selection state (ratatui keeps the selected row there).
    fn active_table_state(&mut self) -> &mut TableState {
        match self.active_tab {
            Tab::Prs => &mut self.table_state,
            Tab::Runs => &mut self.run_table_state,
        }
    }

    /// The URL of the selected item in the active tab (PR or run).
    pub fn selected_url(&self) -> Option<String> {
        match self.active_tab {
            Tab::Prs => self
                .table_state
                .selected()
                .and_then(|i| self.visible_prs().get(i).map(|pr| pr.url.clone())),
            Tab::Runs => self
                .run_table_state
                .selected()
                .and_then(|i| self.visible_runs().get(i).map(|r| r.url.clone())),
        }
    }

    // --- tabs ---

    pub fn set_tab(&mut self, tab: Tab) {
        self.active_tab = tab;
        // The panel is shorter on Actions: keep the cursor inside the new slice.
        self.filter_cursor = self.filter_cursor.min(fields_for(tab).len() - 1);
        // Both column lists hold 8 entries, so a reset is enough here.
        self.column_cursor = 0;
        // A stale grab from the previous tab would move the new tab's
        // columns as soon as the user presses ↑/↓ again.
        self.column_grabbed = false;
        // First visit to Actions -> load the runs.
        if tab == Tab::Runs && !self.runs_loaded {
            self.refresh();
        }
    }

    pub fn next_tab(&mut self) {
        self.set_tab(self.active_tab.next());
    }

    // --- runs view ---

    /// The runs the Actions tab holds before the search: the fetched runs,
    /// narrowed by the "only my PRs' branches" toggle.
    pub fn branch_runs(&self) -> Vec<&Run> {
        let branches: HashSet<&str> = self.prs.iter().map(|p| p.head_ref_name.as_str()).collect();
        filter_runs(&self.runs, &branches, self.only_pr_runs)
    }

    /// What the Actions table shows: `branch_runs`, narrowed by the search.
    /// The two filters compose — branch first, then text.
    pub fn visible_runs(&self) -> Vec<&Run> {
        search::keep_runs(self.branch_runs(), &self.search)
    }

    /// Toggles the "my PRs" filter (no re-fetch: local filtering).
    pub fn toggle_only_pr_runs(&mut self) {
        self.only_pr_runs = !self.only_pr_runs;
        // The selection may fall outside the view -> put it back at the start.
        self.reset_selection();
    }
}

/// The " — N failed" tail of a status line, empty when nothing failed.
fn errors_suffix(errors: usize) -> String {
    if errors > 0 {
        format!(" — {errors} failed")
    } else {
        String::new()
    }
}

/// Keeps the runs whose branch is in `pr_branches`. When `!only`, keeps the
/// most recent `RUN_DISPLAY_LIMIT` runs of each repo instead, whatever their
/// branch: `fetch_runs` pulls a much wider window than that so the `only` pass
/// has something to cross-reference, but the unfiltered view stays the short
/// "what just ran here" list it has always been.
fn filter_runs<'a>(runs: &'a [Run], pr_branches: &HashSet<&str>, only: bool) -> Vec<&'a Run> {
    if !only {
        return most_recent_per_repo(runs, RUN_DISPLAY_LIMIT);
    }
    runs.iter()
        .filter(|r| pr_branches.contains(r.head_branch.as_str()))
        .collect()
}

/// The first `cap` runs of each repo. `runs` arrives grouped by repo and
/// ordered most recent first inside a repo (see `fetch::load_runs`), so taking
/// the first ones is taking the most recent ones.
fn most_recent_per_repo(runs: &[Run], cap: usize) -> Vec<&Run> {
    let mut kept: HashMap<&str, usize> = HashMap::new();
    runs.iter()
        .filter(|r| {
            let n = kept.entry(r.repo.as_str()).or_insert(0);
            *n += 1;
            *n <= cap
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Run;

    fn run(branch: &str) -> Run {
        run_in("r", branch)
    }

    fn run_in(repo: &str, branch: &str) -> Run {
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
            repo: repo.into(),
        }
    }

    #[test]
    fn the_header_stays_empty_until_the_login_answers() {
        // The whole point of the third state: no `@?` flash at startup.
        assert_eq!(Login::Loading.label(), None);
        assert_eq!(
            Login::Known("vincent".to_string()).label(),
            Some("@vincent".to_string())
        );
        assert_eq!(Login::Unknown.label(), Some("@?".to_string()));
    }

    #[test]
    fn fields_are_scoped_to_the_active_tab() {
        let prs = fields_for(Tab::Prs);
        let runs = fields_for(Tab::Runs);

        // Repo is the shared filter: first in both, under the "Common" section.
        assert_eq!(prs[0], FilterField::Repo);
        assert_eq!(runs[0], FilterField::Repo);

        // The Actions tab only exposes Repo + its own toggle.
        assert_eq!(runs, [FilterField::Repo, FilterField::OnlyPrRuns]);

        // The PR-only filters never show up on the Actions tab.
        assert!(prs.contains(&FilterField::Mode));
        assert!(!prs.contains(&FilterField::OnlyPrRuns));
    }

    #[test]
    fn section_groups_the_fields() {
        assert_eq!(section_of(FilterField::Repo), "Common");
        assert_eq!(section_of(FilterField::Mode), "PRs");
        assert_eq!(section_of(FilterField::OnlyPrRuns), "Actions");
    }

    #[test]
    fn switching_tab_clamps_the_filter_cursor() {
        let mut app = App::new(PathBuf::from("."));
        // Last row of the PRs panel (8 fields) …
        app.filter_cursor = fields_for(Tab::Prs).len() - 1;
        // … then switch to Actions, which only has 2. Without clamping the next
        // indexing of the fields array would panic.
        app.set_tab(Tab::Runs);
        assert!(app.filter_cursor < fields_for(Tab::Runs).len());
    }

    #[test]
    fn repo_is_the_only_common_filter() {
        assert!(is_common(FilterField::Repo));
        assert!(!is_common(FilterField::Mode));
        assert!(!is_common(FilterField::OnlyPrRuns));
    }

    #[test]
    fn merging_two_jobs_covers_both_flows() {
        assert_eq!(Job::Prs.merge(Job::Prs), Job::Prs);
        assert_eq!(Job::Runs.merge(Job::Runs), Job::Runs);
        // Asking for one then the other means "reload everything".
        assert_eq!(Job::Prs.merge(Job::Runs), Job::Both);
        assert_eq!(Job::Both.merge(Job::Prs), Job::Both);
    }

    #[test]
    fn a_common_filter_reloads_both_flows() {
        let mut app = App::new(PathBuf::from("."));
        app.runs_loaded = true;

        // A PR-only filter never touches the runs.
        assert_eq!(app.job_after_change(FilterField::Mode), Job::Prs);
        // The shared filter narrows BOTH flows: reload both, or the tab we are
        // not looking at keeps data from the previous scope.
        assert_eq!(app.job_after_change(FilterField::Repo), Job::Both);

        // Same from the Actions tab: `visible_runs` cross-references `prs`, so
        // stale PRs would show the wrong branches there too.
        app.active_tab = Tab::Runs;
        assert_eq!(app.job_after_change(FilterField::Repo), Job::Both);
    }

    #[test]
    fn a_common_filter_stays_lazy_until_the_runs_are_loaded() {
        let mut app = App::new(PathBuf::from("."));
        // Never visited the Actions tab: there is nothing to keep in sync yet,
        // the first visit will load the runs with the current filters.
        app.runs_loaded = false;
        assert_eq!(app.job_after_change(FilterField::Repo), Job::Prs);
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

    #[test]
    fn the_unfiltered_view_keeps_the_most_recent_runs_of_each_repo() {
        // `fetch_runs` pulls a wide window so the "my PRs" cross-reference has
        // something to work with. With the filter off the user still wants the
        // short list he has always had: the most recent runs of each repo.
        let mut runs: Vec<Run> = (0..30).map(|i| run_in("alpha", &format!("b{i}"))).collect();
        runs.extend((0..5).map(|i| run_in("beta", &format!("c{i}"))));

        let branches: HashSet<&str> = HashSet::new();
        let kept = filter_runs(&runs, &branches, false);

        assert_eq!(
            kept.iter().filter(|r| r.repo == "alpha").count(),
            RUN_DISPLAY_LIMIT
        );
        // A repo with fewer runs than the limit keeps all of them.
        assert_eq!(kept.iter().filter(|r| r.repo == "beta").count(), 5);
        // And the ones kept are the most recent, i.e. the first `gh` returned.
        assert_eq!(kept[0].head_branch, "b0");
        assert_eq!(kept[RUN_DISPLAY_LIMIT - 1].head_branch, "b19");
    }

    #[test]
    fn pr_runs_survive_a_base_branch_that_floods_the_window() {
        // The bug: a busy `develop` used to fill the whole fetch window, so the
        // cross-reference found nothing and the tab looked empty. The wider
        // window must reach the PR runs sitting behind that flood — and the
        // display limit must not cut them off again.
        let mut runs: Vec<Run> = (0..40).map(|_| run_in("alpha", "develop")).collect();
        runs.push(run_in("alpha", "feature/x"));

        let mut branches: HashSet<&str> = HashSet::new();
        branches.insert("feature/x");

        let kept = filter_runs(&runs, &branches, true);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].head_branch, "feature/x");
    }
}
