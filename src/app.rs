//! The application state and its logic (independent of rendering).

use crate::columns::Columns;
use crate::fetch::{self, FetchResult, IssuesResult, Job, Loaded, ReposResult, RunsResult};
use crate::filters::Filters;
use crate::gh::{self, RUN_DISPLAY_LIMIT};
use crate::issues::{Issue, IssueFilters, IssuesTab};
use crate::model::{Pr, Run};
use crate::refresh::{AutoRefresh, RefreshSettings};
use crate::repos::{self, CloneEvent, Repo, RepoFilters, RepoSettings, ReposTab};
use crate::runfilters::{self, RunFilters};
use crate::search;
use ratatui::widgets::TableState;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

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

/// A yes/no question in the footer. Holds the keyboard until answered.
#[derive(Debug, Clone, PartialEq)]
pub enum Confirm {
    /// Clone the ticked repos: how many, and into which folder.
    Clone { count: usize, into: String },
    /// Quit while a clone batch runs.
    Quit,
}

/// The rows of the filter panel, in display order.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FilterField {
    Since,
    NoDraft,
    Unreviewed,
    Repo,
    Author,
    /// PRs tab: `review-requested:@me`.
    ReviewAsked,
    Label,
    /// Actions tab only: show the runs of my PRs' branches only.
    OnlyPrRuns,
    /// Actions tab only: keep the runs that failed / are running / succeeded.
    RunStatus,
    /// Actions tab only: keep the runs triggered by one event.
    RunEvent,
    /// Actions tab only: keep the runs of one workflow.
    RunWorkflow,
    /// Repos tab only: whose repos to list (account or org).
    Owner,
    /// Repos tab only: show archived repos too.
    Archived,
    /// Repos tab only: show forks too.
    Forks,
    /// Repos tab only: hide the repos already cloned (in memory).
    HideCloned,
    /// Issues tab only: any / @me / nobody.
    Assignee,
}

/// Panel rows on the PRs tab. `Repo` comes first: it is the shared filter.
const PR_FIELDS: [FilterField; 7] = [
    FilterField::Repo,
    FilterField::Author,
    FilterField::ReviewAsked,
    FilterField::Since,
    FilterField::NoDraft,
    FilterField::Unreviewed,
    FilterField::Label,
];

/// Panel rows on the Actions tab: the shared filter, plus its own four.
const RUN_FIELDS: [FilterField; 5] = [
    FilterField::Repo,
    FilterField::OnlyPrRuns,
    FilterField::RunStatus,
    FilterField::RunEvent,
    FilterField::RunWorkflow,
];

/// Panel rows on the Repos tab. No `Repo` row: the folder filter means
/// nothing for a list of repos that are, mostly, not in the folder yet.
const REPO_FIELDS: [FilterField; 4] = [
    FilterField::Owner,
    FilterField::Archived,
    FilterField::Forks,
    FilterField::HideCloned,
];

/// Panel rows on the Issues tab. `Repo`, `Author`, `Since` and `Label` are
/// the PRs tab's rows, but here they edit the issue filters.
const ISSUE_FIELDS: [FilterField; 5] = [
    FilterField::Repo,
    FilterField::Author,
    FilterField::Assignee,
    FilterField::Since,
    FilterField::Label,
];

/// The panel rows for `tab`. The cursor is an index into THIS slice, so its
/// length changes with the tab (hence the clamping in `set_tab`).
pub fn fields_for(tab: Tab) -> &'static [FilterField] {
    match tab {
        Tab::Prs => &PR_FIELDS,
        Tab::Runs => &RUN_FIELDS,
        Tab::Repos => &REPO_FIELDS,
        Tab::Issues => &ISSUE_FIELDS,
    }
}

/// Does this filter feed BOTH flows? Today only the repo filter does: it
/// restricts the repos scanned by `gh pr list` AND by `gh run list`.
pub fn is_common(field: FilterField) -> bool {
    matches!(field, FilterField::Repo)
}

/// The section a field is displayed under, in the panel. The Issues tab
/// shares no filter with the others: one section.
pub fn section_of(tab: Tab, field: FilterField) -> &'static str {
    if tab == Tab::Issues {
        return "Issues";
    }
    if is_common(field) {
        return "Common";
    }
    match field {
        FilterField::OnlyPrRuns
        | FilterField::RunStatus
        | FilterField::RunEvent
        | FilterField::RunWorkflow => "Actions",
        FilterField::Owner
        | FilterField::Archived
        | FilterField::Forks
        | FilterField::HideCloned => "Repos",
        _ => "PRs",
    }
}

/// The application's tabs.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tab {
    Prs,
    Runs,
    Issues,
    Repos,
}

impl Tab {
    /// The next tab (cycle Prs -> Runs -> Issues -> Repos -> Prs).
    pub fn next(self) -> Tab {
        match self {
            Tab::Prs => Tab::Runs,
            Tab::Runs => Tab::Issues,
            Tab::Issues => Tab::Repos,
            Tab::Repos => Tab::Prs,
        }
    }

    /// Does this tab show the PR/run flow — and so its status line? The
    /// Repos and Issues tabs have lines of their own.
    pub fn shows_pr_flow(self) -> bool {
        matches!(self, Tab::Prs | Tab::Runs)
    }
}

