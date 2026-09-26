//! The table columns: what each one displays, in which order, and which ones
//! are shown. One enum per tab; the order/visibility logic is shared.

use crate::config;
use crate::issues::{Issue, linked_prs_label};
use crate::model::{Label, Pr, Run};
use crate::repos::{Repo, RepoState};
use ratatui::layout::Constraint;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Cell;
use serde::{Deserialize, Serialize};

/// What `ColumnLayout` needs to know about a column, whichever tab it belongs
/// to: the full list, a header label, a width.
/// `'static` is required by `all()`: the type `&'static [Self]` is only
/// well-formed when `Self: 'static`, and the compiler will not infer that for
/// a type parameter — so the bound has to be written down. Putting it on the
/// trait rather than on each `impl` spares every generic consumer
/// (`ColumnLayout<C>`, `column_lines<C>`) from repeating it.
pub trait Column: Copy + PartialEq + 'static {
    fn all() -> &'static [Self];
    fn header(self) -> &'static str;
    fn width(self) -> Constraint;

    /// Whether a layout nobody has customized shows this column. Most do; a
    /// column rarely worth its width starts hidden, one `c` away.
    fn shown_by_default(self) -> bool {
        true
    }
}

/// The columns of the PRs tab.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum PrColumn {
    Repo,
    Updated,
    Number,
    Title,
    Author,
    Review,
    Diff,
    Labels,
}

const PR_COLUMNS: [PrColumn; 8] = [
    PrColumn::Repo,
    PrColumn::Updated,
    PrColumn::Number,
    PrColumn::Title,
    PrColumn::Author,
    PrColumn::Review,
    PrColumn::Diff,
    PrColumn::Labels,
];

impl Column for PrColumn {
    fn all() -> &'static [Self] {
        &PR_COLUMNS
    }

    fn header(self) -> &'static str {
        match self {
            PrColumn::Repo => "Repo",
            PrColumn::Updated => "Updated",
            PrColumn::Number => "#",
            PrColumn::Title => "Title",
            PrColumn::Author => "Author",
            PrColumn::Review => "Review",
            PrColumn::Diff => "+/-",
            PrColumn::Labels => "Labels",
        }
    }

    /// The widths of the former literal array in `render_pr_table`, unchanged.
    fn width(self) -> Constraint {
        match self {
            PrColumn::Repo => Constraint::Length(16),
            PrColumn::Updated => Constraint::Length(10),
            PrColumn::Number => Constraint::Length(7),
            PrColumn::Title => Constraint::Fill(2), // elastic, 2 shares of the rest
            PrColumn::Author => Constraint::Length(20),
            PrColumn::Review => Constraint::Length(9),
            PrColumn::Diff => Constraint::Length(11),
            PrColumn::Labels => Constraint::Fill(1), // elastic, 1 share of the rest
        }
    }
}

/// Label + color of a run based on (status, conclusion). Used by
/// `RunColumn::cell` below; moved here (from `ui.rs`) because nothing in
/// `ui.rs` calls it anymore — `columns.rs` is its only user.
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

/// Same idea for a PR's review decision. Used by `PrColumn::cell` above.
fn review_look(decision: &str) -> (&'static str, Style) {
    match decision {
        "APPROVED" => ("approved", Style::new().fg(Color::Green)),
        "CHANGES_REQUESTED" => ("changes", Style::new().fg(Color::Red)),
        "REVIEW_REQUIRED" => ("review", Style::new().fg(Color::Yellow)),
        _ => ("-", Style::new().fg(Color::DarkGray)),
    }
}

/// Labels as `#bug #api`, grayed and truncated to the column width: the PRs
/// and issues tables show them alike.
fn labels_cell(labels: &[Label]) -> Cell<'static> {
    let text = labels
        .iter()
        .map(|l| format!("#{}", l.name))
        .collect::<Vec<_>>()
        .join(" ");
    Cell::from(Span::styled(text, Style::new().fg(Color::DarkGray)))
}

