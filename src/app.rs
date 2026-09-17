//! The application state and its logic (independent of rendering).

use crate::fetch::{self, Job, Loaded};
use crate::filters::Filters;
use crate::model::{Pr, Run};
use ratatui::widgets::TableState;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

/// Interval of the auto-refresh when it is enabled.
const AUTO_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

/// Which text field we are currently entering (prompt mode).
#[derive(Clone, Copy, PartialEq)]
pub enum InputKind {
    Author,
    Label,
}

/// The rows of the filter panel, in display order.
#[derive(Clone, Copy, PartialEq)]
pub enum FilterField {
    Mode,
    Since,
    NoDraft,
    Unreviewed,
    NotMine,
    Repo,
    Author,
    Label,
}

/// The order of the fields in the panel (the cursor is an index into this array).
pub const FILTER_FIELDS: [FilterField; 8] = [
    FilterField::Mode,
    FilterField::Since,
    FilterField::NoDraft,
    FilterField::Unreviewed,
    FilterField::NotMine,
    FilterField::Repo,
    FilterField::Author,
    FilterField::Label,
];

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
    pub repos: Vec<String>,
    pending_refresh: bool,

    /// Auto-refresh: disabled by default (key `a`).
    pub auto_refresh: bool,
    /// Time of the last load start (to pace the auto-refresh).
    last_refresh: Instant,

    /// Input in progress (author/label): `None` = not in input mode.
    pub input_kind: Option<InputKind>,
    pub input_buffer: String,

    /// Filter panel open (key `f`).
    pub filter_panel_open: bool,
    /// Currently focused panel row (index into `FILTER_FIELDS`).
    pub filter_cursor: usize,

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
            repos: Vec::new(),
            pending_refresh: false,
            auto_refresh: false,
            last_refresh: Instant::now(),
            input_kind: None,
            input_buffer: String::new(),
            filter_panel_open: false,
            filter_cursor: 0,
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

    pub fn refresh(&mut self) {
        if self.loading {
            self.pending_refresh = true;
            return;
        }
        self.loading = true;
        self.pending_refresh = false;
        self.last_refresh = Instant::now();
        self.status = String::from("Loading…");
        let job = match self.active_tab {
            Tab::Prs => Job::Prs,
            Tab::Runs => Job::Runs,
        };
        fetch::spawn(
            job,
            self.root.clone(),
            self.filters.clone(),
            self.tx.clone(),
        );
    }

    /// Keeps the state alive on every loop iteration. Returns `true` if the
    /// display must be refreshed (a result arrived, or a load is animating the
    /// spinner) — to avoid redrawing a frozen screen 10×/s for nothing.
    pub fn on_tick(&mut self) -> bool {
        let mut changed = false;

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
            }
        }

        if !self.loading && self.pending_refresh {
            self.refresh();
        }

        // Auto-refresh: if enabled, no load is in progress and the interval has
        // elapsed, we relaunch.
        if self.auto_refresh
            && !self.loading
            && self.last_refresh.elapsed() >= AUTO_REFRESH_INTERVAL
        {
            self.refresh();
        }

        if self.loading {
            self.spinner_frame = self.spinner_frame.wrapping_add(1);
            changed = true; // the spinner is spinning → we must redraw
        }

        changed
    }

    // --- auto-refresh ---

    pub fn toggle_auto_refresh(&mut self) {
        self.auto_refresh = !self.auto_refresh;
        // On enabling, we refresh right away to start clean.
        if self.auto_refresh {
            self.refresh();
        }
    }

    // --- filter panel ---

    pub fn toggle_filter_panel(&mut self) {
        self.filter_panel_open = !self.filter_panel_open;
    }
    pub fn close_filter_panel(&mut self) {
        self.filter_panel_open = false;
    }

    /// Moves the panel cursor (clamped, without wrapping).
    pub fn filter_cursor_next(&mut self) {
        self.filter_cursor = (self.filter_cursor + 1).min(FILTER_FIELDS.len() - 1);
    }
    pub fn filter_cursor_prev(&mut self) {
        self.filter_cursor = self.filter_cursor.saturating_sub(1);
    }

    /// Changes the value of the focused field. `forward` = cycle direction (←/→).
    pub fn filter_change(&mut self, forward: bool) {
        match FILTER_FIELDS[self.filter_cursor] {
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
            // text fields are edited with Enter, not with ←/→
            FilterField::Author | FilterField::Label => return,
        }
        self.apply_filter_change();
    }

    /// Enter on the focused field: opens the input (text) or advances (others).
    pub fn filter_activate(&mut self) {
        match FILTER_FIELDS[self.filter_cursor] {
            FilterField::Author => self.start_input(InputKind::Author),
            FilterField::Label => self.start_input(InputKind::Label),
            _ => self.filter_change(true),
        }
    }

    fn apply_filter_change(&mut self) {
        self.filters.save();
        self.refresh();
    }

    // --- input mode (author / label) ---

    pub fn is_input_mode(&self) -> bool {
        self.input_kind.is_some()
    }

    /// Opens the prompt, pre-filled with the filter's current value.
    pub fn start_input(&mut self, kind: InputKind) {
        self.input_buffer = match kind {
            InputKind::Author => self.filters.author.clone().unwrap_or_default(),
            InputKind::Label => self.filters.labels.join(" "),
        };
        self.input_kind = Some(kind);
    }

    pub fn input_push(&mut self, c: char) {
        self.input_buffer.push(c);
    }
    pub fn input_backspace(&mut self) {
        self.input_buffer.pop();
    }
    pub fn input_cancel(&mut self) {
        self.input_kind = None;
        self.input_buffer.clear();
    }

    /// Commits the input: applies it to the matching filter, then reloads.
    pub fn input_commit(&mut self) {
        match self.input_kind {
            Some(InputKind::Author) => self.filters.set_author(&self.input_buffer),
            Some(InputKind::Label) => self.filters.set_labels(&self.input_buffer),
            None => return,
        }
        self.input_cancel();
        self.apply_filter_change();
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
            Tab::Prs => self.prs.len(),
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
                .and_then(|i| self.prs.get(i))
                .map(|pr| pr.url.clone()),
            Tab::Runs => self
                .run_table_state
                .selected()
                .and_then(|i| self.visible_runs().get(i).map(|r| r.url.clone())),
        }
    }

    // --- tabs ---

    pub fn set_tab(&mut self, tab: Tab) {
        self.active_tab = tab;
        // First visit to Actions -> load the runs.
        if tab == Tab::Runs && !self.runs_loaded {
            self.refresh();
        }
    }

    pub fn next_tab(&mut self) {
        self.set_tab(self.active_tab.next());
    }

    // --- runs view ---

    /// The displayed runs view: filtered on the PR branches if checked.
    pub fn visible_runs(&self) -> Vec<&Run> {
        let branches: HashSet<&str> = self.prs.iter().map(|p| p.head_ref_name.as_str()).collect();
        filter_runs(&self.runs, &branches, self.only_pr_runs)
    }

    /// Toggles the "my PRs" filter (no re-fetch: local filtering).
    pub fn toggle_only_pr_runs(&mut self) {
        self.only_pr_runs = !self.only_pr_runs;
        // The selection may fall outside the view -> reset it to the start if needed.
        let n = self.visible_runs().len();
        self.run_table_state
            .select(if n == 0 { None } else { Some(0) });
    }
}

/// Keeps the runs whose branch is in `pr_branches`, or all if `!only`.
fn filter_runs<'a>(runs: &'a [Run], pr_branches: &HashSet<&str>, only: bool) -> Vec<&'a Run> {
    if !only {
        return runs.iter().collect();
    }
    runs.iter()
        .filter(|r| pr_branches.contains(r.head_branch.as_str()))
        .collect()
}

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