/// The tab to open on: Repos when the folder holds no git repo — there is
/// nothing to show elsewhere, and cloning is what comes next.
pub fn initial_tab(root: &Path) -> Tab {
    if gh::discover_repos(root).unwrap_or_default().is_empty() {
        Tab::Repos
    } else {
        Tab::Prs
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
    /// Seconds the countdown showed on the previous tick, so we redraw once a
    /// second instead of at every one of the loop's ten ticks.
    last_countdown: Option<u64>,
    /// A one-shot note appended to the next status line — today, the repo
    /// filter we had to drop. Kept out of `status` so the message survives the
    /// "Loading…" that the reload writes over it.
    notice: Option<String>,

    /// Input in progress (author/label): `None` = not in input mode.
    pub input_kind: Option<InputKind>,
    pub input_buffer: String,
    /// A yes/no prompt waiting for its answer (clone, quit).
    pub confirm: Option<Confirm>,
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
    /// The Actions tab's own filters (branch toggle, status, event, workflow).
    /// Local to the view: never sent to `gh`, never saved to disk.
    pub run_filters: RunFilters,
    /// Have we already loaded the runs at least once?
    runs_loaded: bool,
    /// Have we already loaded the PRs at least once? False only when the app
    /// opened on the Repos tab: the PRs tab then loads on its first visit.
    prs_loaded: bool,

    /// The Repos tab: its list, filters, ticks and clone batch.
    pub repo_tab: ReposTab,
    pub repo_table_state: TableState,
    /// The Issues tab: its issues, filters, loading flag and status line.
    pub issue_tab: IssuesTab,
    pub issue_table_state: TableState,
    /// How many rows the table showed at the last draw: the jump of
    /// `PgUp`/`PgDn`. Written by `ui::render`, which alone knows the height.
    pub page_rows: usize,
    /// A repo-list load is in flight. Separate from `loading`: the list is
    /// not a `Job`, it never merges with the PR and run flows.
    repos_loading: bool,
    /// A repo-list reload asked for while one was in flight.
    repos_pending: bool,
    /// The status line of the PRs/Actions tabs while another tab is on
    /// screen: stashed when leaving them, replaced by any PR/run load
    /// landing meanwhile, restored on return.
    hidden_status: Option<String>,

    tx: Sender<Loaded>,
    rx: Receiver<Loaded>,
}

impl App {
    pub fn new(root: PathBuf) -> Self {
        let (tx, rx) = mpsc::channel();
        // Field by field: `ReposTab` keeps private counters, so it cannot be
        // built with `..Default::default()` from outside its module.
        let mut repo_tab = ReposTab::default();
        repo_tab.filters = RepoFilters {
            owner: RepoSettings::load().owner,
            ..Default::default()
        };
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
            last_countdown: None,
            notice: None,
            input_kind: None,
            input_buffer: String::new(),
            confirm: None,
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
            run_filters: RunFilters::default(),
            runs_loaded: false,
            prs_loaded: false,
            repo_tab,
            repo_table_state: TableState::default(),
            issue_tab: IssuesTab {
                filters: IssueFilters::load(),
                ..Default::default()
            },
            issue_table_state: TableState::default(),
            page_rows: 1,
            repos_loading: false,
            repos_pending: false,
            hidden_status: None,
            tx,
            rx,
        }
    }

    /// Reloads what the active tab shows.
    pub fn refresh(&mut self) {
        match self.active_tab {
            Tab::Repos => self.refresh_repos(),
            Tab::Issues => self.refresh_issues(),
            _ => self.refresh_job(self.active_job()),
        }
    }

    /// The flow the active tab displays — and, on the Repos and Issues tabs,
    /// the one the auto-refresh keeps fresh in the background: the repo list
    /// itself rarely changes and is never auto-reloaded.
    fn active_job(&self) -> Job {
        match self.active_tab {
            Tab::Prs => Job::Prs,
            Tab::Runs => Job::Runs,
            Tab::Repos | Tab::Issues => {
                if self.runs_loaded {
                    Job::Both
                } else {
                    Job::Prs
                }
            }
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
        self.reconcile_with_the_folder();
        // Behind the Repos or Issues tab the PRs reload in the background:
        // the line on screen is that tab's, and stays so.
        if self.active_tab.shows_pr_flow() {
            self.status = String::from("Loading…");
        }
        fetch::spawn(
            job,
            self.root.clone(),
            self.filters.clone(),
            self.tx.clone(),
        );
    }

    /// How long before the next automatic reload, or `None` when the
    /// auto-refresh is off. Saturating: a reload that is overdue — held back
    /// because a prompt is open, or still in flight — reads zero rather than
    /// wrapping around.
    pub fn time_to_refresh(&self) -> Option<Duration> {
        let interval = self.auto_refresh.interval()?;
        Some(interval.saturating_sub(self.last_refresh.elapsed()))
    }

    /// Re-reads which repos this root holds, and drops a repo filter that is
    /// not among them. Runs at the start of every load, BEFORE `gh` does: the
    /// filter names a FOLDER while the config is global, so a selection made
    /// in `~/dev/perso` would send every query to a repo that is not in
    /// `~/dev/client` and bring back an empty screen explaining nothing.
    ///
    /// `read_dir` is cheap enough to run on this thread, and the fetch redoes
    /// it anyway to build the list it returns.
    fn reconcile_with_the_folder(&mut self) {
        self.repos = gh::discover_repos(&self.root).unwrap_or_default();
        if let Some(dropped) = self.filters.reconcile_repo(&self.repos) {
            self.notice = Some(format!("repo \"{dropped}\" is not here, showing all"));
        }
    }

    /// Asks for the authenticated account. Called once, at startup.
    pub fn load_login(&mut self) {
        fetch::spawn_login(self.tx.clone());
    }

    /// Everything the first frame needs: the account, the orgs, and the
    /// first load of the tab we open on.
    pub fn start(&mut self) {
        self.load_login();
        fetch::spawn_orgs(self.tx.clone());
        match initial_tab(&self.root) {
            Tab::Repos => self.set_tab(Tab::Repos),
            _ => self.refresh(),
        }
    }

    /// Is anything loading? Drives the header's spinner.
    pub fn is_busy(&self) -> bool {
        self.loading || self.repos_loading || self.issue_tab.loading
    }

    /// Reloads the Repos tab's list — or queues it behind the one in flight,
    /// or explains why it cannot run when no owner is known yet.
    pub fn refresh_repos(&mut self) {
        let Some(owner) = self.repo_tab.filters.owner.clone() else {
            self.status = match self.login {
                Login::Unknown => "No GitHub account: cannot list repos".to_string(),
                _ => "Waiting for the GitHub account…".to_string(),
            };
            return;
        };
        if self.repos_loading {
            self.repos_pending = true;
            return;
        }
        self.repos_loading = true;
        self.repos_pending = false;
        self.status = format!("Loading {owner}'s repos…");
        fetch::spawn_repos(
            self.root.clone(),
            owner,
            self.repo_tab.filters.clone(),
            self.tx.clone(),
        );
    }

    /// Reloads the Issues tab — or queues it behind the load in flight. The
    /// repo filter is checked against the folder first, as for the PRs.
    pub fn refresh_issues(&mut self) {
        if self.issue_tab.loading {
            self.issue_tab.pending = true;
            return;
        }
        self.issue_tab.loading = true;
        self.issue_tab.pending = false;
        self.reconcile_issue_repo();
        self.issue_tab.status = String::from("Loading issues…");
        if self.active_tab == Tab::Issues {
            self.status = self.issue_tab.status.clone();
        }
        fetch::spawn_issues(
            self.root.clone(),
            self.issue_tab.filters.clone(),
            self.tx.clone(),
        );
    }

    /// `reconcile_with_the_folder` for the issue filters: re-reads the
    /// folder and drops an issue repo filter it does not hold. The note goes
    /// into the Issues tab's next line, not the PRs'.
    fn reconcile_issue_repo(&mut self) {
        self.repos = gh::discover_repos(&self.root).unwrap_or_default();
        if let Some(dropped) = self.issue_tab.filters.reconcile_repo(&self.repos) {
            self.issue_tab.notice = Some(format!("repo \"{dropped}\" is not here, showing all"));
        }
    }

    /// What the auto-refresh reloads: the flow the active tab shows (the
    /// PRs behind the Repos and Issues tabs), plus the issues once they have
    /// been loaded — whichever tab is on screen, as the runs are.
    fn background_reload(&mut self) {
        self.refresh_job(self.active_job());
        if self.issue_tab.loaded {
            self.refresh_issues();
        }
    }

    /// The owner picker's values, from what is known so far.
    pub fn owners(&self) -> Vec<String> {
        let login = match &self.login {
            Login::Known(name) => Some(name.as_str()),
            _ => None,
        };
        repos::owners(login, self.repo_tab.orgs.as_deref().unwrap_or(&[]))
    }

    /// Settles the owner as the account and the orgs come in: none picked
    /// yet → the account; a saved one the account no longer reaches (org
    /// left) → the account too, but only once the orgs are known. The
    /// fallback is not saved: a `gh org list` that failed for a moment must
    /// not erase the choice. Reloads the list when the tab is on screen.
    fn reconcile_owner(&mut self) {
        if !matches!(self.login, Login::Known(_)) {
            if self.active_tab == Tab::Repos && self.repo_tab.filters.owner.is_none() {
                self.refresh_repos(); // says why the tab stays empty
            }
            return;
        }
        let current = self.repo_tab.filters.owner.clone();
        if current.is_some() && self.repo_tab.orgs.is_none() {
            return;
        }
        let target = repos::valid_owner(current.as_deref(), &self.owners());
        if target != current
            && let Some(owner) = target
        {
            self.repo_tab.set_owner(owner);
            self.reset_repo_selection();
            if self.active_tab == Tab::Repos {
                self.refresh_repos();
            }
        }
    }

    /// Keeps the state alive on every loop iteration. Returns `true` if the
    /// display must be refreshed (a result arrived, or a load is animating the
    /// spinner) — to avoid redrawing a frozen screen 10×/s for nothing.
    pub fn on_tick(&mut self) -> bool {
        let mut changed = false;

        while let Ok(msg) = self.rx.try_recv() {
            // `None` = the message carries no PR/run status line, so it must
            // leave `status` and `loading` alone: the login, the orgs and the
            // repo list share the channel without being a PR/run load, and
            // clearing `loading` here would cut short a fetch still in flight.
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
                    self.reconcile_owner();
                    None
                }
                Loaded::Orgs(orgs) => {
                    self.repo_tab.orgs = Some(orgs);
                    self.reconcile_owner();
                    None
                }
                Loaded::Repos(result) => {
                    self.apply_repos(result);
                    None
                }
                Loaded::Clone(event) => {
                    self.apply_clone(event);
                    None
                }
                Loaded::Issues(result) => {
                    self.apply_issues(result);
                    None
                }
            };
            if let Some(status) = status {
                let status = match self.notice.take() {
                    Some(notice) => format!("{status} · {notice}"),
                    None => status,
                };
                // Behind the Repos or Issues tab a PR/run load is background
                // work: its line waits for the tabs it describes.
                if !self.active_tab.shows_pr_flow() {
                    self.hidden_status = Some(status);
                } else {
                    self.status = status;
                }
                self.loading = false;
            }
            changed = true;
        }

        if !self.loading
            && let Some(job) = self.pending_job
        {
            self.refresh_job(job);
        }
        if !self.repos_loading && self.repos_pending {
            self.refresh_repos();
        }
        if !self.issue_tab.loading && self.issue_tab.pending {
            self.refresh_issues();
        }

        // Auto-refresh: if a pace is set, no load is in progress, no prompt
        // (text or yes/no) is open and the interval has elapsed, we relaunch.
        // Holding it while the user types keeps a reload from resetting the
        // selection under their fingers — and `last_refresh` is deliberately
        // NOT touched here, so the reload fires on the first tick after the
        // prompt closes rather than skipping a beat.
        if let Some(interval) = self.auto_refresh.interval()
            && !self.loading
            && !self.is_prompt_open()
            && self.last_refresh.elapsed() >= interval
        {
            self.background_reload();
        }

        // The countdown runs down in the header: ask for a redraw when the
        // second it shows changes, and only then. Off means `None` on both
        // sides, so a disabled auto-refresh never wakes the screen up.
        let countdown = self.time_to_refresh().map(|left| left.as_secs());
        if countdown != self.last_countdown {
            self.last_countdown = countdown;
            changed = true;
        }

        if self.is_busy() {
            self.spinner_frame = self.spinner_frame.wrapping_add(1);
            changed = true; // the spinner is spinning → we must redraw
        }

        changed
    }

    /// Stores a PR load and returns the status line describing it.
    fn apply_prs(&mut self, result: FetchResult) -> String {
        self.prs = result.prs;
        self.prs_loaded = true;
        self.repos = result.all_repos;
        // The search survives a reload: the new rows go through it before the
        // selection is placed, so a refresh can never select a hidden row.
        // The runs are cross-referenced against the PRs: their view moves too.
        self.reset_pr_selection();
        self.reset_run_selection();
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
        self.reset_run_selection();
        format!(
            "{n} run(s) — {} repo(s){}",
            result.scanned,
            errors_suffix(result.errors)
        )
    }

    /// Stores a repo-list load. An answer for an owner we already left is
    /// dropped: the reload queued by that change is on its way.
    fn apply_repos(&mut self, result: ReposResult) {
        self.repos_loading = false;
        if self.repo_tab.filters.owner.as_deref() != Some(result.owner.as_str()) {
            return;
        }
        match result.repos {
            Ok(list) => {
                self.repo_tab.apply_load(list, result.locals);
                let failed = self.repo_tab.failed_count();
                self.status = format!(
                    "{} repo(s) — {} cloned{}",
                    self.repo_tab.repos.len(),
                    self.repo_tab.cloned_count(),
                    if failed > 0 {
                        format!(" — {failed} clone(s) failed")
                    } else {
                        String::new()
                    }
                );
            }
            Err(message) => {
                self.repo_tab.apply_load(Vec::new(), result.locals);
                self.status = format!("gh repo list failed: {message}");
            }
        }
        self.reset_repo_selection();
    }

    /// Stores an issue load. Its line belongs to the Issues tab: kept there,
    /// and shown now only if that tab is on screen.
    fn apply_issues(&mut self, result: IssuesResult) {
        self.repos = result.all_repos;
        self.issue_tab.apply_load(result.issues);
        let mut status = format!(
            "{} issue(s) — {} repo(s){}",
            self.issue_tab.issues.len(),
            result.scanned,
            errors_suffix(result.errors)
        );
        if let Some(notice) = self.issue_tab.notice.take() {
            status = format!("{status} · {notice}");
        }
        if self.active_tab == Tab::Issues {
            self.status = status.clone();
        }
        self.issue_tab.status = status;
        self.reset_issue_selection();
    }

    /// Applies one step of the clone batch. Its end brings new folders: the
    /// PRs (and runs, and issues, once loaded) pick them up, and the list
    /// recomputes its states from the folder.
    fn apply_clone(&mut self, event: CloneEvent) {
        let finished = event == CloneEvent::Finished;
        self.status = self.repo_tab.apply_event(event);
        if finished {
            self.refresh_job(if self.runs_loaded {
                Job::Both
            } else {
                Job::Prs
            });
            self.refresh_repos();
            // The new repos have issues too.
            if self.issue_tab.loaded {
                self.refresh_issues();
            }
        }
        self.clamp_repo_selection();
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
        if self.active_tab == Tab::Issues {
            self.issue_filter_change(field, forward);
            return;
        }
        match field {
            FilterField::Since => {
                if forward {
                    self.filters.cycle_since();
                } else {
                    self.filters.cycle_since_back();
                }
            }
            FilterField::NoDraft => self.filters.toggle_no_draft(),
            FilterField::Unreviewed => self.filters.toggle_unreviewed(),
            FilterField::ReviewAsked => self.filters.toggle_review_requested(),
            FilterField::Repo => {
                if forward {
                    self.filters.cycle_repo(&self.repos);
                } else {
                    self.filters.cycle_repo_prev(&self.repos);
                }
            }
            // View filters: local, so no re-fetch and nothing saved to disk.
            FilterField::OnlyPrRuns => {
                self.run_filters.toggle_only_pr_runs();
                self.reset_selection();
                return;
            }
            FilterField::RunStatus => {
                self.run_filters.cycle_status(forward);
                self.reset_selection();
                return;
            }
            FilterField::RunEvent => {
                let values = runfilters::events_of(&self.runs);
                self.run_filters.cycle_event(&values, forward);
                self.reset_selection();
                return;
            }
            FilterField::RunWorkflow => {
                let values = runfilters::workflows_of(&self.runs);
                self.run_filters.cycle_workflow(&values, forward);
                self.reset_selection();
                return;
            }
            FilterField::Owner => {
                let current = self.repo_tab.filters.owner.clone();
                if let Some(owner) = repos::cycle_owner(&self.owners(), current.as_deref(), forward)
                    && current.as_deref() != Some(owner.as_str())
                {
                    RepoSettings {
                        owner: Some(owner.clone()),
                    }
                    .save();
                    self.repo_tab.set_owner(owner);
                    self.reset_repo_selection();
                    self.refresh_repos();
                }
                return;
            }
            FilterField::Archived => {
                self.repo_tab.filters.archived = !self.repo_tab.filters.archived;
                self.refresh_repos();
                return;
            }
            FilterField::Forks => {
                self.repo_tab.filters.forks = !self.repo_tab.filters.forks;
                self.refresh_repos();
                return;
            }
            // In memory, like the Actions tab's filters: no reload.
            FilterField::HideCloned => {
                self.repo_tab.filters.hide_cloned = !self.repo_tab.filters.hide_cloned;
                self.reset_repo_selection();
                return;
            }
            FilterField::Author => {
                if forward {
                    self.filters.cycle_author();
                } else {
                    self.filters.cycle_author_back();
                }
            }
            // text field: edited with Enter, not with ←/→
            FilterField::Label => return,
            // Issues tab only: handled by `issue_filter_change`.
            FilterField::Assignee => return,
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

    /// `filter_change` on the Issues tab: the same rows as on the PRs tab,
    /// but they edit the issue filters, and reload the issues only.
    fn issue_filter_change(&mut self, field: FilterField, forward: bool) {
        // Two disjoint fields of `self`: borrowing one mutably and the
        // other immutably is allowed, as long as it is done field by field.
        let f = &mut self.issue_tab.filters;
        match field {
            FilterField::Repo => f.cycle_repo(&self.repos, forward),
            FilterField::Author => f.cycle_author(forward),
            FilterField::Assignee => f.cycle_assignee(forward),
            FilterField::Since => f.cycle_since(forward),
            // text field: edited with Enter, not with ←/→
            _ => return,
        }
        self.apply_issue_filter_change();
    }

    fn apply_issue_filter_change(&mut self) {
        self.issue_tab.filters.save();
        self.refresh_issues();
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
            Tab::Repos => self.columns.repos.entries.len(),
            Tab::Issues => self.columns.issues.entries.len(),
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
            Tab::Repos => self.columns.repos.toggle(i),
            Tab::Issues => self.columns.issues.toggle(i),
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
            (Tab::Repos, true) => self.columns.repos.move_up(i),
            (Tab::Repos, false) => self.columns.repos.move_down(i),
            (Tab::Issues, true) => self.columns.issues.move_up(i),
            (Tab::Issues, false) => self.columns.issues.move_down(i),
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

    /// The repos the Repos table shows: its list (minus the cloned ones if
    /// `hide cloned`), narrowed by the search.
    pub fn visible_repos(&self) -> Vec<&Repo> {
        search::keep_repos(self.repo_tab.listed(), &self.search)
    }

    /// The issues the Issues table shows: every loaded issue, narrowed by the
    /// search.
    pub fn visible_issues(&self) -> Vec<&Issue> {
        search::keep_issues(&self.issue_tab.issues, &self.search)
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
        self.reset_pr_selection();
        self.reset_run_selection();
        self.reset_repo_selection();
        self.reset_issue_selection();
    }

    /// The per-tab halves of `reset_selection`, for the paths that change
    /// one tab only: a PR load landing behind the Repos tab must not move
    /// the cursor the user is working with there.
    fn reset_pr_selection(&mut self) {
        let prs = self.visible_prs().len();
        self.table_state
            .select(if prs == 0 { None } else { Some(0) });
    }
    fn reset_run_selection(&mut self) {
        let runs = self.visible_runs().len();
        self.run_table_state
            .select(if runs == 0 { None } else { Some(0) });
    }
    fn reset_repo_selection(&mut self) {
        let repos = self.visible_repos().len();
        self.repo_table_state
            .select(if repos == 0 { None } else { Some(0) });
    }
    fn reset_issue_selection(&mut self) {
        let issues = self.visible_issues().len();
        self.issue_table_state
            .select(if issues == 0 { None } else { Some(0) });
    }

    // --- input mode (author / label / search) ---

    pub fn is_input_mode(&self) -> bool {
        self.input_kind.is_some()
    }

    /// Is any prompt open, text or yes/no? Holds the auto-refresh.
    pub fn is_prompt_open(&self) -> bool {
        self.input_kind.is_some() || self.confirm.is_some()
    }

    /// Opens the prompt, pre-filled with the filter's current value.
    pub fn start_input(&mut self, kind: InputKind) {
        let issues = self.active_tab == Tab::Issues;
        self.input_buffer = match kind {
            InputKind::Author if issues => self.issue_tab.filters.author.to_input(),
            InputKind::Label if issues => self.issue_tab.filters.labels.join(" "),
            InputKind::Author => self.filters.author.to_input(),
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
        let issues = self.active_tab == Tab::Issues;
        match self.input_kind {
            // The search is applied keystroke by keystroke: enter only hands
            // the keyboard back to the list — no save, no refetch.
            Some(InputKind::Search) => {
                self.input_kind = None;
                self.input_buffer.clear();
                return;
            }
            // The prompt opened from the Issues panel edits the issue filters.
            Some(InputKind::Author) if issues => {
                self.issue_tab.filters.set_author(&self.input_buffer)
            }
            Some(InputKind::Label) if issues => {
                self.issue_tab.filters.set_labels(&self.input_buffer)
            }
            Some(InputKind::Author) => self.filters.set_author(&self.input_buffer),
            Some(InputKind::Label) => self.filters.set_labels(&self.input_buffer),
            None => return,
        }
        let field = if self.input_kind == Some(InputKind::Author) {
            FilterField::Author
        } else {
            FilterField::Label
        };
        self.input_cancel();
        if issues {
            self.apply_issue_filter_change();
        } else {
            self.apply_filter_change(field);
        }
    }

    // --- help ---

    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    // --- navigation ---

    /// Moves the selection of the ACTIVE tab's list to the row `target`
    /// picks from the current one (`None` while nothing is selected), kept
    /// on the list: past the last row means the last row.
    fn move_selection(&mut self, target: impl FnOnce(Option<usize>) -> usize) {
        // Two steps on purpose: `visible_runs()` borrows the whole `self`, so we
        // must be done with it BEFORE taking `&mut self.run_table_state`.
        let len = self.active_len();
        if len == 0 {
            return;
        }
        let state = self.active_table_state();
        let i = target(state.selected()).min(len - 1);
        state.select(Some(i));
    }

    /// Moves the selection down, in the ACTIVE tab's list.
    pub fn next(&mut self) {
        self.move_selection(|i| i.map_or(0, |i| i + 1));
    }

    /// Moves the selection up, in the ACTIVE tab's list.
    pub fn previous(&mut self) {
        self.move_selection(|i| i.map_or(0, |i| i.saturating_sub(1)));
    }

    /// Moves the selection down by a screenful (`PgDn`).
    pub fn page_down(&mut self) {
        let page = self.page_rows;
        self.move_selection(|i| i.map_or(0, |i| i + page));
    }

    /// Moves the selection up by a screenful (`PgUp`).
    pub fn page_up(&mut self) {
        let page = self.page_rows;
        self.move_selection(|i| i.map_or(0, |i| i.saturating_sub(page)));
    }

    /// Selects the first row (`Home`).
    pub fn first(&mut self) {
        self.move_selection(|_| 0);
    }

    /// Selects the last row (`End`): `move_selection` brings any index past
    /// the end back onto the list.
    pub fn last(&mut self) {
        self.move_selection(|_| usize::MAX);
    }

    /// Number of rows displayed in the active tab.
    fn active_len(&self) -> usize {
        match self.active_tab {
            Tab::Prs => self.visible_prs().len(),
            Tab::Runs => self.visible_runs().len(),
            Tab::Repos => self.repo_rows(),
            Tab::Issues => self.visible_issues().len(),
        }
    }

    /// Rows of the Repos table: the visible repos, plus the clone button
    /// while it shows.
    fn repo_rows(&self) -> usize {
        self.visible_repos().len() + usize::from(self.repo_tab.show_button())
    }

    /// Keeps the Repos selection on a row that exists: the button can vanish
    /// under the cursor (batch started, last tick removed).
    fn clamp_repo_selection(&mut self) {
        let rows = self.repo_rows();
        if let Some(i) = self.repo_table_state.selected()
            && i >= rows
        {
            self.repo_table_state.select(rows.checked_sub(1));
        }
    }

    /// The active tab's selection state (ratatui keeps the selected row there).
    fn active_table_state(&mut self) -> &mut TableState {
        match self.active_tab {
            Tab::Prs => &mut self.table_state,
            Tab::Runs => &mut self.run_table_state,
            Tab::Repos => &mut self.repo_table_state,
            Tab::Issues => &mut self.issue_table_state,
        }
    }

    /// The URL of the selected item in the active tab (PR, run or issue).
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
            Tab::Repos => self
                .repo_table_state
                .selected()
                .and_then(|i| self.visible_repos().get(i).map(|r| r.url.clone())),
            Tab::Issues => self
                .issue_table_state
                .selected()
                .and_then(|i| self.visible_issues().get(i).map(|issue| issue.url.clone())),
        }
    }

    // --- tabs ---

    pub fn set_tab(&mut self, tab: Tab) {
        // Leaving PRs/Actions for a tab with its own line: keep theirs, so
        // coming back shows it rather than the other tab's.
        if self.active_tab.shows_pr_flow() && !tab.shows_pr_flow() {
            self.hidden_status = Some(self.status.clone());
        }
        self.active_tab = tab;
        // The panels differ in length: keep the cursor inside the new slice.
        self.filter_cursor = self.filter_cursor.min(fields_for(tab).len() - 1);
        // The tabs hold different column counts: back to the top.
        self.column_cursor = 0;
        // A stale grab from the previous tab would move the new tab's
        // columns as soon as the user presses ↑/↓ again.
        self.column_grabbed = false;
        // Back on PRs/Actions: their line, as it stands now.
        if tab.shows_pr_flow()
            && let Some(status) = self.hidden_status.take()
        {
            self.status = status;
        }
        // The Issues tab keeps its own line.
        if tab == Tab::Issues {
            self.status = self.issue_tab.status.clone();
        }
        // First visit to a tab -> load what it shows.
        match tab {
            Tab::Prs if !self.prs_loaded => self.refresh(),
            Tab::Runs if !self.runs_loaded => self.refresh(),
            Tab::Repos if !self.repo_tab.loaded && !self.repos_loading => self.refresh(),
            Tab::Issues if !self.issue_tab.loaded && !self.issue_tab.loading => self.refresh(),
            _ => {}
        }
    }

    pub fn next_tab(&mut self) {
        self.set_tab(self.active_tab.next());
    }

    // --- repos: ticks, clone, quit ---

    /// `space` on the Repos tab: ticks or unticks the repo under the cursor.
    /// A row that cannot be ticked says why in the status line.
    pub fn toggle_tick(&mut self) {
        if self.active_tab != Tab::Repos {
            return;
        }
        let Some(name) = self.repo_table_state.selected().and_then(|i| {
            self.visible_repos()
                .get(i)
                .map(|r| r.name_with_owner.clone())
        }) else {
            return;
        };
        if let Err(why) = self.repo_tab.toggle_tick(&name) {
            self.status = why;
        }
        self.clamp_repo_selection();
    }

    /// Is the cursor on the clone button (the row below the list)?
    pub fn on_clone_button(&self) -> bool {
        self.active_tab == Tab::Repos
            && self.repo_tab.show_button()
            && self.repo_table_state.selected() == Some(self.visible_repos().len())
    }

    /// `enter` on the clone button: asks before touching the disk.
    pub fn ask_clone(&mut self) {
        if !self.on_clone_button() {
            return;
        }
        let root = std::fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        let into = tilde(&root, dirs::home_dir().as_deref());
        self.confirm = Some(Confirm::Clone {
            count: self.repo_tab.ticked.len(),
            into,
        });
    }

    /// `y` on a prompt.
    pub fn confirm_yes(&mut self) {
        match self.confirm.take() {
            Some(Confirm::Clone { .. }) => self.start_clones(),
            Some(Confirm::Quit) => self.should_quit = true,
            None => {}
        }
    }

    /// `n` or `esc` on a prompt: nothing changes.
    pub fn confirm_no(&mut self) {
        self.confirm = None;
    }

    fn start_clones(&mut self) {
        let batch = self.repo_tab.start_batch();
        self.clamp_repo_selection();
        if batch.is_empty() {
            return;
        }
        self.status = format!("Cloning {} repo(s)…", batch.len());
        fetch::spawn_clones(self.root.clone(), batch, self.tx.clone());
    }

    /// `q`: quits, unless a clone batch runs — quitting then leaves a
    /// half-cloned folder behind, so it asks first.
    pub fn request_quit(&mut self) {
        if self.repo_tab.cloning {
            self.confirm = Some(Confirm::Quit);
        } else {
            self.should_quit = true;
        }
    }

    // --- runs view ---

    /// The runs the Actions tab holds before the search: the fetched runs,
    /// narrowed by the "only my PRs' branches" toggle.
    pub fn branch_runs(&self) -> Vec<&Run> {
        let branches: HashSet<&str> = self.prs.iter().map(|p| p.head_ref_name.as_str()).collect();
        runfilters::keep(&self.runs, &branches, &self.run_filters, RUN_DISPLAY_LIMIT)
    }

    /// What the Actions table shows: `branch_runs`, narrowed by the search.
    /// The two filters compose — branch first, then text.
    pub fn visible_runs(&self) -> Vec<&Run> {
        search::keep_runs(self.branch_runs(), &self.search)
    }

    /// The `m` shortcut, "mine" on either tab, which is why it lives here as
    /// well as in the panel. On Actions: the runs of my PRs' branches (no
    /// re-fetch, local filtering). On PRs: the `me` mode, which does go
    /// through `gh` again, exactly like changing it from the panel.
    pub fn toggle_mine(&mut self) {
        match self.active_tab {
            Tab::Runs => {
                self.run_filters.toggle_only_pr_runs();
                // The selection may fall outside the view -> put it back at the start.
                self.reset_selection();
            }
            Tab::Prs => {
                self.filters.toggle_mine();
                self.apply_filter_change(FilterField::Author);
            }
            // Nothing is "mine" in a list of repos to clone.
            Tab::Repos => {}
            // Task 6 wires the Issues tab's own "mine".
            Tab::Issues => {}
        }
    }
}

/// `path` with the home folder written `~`: the absolute form of a folder
/// under `$HOME` is long enough to push the prompt's `(y/n)` off screen.
fn tilde(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filters::AuthorFilter;
    use crate::issues::{AssigneeFilter, sample_issue};
    use crate::repos::{CloneEvent, LocalRepo, sample_repo};

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

        // The Actions tab exposes Repo + its own four view filters.
        assert_eq!(
            runs,
            [
                FilterField::Repo,
                FilterField::OnlyPrRuns,
                FilterField::RunStatus,
                FilterField::RunEvent,
                FilterField::RunWorkflow,
            ]
        );

        // The PR-only filters never show up on the Actions tab.
        assert!(prs.contains(&FilterField::Author));
        assert!(!prs.contains(&FilterField::OnlyPrRuns));
    }

    #[test]
    fn section_groups_the_fields() {
        assert_eq!(section_of(Tab::Prs, FilterField::Repo), "Common");
        assert_eq!(section_of(Tab::Prs, FilterField::Author), "PRs");
        assert_eq!(section_of(Tab::Runs, FilterField::OnlyPrRuns), "Actions");
        assert_eq!(section_of(Tab::Runs, FilterField::RunStatus), "Actions");
        assert_eq!(section_of(Tab::Runs, FilterField::RunWorkflow), "Actions");
    }

    #[test]
    fn switching_tab_clamps_the_filter_cursor() {
        let mut app = App::new(PathBuf::from("."));
        // Last row of the PRs panel (7 fields) …
        app.filter_cursor = fields_for(Tab::Prs).len() - 1;
        // … then switch to Actions, which only has 2. Without clamping the next
        // indexing of the fields array would panic.
        app.set_tab(Tab::Runs);
        assert!(app.filter_cursor < fields_for(Tab::Runs).len());
    }

    #[test]
    fn repo_is_the_only_common_filter() {
        assert!(is_common(FilterField::Repo));
        assert!(!is_common(FilterField::Author));
        assert!(!is_common(FilterField::OnlyPrRuns));
        assert!(!is_common(FilterField::RunStatus));
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
        assert_eq!(app.job_after_change(FilterField::Author), Job::Prs);
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
    fn m_on_the_prs_tab_toggles_my_prs_and_reloads_them() {
        let mut app = App::new(PathBuf::from("."));
        // A load "in flight" makes `refresh_job` queue the job instead of
        // spawning `gh`: we can see what would be reloaded without running it.
        app.loading = true;

        app.toggle_mine();

        assert_eq!(app.filters.author, AuthorFilter::me());
        assert_eq!(app.pending_job, Some(Job::Prs));
    }

    #[test]
    fn m_on_the_actions_tab_toggles_the_run_filter() {
        let mut app = App::new(PathBuf::from("."));
        app.active_tab = Tab::Runs;
        let prs_filters = app.filters.clone();
        let before = app.run_filters.only_pr_runs;

        app.toggle_mine();

        assert_eq!(app.run_filters.only_pr_runs, !before);
        assert_eq!(app.filters, prs_filters, "the PR filters must not move");
    }

    #[test]
    fn tab_cycle() {
        assert_eq!(Tab::Prs.next(), Tab::Runs);
        assert_eq!(Tab::Runs.next(), Tab::Issues);
        assert_eq!(Tab::Issues.next(), Tab::Repos);
        assert_eq!(Tab::Repos.next(), Tab::Prs);
    }

    #[test]
    fn there_is_no_countdown_without_a_pace() {
        let mut app = App::new(PathBuf::from("."));
        app.auto_refresh = AutoRefresh::Off;
        assert_eq!(app.time_to_refresh(), None);
    }

    #[test]
    fn the_countdown_starts_at_a_full_interval() {
        let mut app = App::new(PathBuf::from("."));
        app.auto_refresh = AutoRefresh::M1;
        app.last_refresh = Instant::now();

        let left = app.time_to_refresh().expect("a pace is set");
        assert!(
            left > Duration::from_secs(59) && left <= Duration::from_secs(60),
            "expected about a minute left, got {left:?}"
        );
    }

    /// A reload held back (input mode) or still in flight leaves the interval
    /// behind: the countdown must sit at zero, not wrap around.
    #[test]
    fn an_overdue_countdown_saturates_at_zero() {
        let mut app = App::new(PathBuf::from("."));
        app.auto_refresh = AutoRefresh::M1;
        app.last_refresh = Instant::now() - Duration::from_secs(90);

        assert_eq!(app.time_to_refresh(), Some(Duration::ZERO));
    }

    /// The plumbing, on a real folder: a selection saved somewhere else must
    /// not survive the start of a load. The folder holds `web` and nothing
    /// named `ghost`, so the filter has to go — and say so.
    #[test]
    fn reconciling_drops_a_repo_the_folder_does_not_hold() {
        let root = std::env::temp_dir().join("gh-ui-reconcile-drops");
        std::fs::create_dir_all(root.join("web").join(".git")).unwrap();

        let mut app = App::new(root.clone());
        app.filters.repo = Some("ghost".to_string());

        app.reconcile_with_the_folder();

        assert_eq!(app.repos, vec!["web".to_string()]);
        assert_eq!(
            app.filters.repo, None,
            "the stale selection must be dropped"
        );
        assert!(
            app.notice.as_deref().is_some_and(|n| n.contains("ghost")),
            "the status line must name the filter it dropped, got {:?}",
            app.notice
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reconciling_keeps_a_repo_the_folder_holds() {
        let root = std::env::temp_dir().join("gh-ui-reconcile-keeps");
        std::fs::create_dir_all(root.join("web").join(".git")).unwrap();

        let mut app = App::new(root.clone());
        app.filters.repo = Some("web".to_string());

        app.reconcile_with_the_folder();

        assert_eq!(app.filters.repo, Some("web".to_string()));
        assert_eq!(app.notice, None, "nothing was dropped, nothing to report");

        std::fs::remove_dir_all(&root).ok();
    }

    /// An `App` with the PRs panel cursor on `field`, and a load "in flight"
    /// so a filter change queues its reload instead of spawning `gh`.
    fn app_on(field: FilterField) -> App {
        let mut app = App::new(PathBuf::from("."));
        app.loading = true;
        app.filter_cursor = fields_for(Tab::Prs)
            .iter()
            .position(|f| *f == field)
            .expect("a PRs panel row");
        app
    }

    #[test]
    fn the_prs_panel_rows_in_order() {
        assert_eq!(
            fields_for(Tab::Prs),
            [
                FilterField::Repo,
                FilterField::Author,
                FilterField::ReviewAsked,
                FilterField::Since,
                FilterField::NoDraft,
                FilterField::Unreviewed,
                FilterField::Label,
            ]
        );
    }

    #[test]
    fn review_asked_toggles_and_reloads_the_prs() {
        let mut app = app_on(FilterField::ReviewAsked);
        app.filter_change(true);
        assert!(app.filters.review_requested);
        assert_eq!(app.pending_job, Some(Job::Prs));
    }

    #[test]
    fn enter_on_author_prefills_then_parses_the_prompt() {
        let mut app = app_on(FilterField::Author);
        app.filters.author = AuthorFilter::not_me();

        app.filter_activate();
        assert_eq!(app.input_buffer, "-@me");

        app.input_buffer = "-octocat".to_string();
        app.input_commit();
        assert_eq!(
            app.filters.author,
            AuthorFilter::IsNot("octocat".to_string())
        );
        assert_eq!(app.pending_job, Some(Job::Prs));
    }

    #[test]
    fn arrows_on_author_cycle_it_and_reload_the_prs() {
        let mut app = app_on(FilterField::Author);

        app.filter_change(true);
        assert_eq!(app.filters.author, AuthorFilter::me());
        assert_eq!(app.pending_job, Some(Job::Prs));

        app.filter_change(false);
        assert_eq!(app.filters.author, AuthorFilter::Any);
    }

    /// An `App` on the Repos tab, owner `acme`, with a list load "in
    /// flight" so any reload queues (`repos_pending`) instead of running `gh`.
    fn repos_app() -> App {
        let mut app = App::new(PathBuf::from("."));
        app.active_tab = Tab::Repos;
        app.repo_tab.filters.owner = Some("acme".to_string());
        app.repos_loading = true;
        app
    }

    fn repos_answer(owner: &str, repos: &[&str]) -> Loaded {
        Loaded::Repos(ReposResult {
            owner: owner.to_string(),
            repos: Ok(repos.iter().map(|r| sample_repo(r)).collect()),
            locals: Vec::new(),
        })
    }

    #[test]
    fn the_repos_panel_rows_in_order() {
        assert_eq!(
            fields_for(Tab::Repos),
            [
                FilterField::Owner,
                FilterField::Archived,
                FilterField::Forks,
                FilterField::HideCloned,
            ]
        );
        assert_eq!(section_of(Tab::Repos, FilterField::Owner), "Repos");
        assert_eq!(section_of(Tab::Repos, FilterField::HideCloned), "Repos");
    }

    #[test]
    fn switching_to_repos_clamps_the_filter_cursor() {
        let mut app = App::new(PathBuf::from("."));
        app.repos_loading = true;
        app.filter_cursor = fields_for(Tab::Prs).len() - 1;
        app.set_tab(Tab::Repos);
        assert!(app.filter_cursor < fields_for(Tab::Repos).len());
    }

    #[test]
    fn a_list_answer_fills_the_tab() {
        let mut app = repos_app();
        app.tx
            .send(repos_answer("acme", &["acme/api", "acme/web"]))
            .unwrap();
        app.on_tick();

        assert!(!app.repos_loading);
        assert_eq!(app.visible_repos().len(), 2);
        assert!(app.status.contains("2 repo(s)"), "got {:?}", app.status);
        assert_eq!(app.repo_table_state.selected(), Some(0));
    }

    /// Review focus 2: the answer for an owner we already left must not be
    /// painted under the new one.
    #[test]
    fn a_late_answer_for_another_owner_is_dropped() {
        let mut app = repos_app();
        app.tx
            .send(repos_answer("old-owner", &["old-owner/x"]))
            .unwrap();
        app.on_tick();

        assert!(app.repo_tab.repos.is_empty());
        assert!(
            !app.repos_loading,
            "the flag clears so the queued reload runs"
        );
    }

    #[test]
    fn a_failed_list_says_so() {
        let mut app = repos_app();
        app.tx
            .send(Loaded::Repos(ReposResult {
                owner: "acme".to_string(),
                repos: Err("HTTP 404".to_string()),
                locals: Vec::new(),
            }))
            .unwrap();
        app.on_tick();
        assert!(app.status.contains("HTTP 404"), "got {:?}", app.status);
    }

    #[test]
    fn the_account_becomes_the_owner_once_known() {
        let mut app = App::new(PathBuf::from("."));
        app.set_tab(Tab::Repos); // no owner yet: nothing to load
        assert!(app.status.contains("Waiting"), "got {:?}", app.status);

        app.repos_loading = true; // queue instead of running `gh`
        app.tx
            .send(Loaded::User(Some("vincent".to_string())))
            .unwrap();
        app.on_tick();

        assert_eq!(app.repo_tab.filters.owner.as_deref(), Some("vincent"));
        assert!(app.repos_pending, "the tab is on screen: it reloads");
    }

    #[test]
    fn without_an_account_the_tab_says_why_it_stays_empty() {
        let mut app = App::new(PathBuf::from("."));
        app.active_tab = Tab::Repos;
        app.tx.send(Loaded::User(None)).unwrap();
        app.on_tick();
        assert!(
            app.status.contains("No GitHub account"),
            "got {:?}",
            app.status
        );
    }

    #[test]
    fn a_saved_owner_waits_for_the_orgs_before_being_judged() {
        let mut app = repos_app();
        app.tx
            .send(Loaded::User(Some("vincent".to_string())))
            .unwrap();
        app.on_tick();
        assert_eq!(app.repo_tab.filters.owner.as_deref(), Some("acme"));

        // The orgs arrive and `acme` is not among them: back to the account.
        app.tx.send(Loaded::Orgs(vec!["corp".to_string()])).unwrap();
        app.on_tick();
        assert_eq!(app.repo_tab.filters.owner.as_deref(), Some("vincent"));
        assert!(app.repos_pending);
    }

    fn repos_app_on(field: FilterField) -> App {
        let mut app = repos_app();
        app.login = Login::Known("vincent".to_string());
        app.repo_tab.orgs = Some(vec!["acme".to_string()]);
        app.filter_cursor = fields_for(Tab::Repos)
            .iter()
            .position(|f| *f == field)
            .expect("a Repos panel row");
        app
    }

    #[test]
    fn arrows_on_owner_switch_it_and_reload_the_list() {
        let mut app = repos_app_on(FilterField::Owner);
        app.filter_change(true);
        assert_eq!(app.repo_tab.filters.owner.as_deref(), Some("vincent"));
        assert!(app.repos_pending);
    }

    #[test]
    fn the_archived_box_reloads_but_hide_cloned_does_not() {
        let mut app = repos_app_on(FilterField::Archived);
        app.filter_change(true);
        assert!(app.repo_tab.filters.archived);
        assert!(app.repos_pending);

        let mut app = repos_app_on(FilterField::HideCloned);
        app.filter_change(true);
        assert!(app.repo_tab.filters.hide_cloned);
        assert!(!app.repos_pending, "hide cloned narrows in memory");
    }

    #[test]
    fn the_auto_refresh_never_reloads_the_repo_list() {
        let mut app = App::new(PathBuf::from("."));
        app.active_tab = Tab::Repos;
        assert_eq!(app.active_job(), Job::Prs);
        app.runs_loaded = true;
        assert_eq!(app.active_job(), Job::Both);
    }

    #[test]
    fn the_prs_tab_loads_on_its_first_visit() {
        let mut app = App::new(PathBuf::from("."));
        app.active_tab = Tab::Repos;
        app.loading = true; // queue instead of running `gh`
        app.set_tab(Tab::Prs);
        assert_eq!(app.pending_job, Some(Job::Prs));
    }

    #[test]
    fn an_empty_folder_opens_on_the_repos_tab() {
        let root = std::env::temp_dir().join("gh-ui-initial-tab");
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(initial_tab(&root), Tab::Repos);

        std::fs::create_dir_all(root.join("web").join(".git")).unwrap();
        assert_eq!(initial_tab(&root), Tab::Prs);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn enter_on_a_repo_opens_its_page() {
        let mut app = repos_app();
        app.repo_tab
            .apply_load(vec![sample_repo("acme/api")], Vec::<LocalRepo>::new());
        app.reset_selection();
        assert_eq!(
            app.selected_url().as_deref(),
            Some("https://github.com/acme/api")
        );
    }

    /// A Repos tab holding `acme/api` (clonable) and `acme/web` (cloned),
    /// cursor on the first row.
    fn tick_app() -> App {
        let mut app = repos_app();
        app.repo_tab.apply_load(
            vec![sample_repo("acme/api"), sample_repo("acme/web")],
            vec![LocalRepo {
                folder: "web".to_string(),
                origin: Some("acme/web".to_string()),
            }],
        );
        app.reset_selection();
        app
    }

    /// A Repos tab of 30 rows, 10 of them on screen, cursor on the first.
    fn long_list_app() -> App {
        let mut app = repos_app();
        let names: Vec<String> = (0..30).map(|i| format!("acme/r{i}")).collect();
        app.repo_tab
            .apply_load(names.iter().map(|n| sample_repo(n)).collect(), Vec::new());
        app.page_rows = 10;
        app.reset_selection();
        app
    }

    #[test]
    fn page_down_and_up_move_by_a_screen_and_stop_at_the_ends() {
        let mut app = long_list_app();
        app.page_down();
        assert_eq!(app.repo_table_state.selected(), Some(10));
        app.page_down();
        app.page_down();
        assert_eq!(
            app.repo_table_state.selected(),
            Some(29),
            "not past the end"
        );

        app.page_up();
        assert_eq!(app.repo_table_state.selected(), Some(19));
        app.page_up();
        app.page_up();
        assert_eq!(app.repo_table_state.selected(), Some(0), "not past the top");
    }

    #[test]
    fn home_and_end_jump_to_the_first_and_last_row() {
        let mut app = long_list_app();
        app.last();
        assert_eq!(app.repo_table_state.selected(), Some(29));
        app.first();
        assert_eq!(app.repo_table_state.selected(), Some(0));
    }

    #[test]
    fn end_reaches_the_clone_button() {
        let mut app = tick_app();
        app.toggle_tick();
        app.last();
        assert!(app.on_clone_button());
    }

    #[test]
    fn space_ticks_the_repo_under_the_cursor() {
        let mut app = tick_app();
        app.toggle_tick();
        assert!(app.repo_tab.ticked.contains("acme/api"));
        app.toggle_tick();
        assert!(app.repo_tab.ticked.is_empty());
    }

    #[test]
    fn space_on_a_cloned_repo_says_why_nothing_happens() {
        let mut app = tick_app();
        app.next();
        app.toggle_tick();
        assert!(app.repo_tab.ticked.is_empty());
        assert_eq!(app.status, "acme/web is already cloned");
    }

    #[test]
    fn space_does_nothing_outside_the_repos_tab() {
        let mut app = tick_app();
        app.active_tab = Tab::Prs;
        app.toggle_tick();
        assert!(app.repo_tab.ticked.is_empty());
    }

    #[test]
    fn the_clone_button_is_the_row_below_the_list() {
        let mut app = tick_app();
        app.toggle_tick();
        assert!(!app.on_clone_button());

        app.next();
        app.next(); // past `acme/web`: the button
        assert!(app.on_clone_button());
        assert_eq!(app.selected_url(), None, "enter here clones, not opens");

        app.ask_clone();
        assert!(matches!(app.confirm, Some(Confirm::Clone { count: 1, .. })));
        assert!(app.is_prompt_open(), "the prompt holds the auto-refresh");

        app.confirm_no();
        assert_eq!(app.confirm, None);
        assert!(
            app.repo_tab.ticked.contains("acme/api"),
            "no keeps the ticks"
        );
    }

    #[test]
    fn enter_on_a_row_never_asks_to_clone() {
        let mut app = tick_app();
        app.toggle_tick();
        app.ask_clone(); // cursor on `acme/api`, not on the button
        assert_eq!(app.confirm, None);
    }

    /// Review focus 5: the button vanishes under the cursor.
    #[test]
    fn the_cursor_leaves_the_button_when_it_vanishes() {
        let mut app = tick_app();
        app.toggle_tick();
        app.next();
        app.next();
        assert!(app.on_clone_button());

        app.repo_tab.start_batch();
        app.clamp_repo_selection();

        assert_eq!(app.repo_table_state.selected(), Some(1));
        assert!(!app.on_clone_button());
    }

    #[test]
    fn quitting_during_a_batch_asks_first() {
        let mut app = tick_app();
        app.repo_tab.cloning = true;

        app.request_quit();
        assert!(!app.should_quit);
        assert_eq!(app.confirm, Some(Confirm::Quit));

        app.confirm_yes();
        assert!(app.should_quit);
    }

    #[test]
    fn quitting_without_a_batch_is_immediate() {
        let mut app = tick_app();
        app.request_quit();
        assert!(app.should_quit);
    }

    #[test]
    fn a_clone_step_leaves_the_pr_loading_flag_alone() {
        let mut app = tick_app();
        app.loading = true;
        app.tx
            .send(Loaded::Clone(CloneEvent::Started("acme/api".to_string())))
            .unwrap();
        app.on_tick();

        assert!(app.loading, "a clone step is not the end of a PR load");
        assert_eq!(app.status, "Cloning acme/api…");
    }

    #[test]
    fn the_end_of_a_batch_reloads_the_prs_and_the_list() {
        let mut app = tick_app();
        app.loading = true; // queue the PR reload instead of running `gh`
        app.repo_tab.cloning = true;
        app.tx.send(Loaded::Clone(CloneEvent::Finished)).unwrap();
        app.on_tick();

        assert!(!app.repo_tab.cloning);
        assert_eq!(app.pending_job, Some(Job::Prs));
        assert!(app.repos_pending);
    }

    #[test]
    fn the_clone_prompt_abbreviates_the_home_folder() {
        let home = Path::new("/Users/vincent");
        assert_eq!(
            tilde(Path::new("/Users/vincent/dev/workspace"), Some(home)),
            "~/dev/workspace"
        );
        assert_eq!(tilde(Path::new("/tmp/ws"), Some(home)), "/tmp/ws");
        assert_eq!(tilde(Path::new("/tmp/ws"), None), "/tmp/ws");
    }

    fn empty_pr_load() -> Loaded {
        Loaded::Prs(FetchResult {
            prs: Vec::new(),
            all_repos: Vec::new(),
            scanned: 0,
            errors: 0,
        })
    }

    /// Final review I1: the auto-refresh reloads the PRs behind the Repos
    /// tab; that must not move the Repos cursor.
    #[test]
    fn a_background_pr_load_keeps_the_repos_cursor() {
        let mut app = tick_app();
        app.next();
        app.loading = true;
        app.tx.send(empty_pr_load()).unwrap();
        app.on_tick();
        assert_eq!(app.repo_table_state.selected(), Some(1));
    }

    /// Final review I2: nor may it overwrite the Repos tab's status line —
    /// which the PRs tab gets back when the user returns to it.
    #[test]
    fn a_background_pr_load_leaves_the_repos_status_alone() {
        let mut app = tick_app();
        app.status = "2 repo(s) — 1 cloned — 1 clone(s) failed".to_string();
        app.loading = true;
        app.tx.send(empty_pr_load()).unwrap();
        app.on_tick();

        assert_eq!(app.status, "2 repo(s) — 1 cloned — 1 clone(s) failed");
        assert!(!app.loading, "the load itself is over");

        app.set_tab(Tab::Prs);
        assert!(app.status.starts_with("0 PR(s)"), "got {:?}", app.status);
    }

    /// An `App` on the Issues tab with an issue load "in flight", so any
    /// reload queues (`issue_tab.pending`) instead of running `gh`. The root
    /// does not exist: a load that does start discovers no repo, runs no `gh`.
    fn issues_app() -> App {
        let mut app = App::new(PathBuf::from("/nonexistent/gh-ui-test"));
        app.active_tab = Tab::Issues;
        app.issue_tab.loading = true;
        app
    }

    fn issues_answer(issues: Vec<Issue>) -> Loaded {
        Loaded::Issues(IssuesResult {
            issues,
            all_repos: vec!["api".to_string(), "web".to_string()],
            scanned: 2,
            errors: 1,
        })
    }

    #[test]
    fn an_issue_load_is_sorted_newest_first_and_says_what_failed() {
        let mut app = issues_app();
        app.tx
            .send(issues_answer(vec![
                sample_issue("api", 1, "2026-09-01T00:00:00Z"),
                sample_issue("web", 2, "2026-09-20T00:00:00Z"),
            ]))
            .unwrap();
        app.on_tick();

        let numbers: Vec<u64> = app.visible_issues().iter().map(|i| i.number).collect();
        assert_eq!(numbers, [2, 1]);
        assert_eq!(app.status, "2 issue(s) — 2 repo(s) — 1 failed");
        assert!(!app.issue_tab.loading);
        assert_eq!(app.issue_table_state.selected(), Some(0));
        assert_eq!(
            app.selected_url().as_deref(),
            Some("https://github.com/acme/web/issues/2")
        );
    }

    /// Review focus 1: the auto-refresh reloads the issues behind the PRs
    /// tab; their line must wait for the Issues tab.
    #[test]
    fn an_issue_load_behind_another_tab_keeps_that_tab_line() {
        let mut app = issues_app();
        app.active_tab = Tab::Prs;
        app.prs_loaded = true;
        app.status = "3 PR(s) — 2 repo(s)".to_string();
        app.tx.send(issues_answer(Vec::new())).unwrap();
        app.on_tick();
        assert_eq!(app.status, "3 PR(s) — 2 repo(s)");

        app.set_tab(Tab::Issues);
        assert!(app.status.starts_with("0 issue(s)"), "got {:?}", app.status);
    }

    /// Review focus 2: leaving PRs for Issues and back shows the PRs line.
    #[test]
    fn the_prs_line_comes_back_after_a_visit_to_the_issues_tab() {
        let mut app = App::new(PathBuf::from("/nonexistent/gh-ui-test"));
        app.prs_loaded = true;
        app.issue_tab.loaded = true;
        app.issue_tab.status = "4 issue(s) — 2 repo(s)".to_string();
        app.status = "3 PR(s) — 2 repo(s)".to_string();

        app.set_tab(Tab::Issues);
        assert_eq!(app.status, "4 issue(s) — 2 repo(s)");
        app.set_tab(Tab::Prs);
        assert_eq!(app.status, "3 PR(s) — 2 repo(s)");
    }

    #[test]
    fn a_pr_load_behind_the_issues_tab_waits_for_the_prs_tab() {
        let mut app = issues_app();
        app.status = "4 issue(s)".to_string();
        app.loading = true;
        app.tx.send(empty_pr_load()).unwrap();
        app.on_tick();
        assert_eq!(app.status, "4 issue(s)");

        app.set_tab(Tab::Prs);
        assert!(app.status.starts_with("0 PR(s)"), "got {:?}", app.status);
    }

    #[test]
    fn the_issues_tab_loads_on_its_first_visit_only() {
        let mut app = App::new(PathBuf::from("/nonexistent/gh-ui-test"));
        app.prs_loaded = true;
        app.set_tab(Tab::Issues);
        assert!(app.issue_tab.loading, "first visit: a load starts");
        assert_eq!(app.status, "Loading issues…");

        app.issue_tab.loading = false;
        app.issue_tab.loaded = true;
        app.set_tab(Tab::Prs);
        app.set_tab(Tab::Issues);
        assert!(!app.issue_tab.loading, "second visit: no reload");
    }

    /// Review focus 4: a reload asked for during a load queues, once.
    #[test]
    fn a_reload_asked_during_an_issue_load_queues() {
        let mut app = issues_app();
        app.refresh();
        assert!(app.issue_tab.pending);
        assert!(app.issue_tab.loading, "still the first load");
    }

    #[test]
    fn the_auto_refresh_reloads_the_issues_once_they_are_loaded() {
        let mut app = issues_app();
        app.loading = true; // the PR job queues instead of running `gh`
        app.background_reload();
        assert!(!app.issue_tab.pending, "never loaded: the first visit will");

        app.issue_tab.loaded = true;
        app.background_reload();
        assert!(app.issue_tab.pending);
        assert!(app.pending_job.is_some(), "the PRs reload too");
    }

    #[test]
    fn the_end_of_a_clone_batch_reloads_loaded_issues() {
        let mut app = issues_app();
        app.loading = true;
        app.issue_tab.loaded = true;
        app.tx.send(Loaded::Clone(CloneEvent::Finished)).unwrap();
        app.on_tick();
        assert!(app.issue_tab.pending);
    }

    /// Review focus 3, on a real folder: an issue repo filter saved
    /// elsewhere is dropped before the load, and the Issues line says so.
    #[test]
    fn reconciling_drops_an_issue_repo_the_folder_does_not_hold() {
        let root = std::env::temp_dir().join("gh-ui-reconcile-issues");
        std::fs::create_dir_all(root.join("web").join(".git")).unwrap();

        let mut app = App::new(root.clone());
        app.issue_tab.filters.repo = Some("ghost".to_string());
        app.reconcile_issue_repo();

        assert_eq!(app.issue_tab.filters.repo, None);
        assert!(
            app.issue_tab
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("ghost")),
            "got {:?}",
            app.issue_tab.notice
        );
        assert_eq!(app.notice, None, "the PRs line is not concerned");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_issues_panel_has_its_own_section() {
        assert_eq!(section_of(Tab::Issues, FilterField::Repo), "Issues");
        assert_eq!(section_of(Tab::Issues, FilterField::Assignee), "Issues");
        assert_eq!(section_of(Tab::Prs, FilterField::Repo), "Common");
    }

    #[test]
    fn the_issues_panel_edits_the_issue_filters_and_reloads_the_issues_only() {
        let mut app = issues_app();
        app.filter_cursor = fields_for(Tab::Issues)
            .iter()
            .position(|f| *f == FilterField::Assignee)
            .unwrap();
        app.filter_change(true);

        assert_eq!(app.issue_tab.filters.assignee, AssigneeFilter::Me);
        assert!(app.issue_tab.pending, "the issues reload");
        assert_eq!(app.pending_job, None, "the PRs do not");
    }

    #[test]
    fn the_issues_panel_cycles_the_issue_repo_not_the_pr_one() {
        let mut app = issues_app();
        app.repos = vec!["api".to_string(), "web".to_string()];
        app.filter_cursor = 0; // Repo
        app.filter_change(true);
        assert_eq!(app.issue_tab.filters.repo.as_deref(), Some("api"));
        assert_eq!(app.filters.repo, None);
    }

    #[test]
    fn an_author_typed_on_the_issues_tab_goes_to_the_issue_filters() {
        let mut app = issues_app();
        app.filter_cursor = fields_for(Tab::Issues)
            .iter()
            .position(|f| *f == FilterField::Author)
            .unwrap();
        app.filter_activate();
        assert!(app.input_kind == Some(InputKind::Author));
        for c in "octocat".chars() {
            app.input_push(c);
        }
        app.input_commit();

        assert_eq!(
            app.issue_tab.filters.author,
            AuthorFilter::Is("octocat".to_string())
        );
        assert_eq!(app.filters.author, AuthorFilter::Any);
        assert!(app.issue_tab.pending);
    }

    #[test]
    fn the_label_prompt_opens_prefilled_with_the_issue_labels() {
        let mut app = issues_app();
        app.issue_tab.filters.labels = vec!["bug".to_string(), "ui".to_string()];
        app.filter_cursor = fields_for(Tab::Issues)
            .iter()
            .position(|f| *f == FilterField::Label)
            .unwrap();
        app.filter_activate();
        assert_eq!(app.input_buffer, "bug ui");
    }

    #[test]
    fn the_filter_cursor_stays_inside_the_shorter_issues_panel() {
        let mut app = issues_app();
        app.active_tab = Tab::Prs;
        app.issue_tab.loaded = true;
        app.filter_cursor = fields_for(Tab::Prs).len() - 1;
        app.set_tab(Tab::Issues);
        assert!(app.filter_cursor < fields_for(Tab::Issues).len());
    }
}