impl PrColumn {
    /// The cell of this column for `pr`. `Cell<'static>`: every cell owns its
    /// `String`, so the row keeps borrowing neither the `Pr` nor the `App`.
    pub fn cell(self, pr: &Pr) -> Cell<'static> {
        match self {
            PrColumn::Repo => Cell::from(pr.repo.clone()),
            PrColumn::Updated => Cell::from(pr.updated_at.get(..10).unwrap_or("").to_string()),
            PrColumn::Number => Cell::from(format!("#{}", pr.number)),
            // Prefixed with "[D]" and grayed out if it is a draft.
            PrColumn::Title => {
                if pr.is_draft {
                    Cell::from(format!("[D] {}", pr.title)).style(Style::new().fg(Color::DarkGray))
                } else {
                    Cell::from(pr.title.clone())
                }
            }
            PrColumn::Author => Cell::from(format!("@{}", pr.author.login)),
            PrColumn::Review => {
                let (label, style) = review_look(&pr.review_decision);
                Cell::from(Span::styled(label, style))
            }
            PrColumn::Diff => Cell::from(Line::from(vec![
                Span::styled(format!("+{}", pr.additions), Style::new().fg(Color::Green)),
                Span::raw("/"),
                Span::styled(format!("-{}", pr.deletions), Style::new().fg(Color::Red)),
            ])),
            PrColumn::Labels => labels_cell(&pr.labels),
        }
    }
}

/// The columns of the Actions tab.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum RunColumn {
    Repo,
    Created,
    Number,
    Workflow,
    Branch,
    Event,
    Status,
    Title,
}

const RUN_COLUMNS: [RunColumn; 8] = [
    RunColumn::Repo,
    RunColumn::Created,
    RunColumn::Number,
    RunColumn::Workflow,
    RunColumn::Branch,
    RunColumn::Event,
    RunColumn::Status,
    RunColumn::Title,
];

impl Column for RunColumn {
    fn all() -> &'static [Self] {
        &RUN_COLUMNS
    }

    fn header(self) -> &'static str {
        match self {
            RunColumn::Repo => "Repo",
            RunColumn::Created => "Created",
            RunColumn::Number => "#",
            RunColumn::Workflow => "Workflow",
            RunColumn::Branch => "Branch",
            RunColumn::Event => "Event",
            RunColumn::Status => "Status",
            RunColumn::Title => "Title",
        }
    }

    fn width(self) -> Constraint {
        match self {
            RunColumn::Repo => Constraint::Length(16),
            RunColumn::Created => Constraint::Length(10),
            RunColumn::Number => Constraint::Length(7),
            RunColumn::Workflow => Constraint::Length(18),
            RunColumn::Branch => Constraint::Length(22),
            RunColumn::Event => Constraint::Length(12),
            RunColumn::Status => Constraint::Length(11),
            RunColumn::Title => Constraint::Fill(1),
        }
    }
}

impl RunColumn {
    pub fn cell(self, run: &Run) -> Cell<'static> {
        match self {
            RunColumn::Repo => Cell::from(run.repo.clone()),
            RunColumn::Created => Cell::from(run.created_at.get(..10).unwrap_or("").to_string()),
            RunColumn::Number => Cell::from(format!("#{}", run.number)),
            RunColumn::Workflow => Cell::from(run.workflow_name.clone()),
            RunColumn::Branch => Cell::from(run.head_branch.clone()),
            RunColumn::Event => Cell::from(run.event.clone()),
            RunColumn::Status => {
                let (label, style) = run_look(&run.status, &run.conclusion);
                Cell::from(Span::styled(label, style))
            }
            RunColumn::Title => Cell::from(run.display_title.clone()),
        }
    }
}

/// The columns of the Repos tab.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum RepoColumn {
    Tick,
    Repo,
    State,
    Visibility,
    Pushed,
    Description,
}

const REPO_COLUMNS: [RepoColumn; 6] = [
    RepoColumn::Tick,
    RepoColumn::Repo,
    RepoColumn::State,
    RepoColumn::Visibility,
    RepoColumn::Pushed,
    RepoColumn::Description,
];

impl Column for RepoColumn {
    fn all() -> &'static [Self] {
        &REPO_COLUMNS
    }

    fn header(self) -> &'static str {
        match self {
            RepoColumn::Tick => "Tick",
            RepoColumn::Repo => "Repo",
            RepoColumn::State => "State",
            RepoColumn::Visibility => "Visibility",
            RepoColumn::Pushed => "Pushed",
            RepoColumn::Description => "Description",
        }
    }

    fn width(self) -> Constraint {
        match self {
            RepoColumn::Tick => Constraint::Length(4),
            RepoColumn::Repo => Constraint::Length(40),
            RepoColumn::State => Constraint::Length(26),
            RepoColumn::Visibility => Constraint::Length(10),
            RepoColumn::Pushed => Constraint::Length(10),
            RepoColumn::Description => Constraint::Fill(1),
        }
    }
}

