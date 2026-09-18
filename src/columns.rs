//! The table columns: what each one displays, in which order, and which ones
//! are shown. One enum per tab; the order/visibility logic is shared.

use crate::model::{Pr, Run};
use ratatui::layout::Constraint;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Cell;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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
            PrColumn::Repo => "repo",
            PrColumn::Updated => "updated",
            PrColumn::Number => "#",
            PrColumn::Title => "title",
            PrColumn::Author => "author",
            PrColumn::Review => "review",
            PrColumn::Diff => "+/-",
            PrColumn::Labels => "labels",
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
            // Joined labels: "#bug #api" (grayed), truncated to the column width.
            PrColumn::Labels => {
                let labels = pr
                    .labels
                    .iter()
                    .map(|l| format!("#{}", l.name))
                    .collect::<Vec<_>>()
                    .join(" ");
                Cell::from(Span::styled(labels, Style::new().fg(Color::DarkGray)))
            }
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
            RunColumn::Repo => "repo",
            RunColumn::Created => "created",
            RunColumn::Number => "#",
            RunColumn::Workflow => "workflow",
            RunColumn::Branch => "branch",
            RunColumn::Event => "event",
            RunColumn::Status => "status",
            RunColumn::Title => "title",
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
// `Vec::default()` (empty), whereas the default layout is "every column,
// visible, in the order of `C::all()`".
impl<C: Column> Default for ColumnLayout<C> {
    fn default() -> Self {
        Self {
            entries: C::all()
                .iter()
                .map(|&column| ColumnEntry {
                    column,
                    visible: true,
                })
                .collect(),
        }
    }
}

impl<C: Column> ColumnLayout<C> {
    /// Makes a layout read from disk usable again: drops duplicates, then
    /// appends (visible, at the end) every column the file did not mention.
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
                    visible: true,
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

/// The layout of BOTH tables, as persisted. Its own file, next to
/// `filters.json` but separate from it: two independent settings, two files.
#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Columns {
    pub prs: ColumnLayout<PrColumn>,
    pub runs: ColumnLayout<RunColumn>,
}

impl Columns {
    /// Path of the config file: ~/.config/gh-ui/columns.json (Linux/macOS).
    fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("gh-ui").join("columns.json"))
    }

    /// Parses the config's contents. Split out of `load` so the behaviour that
    /// matters — parse, then normalize BOTH layouts — is testable without
    /// touching the filesystem. An unreadable or malformed file yields the
    /// defaults, like `Filters::load` does.
    fn from_json(content: &str) -> Self {
        let mut columns: Self = serde_json::from_str(content).unwrap_or_default();
        // Both normalize calls are unconditional: they run even when the file
        // exactly matches our current defaults, so that old configs written
        // before new columns were added still yield complete layouts. If the
        // file already contains all columns in the right order, normalize is
        // fast; if it's partial, normalize fills the gaps.
        columns.prs.normalize();
        columns.runs.normalize();
        columns
    }

    /// Reloads the layout, falling back to the defaults if the file is
    /// missing or unreadable — the same lenient behaviour as `Filters::load`.
    /// Always normalized, so a partial or outdated file still yields a
    /// complete layout.
    pub fn load() -> Self {
        let Some(path) = Self::config_path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(content) => Self::from_json(&content),
            Err(_) => Self::default(),
        }
    }

    /// Saves the layout as JSON (silent on failure).
    pub fn save(&self) {
        let Some(path) = Self::config_path() else {
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
}
