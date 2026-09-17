//! The application state and its logic (independent of rendering).

use crate::fetch::{self, Job, Loaded};
use crate::filters::Filters;
use crate::model::Pr;
use ratatui::widgets::TableState;
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
        fetch::spawn(
            Job::Prs,
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
                Loaded::Runs(_) => {} // filled in Task 5
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

    pub fn next(&mut self) {
        if self.prs.is_empty() {
            return;
        }
        let i = match self.table_state.selected() {
            Some(i) => (i + 1).min(self.prs.len() - 1),
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    pub fn previous(&mut self) {
        if self.prs.is_empty() {
            return;
        }
        let i = match self.table_state.selected() {
            Some(i) => i.saturating_sub(1),
            None => 0,
        };
        self.table_state.select(Some(i));
    }

    pub fn selected_pr(&self) -> Option<&Pr> {
        self.table_state.selected().and_then(|i| self.prs.get(i))
    }
}