/// Label + color of a repo's state. A clonable row reads blank: its tick
/// box already says it can be cloned.
fn repo_state_look(state: &RepoState) -> (String, Style) {
    match state {
        RepoState::Clonable => (String::new(), Style::new()),
        RepoState::Cloned => ("✓ cloned".to_string(), Style::new().fg(Color::Green)),
        RepoState::NameTaken(origin) => (
            format!("name taken → {}", origin.as_deref().unwrap_or("?")),
            Style::new().fg(Color::Yellow),
        ),
        RepoState::Queued => ("queued".to_string(), Style::new().fg(Color::Gray)),
        RepoState::Cloning => ("cloning…".to_string(), Style::new().fg(Color::Yellow)),
        RepoState::Failed(_) => ("✗ failed".to_string(), Style::new().fg(Color::Red)),
    }
}

impl RepoColumn {
    /// `state` and `ticked` come from the tab, not from the repo: the same
    /// repo reads differently depending on the folder and the batch.
    pub fn cell(self, repo: &Repo, state: &RepoState, ticked: bool) -> Cell<'static> {
        match self {
            RepoColumn::Tick => match state {
                RepoState::Clonable | RepoState::Failed(_) => {
                    Cell::from(if ticked { "[x]" } else { "[ ]" })
                }
                _ => Cell::from(""),
            },
            RepoColumn::Repo => Cell::from(repo.name_with_owner.clone()),
            RepoColumn::State => {
                let (label, style) = repo_state_look(state);
                Cell::from(Span::styled(label, style))
            }
            RepoColumn::Visibility => Cell::from(repo.visibility.to_lowercase()),
            RepoColumn::Pushed => Cell::from(
                repo.pushed_at
                    .as_deref()
                    .and_then(|at| at.get(..10))
                    .unwrap_or("")
                    .to_string(),
            ),
            RepoColumn::Description => match state {
                // Why the clone failed. The status line says it too, but the
                // next step of the batch overwrites it within the same tick:
                // the row is where the reason stays.
                RepoState::Failed(message) => Cell::from(Span::styled(
                    format!("✗ {message}"),
                    Style::new().fg(Color::Red),
                )),
                _ => Cell::from(Span::styled(
                    repo.description.clone().unwrap_or_default(),
                    Style::new().fg(Color::DarkGray),
                )),
            },
        }
    }
}

/// The columns of the Issues tab.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum IssueColumn {
    Repo,
    Number,
    Updated,
    Title,
    Author,
    Assignees,
    Labels,
    Prs,
    Created,
}

const ISSUE_COLUMNS: [IssueColumn; 9] = [
    IssueColumn::Repo,
    IssueColumn::Number,
    IssueColumn::Updated,
    IssueColumn::Title,
    IssueColumn::Author,
    IssueColumn::Assignees,
    IssueColumn::Labels,
    IssueColumn::Prs,
    IssueColumn::Created,
];

impl Column for IssueColumn {
    fn all() -> &'static [Self] {
        &ISSUE_COLUMNS
    }

    fn header(self) -> &'static str {
        match self {
            IssueColumn::Repo => "Repo",
            IssueColumn::Number => "#",
            IssueColumn::Updated => "Updated",
            IssueColumn::Title => "Title",
            IssueColumn::Author => "Author",
            IssueColumn::Assignees => "Assignees",
            IssueColumn::Labels => "Labels",
            IssueColumn::Prs => "PRs",
            IssueColumn::Created => "Created",
        }
    }

    fn width(self) -> Constraint {
        match self {
            IssueColumn::Repo => Constraint::Length(16),
            IssueColumn::Number => Constraint::Length(7),
            IssueColumn::Updated => Constraint::Length(10),
            // The title is what an issue is read by: 3 shares of the rest.
            IssueColumn::Title => Constraint::Fill(3),
            IssueColumn::Author => Constraint::Length(20),
            // Narrower than Author: a long second login may be cut.
            IssueColumn::Assignees => Constraint::Length(16),
            IssueColumn::Labels => Constraint::Fill(1), // elastic, 1 share of the rest
            // `#12345` and `3 PRs` fit; a rare `owner/name#12` is cut.
            IssueColumn::Prs => Constraint::Length(10),
            IssueColumn::Created => Constraint::Length(10),
        }
    }

    /// `Updated` already orders the table; the creation date is there for
    /// whoever wants it.
    fn shown_by_default(self) -> bool {
        self != IssueColumn::Created
    }
}

impl IssueColumn {
    /// The cell of this column for `issue`; owned, as the other tables'.
    pub fn cell(self, issue: &Issue) -> Cell<'static> {
        match self {
            IssueColumn::Repo => Cell::from(issue.repo.clone()),
            IssueColumn::Number => Cell::from(format!("#{}", issue.number)),
            IssueColumn::Updated => {
                Cell::from(issue.updated_at.get(..10).unwrap_or("").to_string())
            }
            IssueColumn::Title => Cell::from(issue.title.clone()),
            IssueColumn::Author => Cell::from(format!("@{}", issue.author.login)),
            IssueColumn::Assignees => Cell::from(
                issue
                    .assignees
                    .iter()
                    .map(|a| format!("@{}", a.login))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            IssueColumn::Labels => labels_cell(&issue.labels),
            IssueColumn::Prs => Cell::from(linked_prs_label(issue)),
            IssueColumn::Created => {
                Cell::from(issue.created_at.get(..10).unwrap_or("").to_string())
            }
        }
    }
}

/// One column and whether it is shown. The ORDER of the entries in
/// `ColumnLayout` is the order of the table: no second collection to keep in
/// sync with this one.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
pub struct ColumnEntry<C> {
    pub column: C,
    pub visible: bool,
}

/// The layout of one table: every column, in display order.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ColumnLayout<C> {
    // A plain `#[serde(default)]` would make serde's derive add a `C: Default`
    // bound on every impl (it can't see that `Vec<T>: Default` needs no bound
    // on `T`), which would ripple out to `PrColumn`/`RunColumn` for no reason.
    // Naming the function sidesteps that: serde just calls it, generically.
    #[serde(default = "Vec::new")]
    pub entries: Vec<ColumnEntry<C>>,
}

// Written by hand rather than derived: `derive(Default)` would need
// `Vec::default()` (empty), whereas the default layout is "every column in the
// order of `C::all()`, shown unless `shown_by_default` says otherwise".
impl<C: Column> Default for ColumnLayout<C> {
    fn default() -> Self {
        Self {
            entries: C::all()
                .iter()
                .map(|&column| ColumnEntry {
                    column,
                    visible: column.shown_by_default(),
                })
                .collect(),
        }
    }
}

impl<C: Column> ColumnLayout<C> {
    /// Makes a layout read from disk usable again: drops duplicates, then
    /// appends (at the end, as they would be by default) every column the file did not mention.
    /// This is what makes a config written today survive a future new column.
    pub fn normalize(&mut self) {
        let mut seen: Vec<C> = Vec::new();
        self.entries.retain(|entry| {
            if seen.contains(&entry.column) {
                false
            } else {
                seen.push(entry.column);
                true
            }
        });

        for &column in C::all() {
            if !seen.contains(&column) {
                self.entries.push(ColumnEntry {
                    column,
                    visible: column.shown_by_default(),
                });
            }
        }
    }

    /// The shown columns, in display order.
    pub fn visible(&self) -> impl Iterator<Item = C> + '_ {
        self.entries.iter().filter(|e| e.visible).map(|e| e.column)
    }

    pub fn visible_count(&self) -> usize {
        self.entries.iter().filter(|e| e.visible).count()
    }

    /// Moves the entry at `i` one step towards the LEFT of the table (up in the
    /// panel). Returns its new index: the caller moves its cursor with it, so
    /// several moves can be chained.
    pub fn move_up(&mut self, i: usize) -> usize {
        if i == 0 || i >= self.entries.len() {
            return i;
        }
        self.entries.swap(i, i - 1);
        i - 1
    }

    /// Same, towards the RIGHT of the table.
    pub fn move_down(&mut self, i: usize) -> usize {
        if i + 1 >= self.entries.len() {
            return i;
        }
        self.entries.swap(i, i + 1);
        i + 1
    }

    /// Shows/hides the column at `i`, EXCEPT when it is the last visible one:
    /// a table with no column at all is not a state we let the user reach.
    pub fn toggle(&mut self, i: usize) {
        // Destructured copy: reading `visible` here rather than holding a
        // borrow of `self.entries` lets us call `visible_count()` next.
        let Some(&ColumnEntry { visible, .. }) = self.entries.get(i) else {
            return;
        };
        if visible && self.visible_count() == 1 {
            return;
        }
        self.entries[i].visible = !visible;
    }
}

/// The layout of every table, as persisted. Its own file, next to
/// `filters.json` but separate from it: two independent settings, two files.
#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Columns {
    pub prs: ColumnLayout<PrColumn>,
    pub runs: ColumnLayout<RunColumn>,
    pub repos: ColumnLayout<RepoColumn>,
    pub issues: ColumnLayout<IssueColumn>,
}

impl Columns {
    /// Parses the config's contents. Split out of `load` so the behaviour that
    /// matters — parse, then normalize layouts — is testable without
    /// touching the filesystem. An unreadable or malformed file yields the
    /// defaults, like `Filters::load` does.
    fn from_json(content: &str) -> Self {
        let mut columns: Self = serde_json::from_str(content).unwrap_or_default();
        // Every normalize call is unconditional: they run even when the file
        // exactly matches our current defaults, so that old configs written
        // before new columns were added still yield complete layouts. If the
        // file already contains all columns in the right order, normalize is
        // fast; if it's partial, normalize fills the gaps.
        columns.prs.normalize();
        columns.runs.normalize();
        columns.repos.normalize();
        columns.issues.normalize();
        columns
    }

    /// Reloads the layout, falling back to the defaults if the file is
    /// missing or unreadable — the same lenient behaviour as `Filters::load`.
    /// Always normalized, so a partial or outdated file still yields a
    /// complete layout.
    pub fn load() -> Self {
        let Some(path) = config::path("columns.json") else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(content) => Self::from_json(&content),
            Err(_) => Self::default(),
        }
    }

    /// Saves the layout as JSON (silent on failure).
    pub fn save(&self) {
        let Some(path) = config::path("columns.json") else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_header_starts_with_a_capital() {
        let headers = PR_COLUMNS
            .iter()
            .map(|c| c.header())
            .chain(RUN_COLUMNS.iter().map(|c| c.header()))
            .chain(REPO_COLUMNS.iter().map(|c| c.header()))
            .chain(ISSUE_COLUMNS.iter().map(|c| c.header()));
        for header in headers {
            let first = header.chars().next().expect("a non-empty header");
            // `#` and `+/-` are symbols: nothing to capitalize.
            assert!(
                !first.is_lowercase(),
                "{header:?} should start with a capital"
            );
        }
    }

    fn sample_pr() -> Pr {
        serde_json::from_str(
            r#"{
                "number": 42, "title": "fixes a bug", "author": {"login": "moi"},
                "isDraft": false, "url": "u", "updatedAt": "2026-01-02T03:04:05Z",
                "additions": 7, "deletions": 3, "labels": [{"name": "bug"}],
                "headRefName": "feature/x"
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn each_pr_cell_renders_its_own_column() {
        let pr = sample_pr();

        assert_eq!(PrColumn::Number.cell(&pr), Cell::from("#42".to_string()));
        assert_eq!(PrColumn::Author.cell(&pr), Cell::from("@moi".to_string()));
        // Only the date part of the timestamp.
        assert_eq!(
            PrColumn::Updated.cell(&pr),
            Cell::from("2026-01-02".to_string())
        );
    }

    #[test]
    fn a_draft_title_is_prefixed() {
        let mut pr = sample_pr();
        pr.is_draft = true;

        let cell = PrColumn::Title.cell(&pr);

        assert_eq!(
            cell,
            Cell::from("[D] fixes a bug".to_string()).style(Style::new().fg(Color::DarkGray))
        );
    }

    #[test]
    fn a_run_cell_renders_its_own_column() {
        let run: Run = serde_json::from_str(
            r#"{
                "workflowName": "CI", "displayTitle": "fixes a bug",
                "headBranch": "feature/x", "status": "in_progress",
                "event": "push", "createdAt": "2026-01-02T03:04:05Z",
                "number": 7, "url": "u"
            }"#,
        )
        .unwrap();

        assert_eq!(RunColumn::Workflow.cell(&run), Cell::from("CI".to_string()));
        assert_eq!(
            RunColumn::Branch.cell(&run),
            Cell::from("feature/x".to_string())
        );
    }

    #[test]
    fn default_layout_shows_every_column_in_order() {
        let layout = ColumnLayout::<PrColumn>::default();
        assert_eq!(
            layout.visible().collect::<Vec<_>>(),
            PrColumn::all().to_vec()
        );
        assert_eq!(layout.visible_count(), PrColumn::all().len());
    }

    #[test]
    fn normalize_appends_a_missing_column_at_the_end_visible() {
        // A config file written before the other columns existed.
        let mut layout = ColumnLayout {
            entries: vec![ColumnEntry {
                column: PrColumn::Title,
                visible: true,
            }],
        };
        layout.normalize();

        assert_eq!(layout.entries[0].column, PrColumn::Title);
        assert_eq!(layout.entries.len(), PrColumn::all().len());
        assert!(layout.entries[1..].iter().all(|e| e.visible));
    }

    #[test]
    fn normalize_drops_a_duplicated_column_keeping_the_first() {
        let mut layout = ColumnLayout {
            entries: vec![
                ColumnEntry {
                    column: PrColumn::Title,
                    visible: false,
                },
                ColumnEntry {
                    column: PrColumn::Title,
                    visible: true,
                },
            ],
        };
        layout.normalize();

        let titles = layout
            .entries
            .iter()
            .filter(|e| e.column == PrColumn::Title)
            .count();
        assert_eq!(titles, 1);
        assert!(!layout.entries[0].visible, "the first occurrence wins");
    }

    #[test]
    fn run_columns_have_their_own_list() {
        let layout = ColumnLayout::<RunColumn>::default();
        assert_eq!(layout.visible().next(), Some(RunColumn::Repo));
        assert_eq!(layout.visible_count(), 8);
    }

    #[test]
    fn move_down_swaps_with_the_next_column_and_returns_its_new_index() {
        let mut layout = ColumnLayout::<PrColumn>::default();

        assert_eq!(layout.move_down(0), 1);
        let order: Vec<_> = layout.visible().collect();
        assert_eq!(order[0], PrColumn::Updated);
        assert_eq!(order[1], PrColumn::Repo);
    }

    #[test]
    fn move_up_swaps_with_the_previous_column() {
        let mut layout = ColumnLayout::<PrColumn>::default();

        assert_eq!(layout.move_up(3), 2);
        let order: Vec<_> = layout.visible().collect();
        assert_eq!(order[2], PrColumn::Title);
        assert_eq!(order[3], PrColumn::Number);
    }

    #[test]
    fn moving_past_either_end_is_a_no_op() {
        let mut layout = ColumnLayout::<PrColumn>::default();
        let before = layout.clone();
        let last = layout.entries.len() - 1;

        assert_eq!(layout.move_up(0), 0);
        assert_eq!(layout.move_down(last), last);
        assert_eq!(layout, before);
    }

    #[test]
    fn toggle_hides_then_shows_a_column_again() {
        let mut layout = ColumnLayout::<PrColumn>::default();

        layout.toggle(7); // labels
        assert_eq!(layout.visible_count(), 7);
        assert!(!layout.visible().any(|c| c == PrColumn::Labels));

        layout.toggle(7);
        assert_eq!(layout.visible_count(), 8);
    }

    #[test]
    fn the_last_visible_column_cannot_be_hidden() {
        let mut layout = ColumnLayout::<PrColumn>::default();
        for i in 1..layout.entries.len() {
            layout.toggle(i);
        }
        assert_eq!(layout.visible_count(), 1);

        layout.toggle(0);

        assert_eq!(layout.visible_count(), 1, "hiding the last one is refused");
    }

    #[test]
    fn an_out_of_range_index_is_ignored() {
        let mut layout = ColumnLayout::<PrColumn>::default();
        let before = layout.clone();

        layout.toggle(99);
        assert_eq!(layout.move_down(99), 99);

        assert_eq!(layout, before);
    }

    #[test]
    fn a_column_is_stored_under_its_snake_case_name() {
        // Guards the on-disk format: renaming a variant would break configs.
        let entry = ColumnEntry {
            column: PrColumn::Updated,
            visible: false,
        };
        let json = serde_json::to_string(&entry).unwrap();

        assert_eq!(json, r#"{"column":"updated","visible":false}"#);
    }

    #[test]
    fn a_layout_survives_a_json_round_trip() {
        let mut columns = Columns::default();
        columns.prs.move_down(0);
        columns.prs.toggle(7);
        columns.runs.toggle(4);

        let json = serde_json::to_string_pretty(&columns).unwrap();
        let back: Columns = serde_json::from_str(&json).unwrap();

        assert_eq!(back, columns);
    }

    #[test]
    fn a_default_columns_holds_both_tabs_complete() {
        let columns = Columns::default();

        assert_eq!(columns.prs.entries.len(), PrColumn::all().len());
        assert_eq!(columns.runs.entries.len(), RunColumn::all().len());
        assert_eq!(columns.repos.entries.len(), RepoColumn::all().len());
    }

    #[test]
    fn loading_a_partial_config_normalizes_both_tabs() {
        // "prs" is missing columns (the rest must be appended), and "runs" has
        // a duplicated entry (must be deduplicated). `#[serde(default)]` alone
        // would let a missing "runs" key sneak through as an already-complete
        // default layout without ever calling `normalize`; a duplicate can only
        // be fixed by `normalize`, so this pins that call for both tabs.
        let columns = Columns::from_json(
            r#"{
                "prs":{"entries":[{"column":"title","visible":true}]},
                "runs":{"entries":[
                    {"column":"repo","visible":true},
                    {"column":"repo","visible":true}
                ]}
            }"#,
        );

        assert_eq!(columns.prs.entries[0].column, PrColumn::Title);
        assert_eq!(columns.prs.entries.len(), PrColumn::all().len());
        // The duplicated "repo" entry is deduplicated, then the rest of the
        // columns are appended: this only happens through `normalize`.
        assert_eq!(columns.runs.entries.len(), RunColumn::all().len());
    }

    #[test]
    fn a_malformed_config_falls_back_to_the_defaults() {
        let columns = Columns::from_json("{ not json");

        assert_eq!(columns, Columns::default());
    }

    #[test]
    fn an_entries_less_layout_self_heals_through_normalize() {
        // A hand-edited file dropping "entries" entirely is a likely typo
        // (e.g. clearing a tab's layout to "reset" it): `entries` needs its
        // own `#[serde(default)]` so this doesn't fail the whole file and
        // reset BOTH tabs to their defaults.
        let columns = Columns::from_json(r#"{"prs":{}}"#);

        assert_eq!(columns.prs.entries.len(), PrColumn::all().len());
        assert_eq!(
            columns.prs.visible().collect::<Vec<_>>(),
            PrColumn::all().to_vec()
        );
        // "runs" is entirely absent too: still a complete, normalized layout.
        assert_eq!(columns.runs.entries.len(), RunColumn::all().len());
    }

    #[test]
    fn run_look_colors() {
        assert_eq!(run_look("completed", "success").0, "✓ success");
        assert_eq!(run_look("completed", "failure").0, "✗ failure");
        assert_eq!(run_look("completed", "cancelled").0, "cancelled");
        assert_eq!(run_look("in_progress", "").0, "● running");
        assert_eq!(run_look("queued", "").0, "queued");
    }

    #[test]
    fn a_repo_row_shows_its_name_visibility_and_date() {
        let repo = crate::repos::sample_repo("acme/api");
        let state = RepoState::Clonable;

        assert_eq!(
            RepoColumn::Repo.cell(&repo, &state, false),
            Cell::from("acme/api".to_string())
        );
        assert_eq!(
            RepoColumn::Visibility.cell(&repo, &state, false),
            Cell::from("private".to_string())
        );
        assert_eq!(
            RepoColumn::Pushed.cell(&repo, &state, false),
            Cell::from("2026-09-20".to_string())
        );
    }

    #[test]
    fn only_a_clonable_or_failed_row_has_a_tick_box() {
        let repo = crate::repos::sample_repo("acme/api");
        let tick = |state: RepoState, ticked| RepoColumn::Tick.cell(&repo, &state, ticked);

        assert_eq!(tick(RepoState::Clonable, true), Cell::from("[x]"));
        assert_eq!(tick(RepoState::Clonable, false), Cell::from("[ ]"));
        assert_eq!(
            tick(RepoState::Failed("x".into()), false),
            Cell::from("[ ]")
        );
        assert_eq!(tick(RepoState::Cloned, false), Cell::from(""));
        assert_eq!(tick(RepoState::NameTaken(None), false), Cell::from(""));
    }

    #[test]
    fn repo_states_read_as_words() {
        assert_eq!(repo_state_look(&RepoState::Clonable).0, "");
        assert_eq!(repo_state_look(&RepoState::Cloned).0, "✓ cloned");
        assert_eq!(
            repo_state_look(&RepoState::NameTaken(Some("other/api".into()))).0,
            "name taken → other/api"
        );
        assert_eq!(
            repo_state_look(&RepoState::NameTaken(None)).0,
            "name taken → ?"
        );
        assert_eq!(repo_state_look(&RepoState::Queued).0, "queued");
        assert_eq!(repo_state_look(&RepoState::Cloning).0, "cloning…");
        assert_eq!(
            repo_state_look(&RepoState::Failed("x".into())).0,
            "✗ failed"
        );
    }

    #[test]
    fn a_config_written_before_the_repos_tab_still_gets_its_columns() {
        let columns = Columns::from_json(r#"{"prs":{"entries":[]}}"#);
        assert_eq!(columns.repos.entries.len(), RepoColumn::all().len());
    }

    /// Final review C1: the reason a clone failed must reach the screen —
    /// the status line is overwritten within the same tick.
    #[test]
    fn a_failed_row_shows_why_in_its_description() {
        let repo = crate::repos::sample_repo("acme/api");
        let state = RepoState::Failed("fatal: repository not found".to_string());
        assert_eq!(
            RepoColumn::Description.cell(&repo, &state, false),
            Cell::from(Span::styled(
                "✗ fatal: repository not found".to_string(),
                Style::new().fg(Color::Red)
            ))
        );
    }

    #[test]
    fn a_never_pushed_repo_has_a_blank_date() {
        let mut repo = crate::repos::sample_repo("acme/api");
        repo.pushed_at = None;
        assert_eq!(
            RepoColumn::Pushed.cell(&repo, &RepoState::Clonable, false),
            Cell::from(String::new())
        );
    }

    #[test]
    fn the_issue_layout_hides_only_created_by_default() {
        let layout = ColumnLayout::<IssueColumn>::default();
        let hidden: Vec<IssueColumn> = layout
            .entries
            .iter()
            .filter(|e| !e.visible)
            .map(|e| e.column)
            .collect();
        assert_eq!(hidden, [IssueColumn::Created]);
        assert_eq!(layout.entries.len(), IssueColumn::all().len());
    }

    #[test]
    fn a_config_written_before_the_issues_tab_gets_the_default_issue_layout() {
        let columns = Columns::from_json(r#"{"prs":{"entries":[]}}"#);
        assert_eq!(columns.issues, ColumnLayout::<IssueColumn>::default());
    }

    /// At 120 columns the title must stay readable: the fixed-width columns
    /// may not eat the room it needs.
    #[test]
    fn the_issue_title_keeps_room_at_120_columns() {
        use ratatui::layout::{Layout, Rect};

        let visible: Vec<IssueColumn> = ColumnLayout::<IssueColumn>::default().visible().collect();
        let widths: Vec<Constraint> = visible.iter().map(|c| c.width()).collect();
        // 120 minus the table's two borders and its "▌ " highlight symbol;
        // one space between columns, as the table puts.
        let areas = Layout::horizontal(widths)
            .spacing(1)
            .split(Rect::new(0, 0, 116, 1));
        let title = visible
            .iter()
            .position(|c| *c == IssueColumn::Title)
            .unwrap();
        assert!(
            areas[title].width >= 20,
            "the title gets {} columns",
            areas[title].width
        );
    }

    /// A column the saved layout does not mention is appended as it would
    /// be by default: `Created` hidden, the others shown.
    #[test]
    fn a_partial_issue_layout_appends_the_missing_columns_with_their_default() {
        let columns =
            Columns::from_json(r#"{"issues":{"entries":[{"column":"title","visible":true}]}}"#);
        let entries = &columns.issues.entries;
        assert_eq!(entries.len(), IssueColumn::all().len());
        assert_eq!(entries[0].column, IssueColumn::Title);
        for entry in entries {
            assert_eq!(
                entry.visible,
                entry.column != IssueColumn::Created,
                "{:?}",
                entry.column
            );
        }
    }

    #[test]
    fn an_issue_row_shows_its_number_people_date_and_linked_pr() {
        let mut issue = crate::issues::sample_issue("api", 7, "2026-09-20T10:00:00Z");
        issue.assignees = vec![
            crate::model::Author {
                login: "bob".to_string(),
            },
            crate::model::Author {
                login: "carol".to_string(),
            },
        ];
        issue.linked_prs = vec![crate::issues::PrRef {
            number: 12,
            repository: crate::issues::RepoRef {
                name: "api".to_string(),
                owner: crate::model::Author {
                    login: "acme".to_string(),
                },
            },
        }];

        assert_eq!(
            IssueColumn::Number.cell(&issue),
            Cell::from("#7".to_string())
        );
        assert_eq!(
            IssueColumn::Author.cell(&issue),
            Cell::from("@alice".to_string())
        );
        assert_eq!(
            IssueColumn::Assignees.cell(&issue),
            Cell::from("@bob, @carol".to_string())
        );
        assert_eq!(
            IssueColumn::Updated.cell(&issue),
            Cell::from("2026-09-20".to_string())
        );
        assert_eq!(IssueColumn::Prs.cell(&issue), Cell::from("#12".to_string()));
    }
}
