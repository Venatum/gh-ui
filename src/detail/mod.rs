//! The detail view (`enter` on a row): what `gh … view` returns, the
//! view's state, and the lines it shows. This file holds what every kind
//! of row shares — the view, its sections, the markdown pass, the
//! conversation — and the PRs' own detail. Pure: `gh.rs` runs the command,
//! `fetch.rs` carries the answer, `ui.rs` draws what this module builds.

use crate::columns::label_spans;
use crate::model::{Author, Bucket, Check, Label, latest_attempts};
use chrono::{DateTime, Datelike, FixedOffset, Utc};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use serde::Deserialize;

mod issue;
mod release;
mod run;

pub use issue::IssueDetail;
pub use release::ReleaseDetail;
pub use run::RunDetail;

/// One PR as `gh pr view --json …` returns it: only the fields the view
/// shows (see `gh::PR_VIEW_FIELDS`, which asks for exactly these). The
/// state, the author and the sizes are always there; everything else may
/// be missing, and defaults rather than failing the whole answer.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrDetail {
    pub title: String,
    pub url: String,
    /// Markdown, as typed in GitHub's editor (CRLF line ends included).
    /// Empty when the author wrote nothing.
    #[serde(default)]
    pub body: String,
    /// "OPEN" | "CLOSED" | "MERGED".
    pub state: String,
    pub is_draft: bool,
    pub author: Author,
    /// The branch it merges into, and the one it comes from.
    #[serde(default)]
    pub base_ref_name: String,
    #[serde(default)]
    pub head_ref_name: String,
    pub additions: u64,
    pub deletions: u64,
    /// How many files the PR touches.
    pub changed_files: u64,
    /// "APPROVED" | "CHANGES_REQUESTED" | "REVIEW_REQUIRED" | "" (none).
    #[serde(default)]
    pub review_decision: String,
    /// "MERGEABLE" | "CONFLICTING" | "UNKNOWN" (not computed yet).
    #[serde(default)]
    pub mergeable: String,
    #[serde(default)]
    pub labels: Vec<Label>,
    /// The reviews still awaited.
    #[serde(default)]
    pub review_requests: Vec<ReviewRequest>,
    /// The latest review of each reviewer, older ones left out by GitHub:
    /// what the Overview's `review` fact sums up.
    #[serde(default)]
    pub latest_reviews: Vec<Review>,
    /// Every review, oldest first: what the Comments section lists.
    #[serde(default)]
    pub reviews: Vec<Review>,
    /// The head commit, to tell a review of an older one.
    #[serde(default)]
    pub head_ref_oid: String,
    /// Every attempt of every check of the head commit, re-runs included
    /// (`checks` keeps the latest of each). The type of the PR list's
    /// `State` column: one vocabulary for both. An `Option` because `gh`
    /// sends `null` for a commit without any status.
    #[serde(default)]
    pub status_check_rollup: Option<Vec<Check>>,
    /// The changed files, 100 at most: `gh` stops there, `changed_files`
    /// has the real count.
    #[serde(default)]
    pub files: Vec<ChangedFile>,
    /// The conversation, oldest first. Comments on a line of the diff are
    /// not in it: `gh pr view` does not expose them.
    #[serde(default)]
    pub comments: Vec<Comment>,
}

/// One comment of the conversation.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub author: Author,
    #[serde(default)]
    pub body: String,
    pub created_at: String,
    /// Hidden by a maintainer, for `minimized_reason` ("OUTDATED",
    /// "OFF_TOPIC"…): its text is not shown, as on GitHub.
    #[serde(default)]
    pub is_minimized: bool,
    #[serde(default)]
    pub minimized_reason: String,
}

/// One changed file and its size.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
    /// "ADDED" | "MODIFIED" | "DELETED" | "RENAMED" | "COPIED" | "CHANGED".
    #[serde(default)]
    pub change_type: String,
}

impl PrDetail {
    /// The checks as the view lists them: the latest attempt of each,
    /// failures first.
    pub fn checks(&self) -> Vec<&Check> {
        latest_checks(self.status_check_rollup.as_deref().unwrap_or_default())
    }
}

/// A section of the view. Each kind of row walks its own, in its own
/// order (see `DetailKey::sections`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// The facts and the body.
    Overview,
    Checks,
    Files,
    /// The conversation (and, for a PR, its reviews).
    Comments,
    /// A run's jobs and their steps.
    Jobs,
    /// A release's facts and notes.
    Notes,
    /// A release's files.
    Assets,
}

/// How many sections there are in all: the size of `DetailView::scrolls`,
/// indexed by `Section as usize`.
const SECTION_KINDS: usize = 7;

/// An issue's sections.
const ISSUE_SECTIONS: [Section; 2] = [Section::Overview, Section::Comments];

/// A run's sections.
const RUN_SECTIONS: [Section; 2] = [Section::Overview, Section::Jobs];

/// A release's sections.
const RELEASE_SECTIONS: [Section; 2] = [Section::Notes, Section::Assets];

/// A PR's sections, in the order `←`/`→` walk them.
const PR_SECTIONS: [Section; 4] = [
    Section::Overview,
    Section::Checks,
    Section::Files,
    Section::Comments,
];

impl Section {
    fn label(self) -> &'static str {
        match self {
            Section::Overview => "Overview",
            Section::Checks => "Checks",
            Section::Files => "Files",
            Section::Comments => "Comments",
            Section::Jobs => "Jobs",
            Section::Notes => "Notes",
            Section::Assets => "Assets",
        }
    }
}

/// A review asked of someone: a user has a `login`, a team a `name`.
#[derive(Debug, Deserialize)]
pub struct ReviewRequest {
    #[serde(default)]
    pub login: String,
    #[serde(default)]
    pub name: String,
}

/// One review.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Review {
    pub author: Author,
    /// "APPROVED" | "CHANGES_REQUESTED" | "COMMENTED" | "DISMISSED" |
    /// "PENDING" (a draft review, invisible to others).
    pub state: String,
    /// The review's summary, often empty: a bare approval, or a review
    /// made only of comments on the diff (which `gh pr view` does not
    /// bring).
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub submitted_at: String,
    /// The commit it was made on. `None` in `latestReviews`, which does
    /// not carry it here.
    #[serde(default)]
    pub commit: Option<ReviewCommit>,
}

/// The commit a review was made on.
#[derive(Debug, Deserialize)]
pub struct ReviewCommit {
    #[serde(default)]
    pub oid: String,
}

/// Which row a view (or a late answer) is about: the folder it comes from,
/// and what tells it apart there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetailKey {
    /// Two PRs of one folder never share a number.
    Pr { repo: String, number: u64 },
    /// Nor two issues (nor an issue and a PR).
    Issue { repo: String, number: u64 },
    /// A run is named by its id, `gh run view`'s argument; its number
    /// (`#812`) is what the view shows.
    Run { repo: String, id: u64, number: u64 },
    /// A folder's release, by its tag.
    Release { repo: String, tag: String },
}

impl DetailKey {
    /// The folder, where `gh` is run.
    pub fn repo(&self) -> &str {
        match self {
            DetailKey::Pr { repo, .. }
            | DetailKey::Issue { repo, .. }
            | DetailKey::Run { repo, .. }
            | DetailKey::Release { repo, .. } => repo,
        }
    }

    /// `api #412`: what the view's border names before the title.
    pub fn label(&self) -> String {
        match self {
            DetailKey::Pr { repo, number } | DetailKey::Issue { repo, number } => {
                format!("{repo} #{number}")
            }
            DetailKey::Run { repo, number, .. } => format!("{repo} run #{number}"),
            DetailKey::Release { repo, tag } => format!("{repo} {tag}"),
        }
    }

    /// The sections the view walks for this kind of row.
    pub fn sections(&self) -> &'static [Section] {
        match self {
            DetailKey::Pr { .. } => &PR_SECTIONS,
            DetailKey::Issue { .. } => &ISSUE_SECTIONS,
            DetailKey::Run { .. } => &RUN_SECTIONS,
            DetailKey::Release { .. } => &RELEASE_SECTIONS,
        }
    }
}

/// What `gh` answered for a view, one variant per kind of row.
#[derive(Debug)]
pub enum Detail {
    Pr(PrDetail),
    Issue(IssueDetail),
    Run(RunDetail),
    Release(ReleaseDetail),
}

impl Detail {
    /// The title as loaded, which may have changed since the list was.
    pub fn title(&self) -> &str {
        match self {
            Detail::Pr(d) => &d.title,
            Detail::Issue(d) => &d.title,
            Detail::Run(d) => &d.display_title,
            Detail::Release(d) => d.title(),
        }
    }

    /// The GitHub page of `section`.
    fn url(&self, section: Section) -> String {
        match self {
            Detail::Pr(d) => match section {
                Section::Checks => format!("{}/checks", d.url),
                Section::Files => format!("{}/files", d.url),
                // The conversation is the PR's own page.
                _ => d.url.clone(),
            },
            Detail::Issue(d) => d.url.clone(),
            Detail::Run(d) => d.url.clone(),
            Detail::Release(d) => d.url.clone(),
        }
    }

    /// How many rows `section` lists, for the section bar; `None` for a
    /// section that is no list (an overview).
    fn count(&self, section: Section) -> Option<usize> {
        match self {
            Detail::Pr(d) => match section {
                Section::Checks => Some(d.checks().len()),
                Section::Files => usize::try_from(d.changed_files).ok(),
                Section::Comments => Some(timeline(d).len()),
                _ => None,
            },
            Detail::Issue(d) => match section {
                Section::Comments => Some(d.comments.len()),
                _ => None,
            },
            Detail::Run(d) => match section {
                Section::Jobs => Some(d.jobs.len()),
                _ => None,
            },
            Detail::Release(d) => match section {
                Section::Assets => Some(d.assets.len()),
                _ => None,
            },
        }
    }
}

/// What the drawing needs besides the detail: the room, the clock, the
/// time zone and whether folded entries are open. Gathered so a section's
/// lines are one pure call, which tests pin.
#[derive(Debug, Clone, Copy)]
pub struct Context {
    pub width: u16,
    pub now: DateTime<Utc>,
    pub offset: FixedOffset,
    pub show_folded: bool,
}

/// The lines of `section` of `detail`.
pub fn section_lines(detail: &Detail, section: Section, cx: Context) -> Vec<Line<'static>> {
    match detail {
        Detail::Pr(d) => match section {
            Section::Checks => checks_lines(d, cx.now, cx.width),
            Section::Files => files_lines(d, cx.width),
            Section::Comments => comments_lines(d, cx.offset, cx.show_folded),
            _ => overview_lines(d, cx.width),
        },
        Detail::Issue(d) => match section {
            Section::Comments => issue::comments_lines(d, cx.offset),
            _ => issue::overview_lines(d, cx.width, cx.offset),
        },
        Detail::Run(d) => match section {
            Section::Jobs => run::jobs_lines(d, cx.now, cx.width, cx.show_folded),
            _ => run::overview_lines(d, cx.now, cx.offset),
        },
        Detail::Release(d) => match section {
            Section::Assets => release::assets_lines(d, cx.width),
            _ => release::notes_lines(d, cx.width, cx.offset),
        },
    }
}

/// The state of the open view: which PR, what `gh` answered so far, and
/// whether an answer is on its way.
#[derive(Debug)]
pub struct DetailView {
    pub key: DetailKey,
    /// The title the list row had: shown while the detail loads.
    pub title: String,
    /// The last answer that succeeded. Boxed because it travels inside
    /// `fetch::Loaded`, whose other variants are much smaller.
    pub detail: Option<Box<Detail>>,
    /// The last load's failure, `gh`'s own message.
    pub error: Option<String>,
    /// A load is in flight: the header spinner turns.
    pub loading: bool,
    /// The section on screen.
    pub section: Section,
    /// How many rows each section is scrolled by, so going back to one
    /// finds it where it was left. Indexed by `Section as usize`.
    scrolls: [u16; SECTION_KINDS],
    /// The furthest the section on screen may scroll: its last row at the
    /// bottom of the view. Written by `ui::render_detail`, which alone
    /// knows the size of the screen, as it writes `App::page_rows`.
    pub max_scroll: u16,
    /// How many rows the text shows at once: what PgUp/PgDn move by.
    pub page: u16,
    /// `space`: the Comments section shows the folded reviews in full.
    pub show_folded: bool,
}

impl DetailView {
    /// A view that waits for its first answer.
    pub fn new(key: DetailKey, title: String) -> Self {
        Self {
            section: key.sections()[0],
            key,
            title,
            detail: None,
            error: None,
            loading: true,
            scrolls: [0; SECTION_KINDS],
            max_scroll: 0,
            page: 1,
            show_folded: false,
        }
    }

    /// `space`: unfolds the folded reviews, or folds them back.
    pub fn toggle_folded(&mut self) {
        self.show_folded = !self.show_folded;
    }

    /// How far the section on screen is scrolled.
    pub fn scroll(&self) -> u16 {
        self.scrolls[self.section as usize]
    }

    /// The same, to change it.
    pub fn scroll_mut(&mut self) -> &mut u16 {
        &mut self.scrolls[self.section as usize]
    }

    /// `→`: the next section, back to the first after the last.
    pub fn next_section(&mut self) {
        let sections = self.key.sections();
        let at = sections
            .iter()
            .position(|&s| s == self.section)
            .unwrap_or(0);
        self.section = sections[(at + 1) % sections.len()];
    }

    /// `←`: the previous section, to the last before the first.
    pub fn prev_section(&mut self) {
        let sections = self.key.sections();
        let at = sections
            .iter()
            .position(|&s| s == self.section)
            .unwrap_or(0);
        self.section = sections[(at + sections.len() - 1) % sections.len()];
    }

    /// Scrolls by `delta` rows (negative: up), never past either end.
    pub fn scroll_by(&mut self, delta: i32) {
        let target = i32::from(self.scroll()) + delta;
        // In `0..=max_scroll` after the clamp, so it fits a `u16` again.
        *self.scroll_mut() = target.clamp(0, i32::from(self.max_scroll)) as u16;
    }

    pub fn page_down(&mut self) {
        self.scroll_by(i32::from(self.page));
    }

    pub fn page_up(&mut self) {
        self.scroll_by(-i32::from(self.page));
    }

    pub fn home(&mut self) {
        *self.scroll_mut() = 0;
    }

    pub fn end(&mut self) {
        *self.scroll_mut() = self.max_scroll;
    }

    /// What `enter` opens: the GitHub page of the section on screen.
    /// Nothing until the first answer brings the row's URL.
    pub fn url(&self) -> Option<String> {
        Some(self.detail.as_ref()?.url(self.section))
    }

    /// Takes in an answer. A failure keeps the text already on screen: a
    /// reload that fails must not blank what the user is reading.
    pub fn apply(&mut self, answer: Result<Box<Detail>, String>) {
        self.loading = false;
        match answer {
            Ok(detail) => {
                self.detail = Some(detail);
                self.error = None;
            }
            Err(message) => self.error = Some(message),
        }
    }
}

// --- the overview: facts, then the body ---

/// The facts above the body, one line per question: what state, by whom,
/// from where to where and how big; who reviewed and who is waited on; can
/// it merge.
fn facts_lines(d: &PrDetail) -> Vec<Line<'static>> {
    let files = if d.changed_files == 1 {
        "file"
    } else {
        "files"
    };
    let (state, color) = state_look(d);
    let headline = Line::from(vec![
        Span::styled("● ", Style::new().fg(color)),
        Span::styled(state, Style::new().fg(color)),
        Span::raw(format!(
            " · @{} · {} → {} · ",
            d.author.login, d.head_ref_name, d.base_ref_name
        )),
        Span::styled(format!("+{}", d.additions), Style::new().fg(Color::Green)),
        Span::raw(" "),
        Span::styled(format!("−{}", d.deletions), Style::new().fg(Color::Red)),
        Span::raw(format!(" in {} {files}", d.changed_files)),
    ]);
    vec![
        headline,
        review_fact(d),
        checks_summary(&d.checks()),
        merge_fact(d),
    ]
}

/// The word for the PR's state, and its color: GitHub's own colors.
fn state_look(d: &PrDetail) -> (&'static str, Color) {
    if d.is_draft {
        return ("draft", Color::DarkGray);
    }
    match d.state.as_str() {
        "MERGED" => ("merged", Color::Magenta),
        "CLOSED" => ("closed", Color::Red),
        _ => ("open", Color::Green),
    }
}

/// A fact's label, padded so the values line up: `review   …`.
fn fact_label(label: &str) -> Span<'static> {
    Span::styled(format!("{label:<9}"), Style::new().fg(Color::DarkGray))
}

/// `review   approved · bob approved · waiting on @Venatum`: the
/// decision, what each reviewer said last, and who has not answered yet.
fn review_fact(d: &PrDetail) -> Line<'static> {
    let mut parts: Vec<Span<'static>> = Vec::new();
    // Whole words here, where the table's narrow column says `changes`.
    let decision = match d.review_decision.as_str() {
        "APPROVED" => Some(("approved", Color::Green)),
        "CHANGES_REQUESTED" => Some(("changes requested", Color::Red)),
        "REVIEW_REQUIRED" => Some(("review required", Color::Yellow)),
        _ => None,
    };
    if let Some((word, color)) = decision {
        parts.push(Span::styled(word, Style::new().fg(color)));
    }
    for review in &d.latest_reviews {
        if let Some(said) = review_word(&review.state) {
            parts.push(Span::raw(format!("{} {said}", review.author.login)));
        }
    }
    let waiting: Vec<String> = d
        .review_requests
        .iter()
        .map(|r| {
            if r.login.is_empty() {
                &r.name
            } else {
                &r.login
            }
        })
        .filter(|who| !who.is_empty())
        .map(|who| format!("@{who}"))
        .collect();
    if !waiting.is_empty() {
        parts.push(Span::raw(format!("waiting on {}", waiting.join(", "))));
    }
    if parts.is_empty() {
        parts.push(Span::styled("none yet", Style::new().fg(Color::DarkGray)));
    }

    let mut spans = vec![fact_label("review")];
    for (i, part) in parts.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" · "));
        }
        spans.push(part);
    }
    Line::from(spans)
}

/// What a review's state says, as a verb: `bob approved`. `None` for a
/// draft review, which only its author can see: nothing said yet.
fn review_word(state: &str) -> Option<&'static str> {
    match state {
        "PENDING" => None,
        "APPROVED" => Some("approved"),
        "CHANGES_REQUESTED" => Some("requested changes"),
        "COMMENTED" => Some("commented"),
        "DISMISSED" => Some("dismissed"),
        _ => Some("reviewed"),
    }
}

/// `merge    no conflict · labels ● api`. Only an open PR can conflict;
/// "UNKNOWN" means GitHub has not computed it yet, never a conflict.
fn merge_fact(d: &PrDetail) -> Line<'static> {
    let (word, color) = match d.mergeable.as_str() {
        _ if d.state != "OPEN" => ("-", Color::DarkGray),
        "CONFLICTING" => ("! conflict", Color::Red),
        "MERGEABLE" => ("no conflict", Color::Green),
        _ => ("computing…", Color::DarkGray),
    };
    let mut spans = vec![
        fact_label("merge"),
        Span::styled(word, Style::new().fg(color)),
    ];
    if !d.labels.is_empty() {
        spans.push(Span::raw(" · labels "));
        spans.extend(label_spans(&d.labels));
    }
    Line::from(spans)
}

/// The Overview section: the facts, a rule `width` wide, the body.
pub fn overview_lines(d: &PrDetail, width: u16) -> Vec<Line<'static>> {
    // GitHub's own words for a PR without a description.
    facts_then_body(facts_lines(d), &d.body, width, "No description provided.")
}

/// An overview's shape, whatever the row: `facts`, a rule `width` wide,
/// then `body` through the markdown pass — or `empty`, in gray, when there
/// is none.
fn facts_then_body(
    mut lines: Vec<Line<'static>>,
    body: &str,
    width: u16,
    empty: &'static str,
) -> Vec<Line<'static>> {
    lines.push(Line::styled(
        "─".repeat(usize::from(width)),
        Style::new().fg(Color::DarkGray),
    ));
    lines.push(Line::default());
    let body = markdown_lines(body);
    if body.is_empty() {
        lines.push(Line::styled(empty, Style::new().fg(Color::DarkGray)));
    }
    lines.extend(body);
    lines
}

// --- the section bar ---

/// ` Overview   Checks 12 `: the view's sections, the one on screen
/// highlighted like the header's active tab, each list with how many rows
/// it holds.
pub fn section_bar(active: Section, sections: &[Section], detail: &Detail) -> Line<'static> {
    let mut spans = Vec::new();
    for (i, &section) in sections.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" "));
        }
        let label = match detail.count(section) {
            Some(count) => format!("{} {count}", section.label()),
            None => section.label().to_string(),
        };
        let style = if section == active {
            Style::new().fg(Color::Black).bg(Color::Cyan).bold()
        } else {
            Style::new().fg(Color::DarkGray)
        };
        spans.push(Span::styled(format!(" {label} "), style));
    }
    Line::from(spans)
}

// --- the checks ---

/// The rollup reduced to what `gh pr checks` lists: the latest attempt of
/// each check (`model::latest_attempts`, the very dedupe behind the
/// `State` column), failures first, then by name.
pub fn latest_checks(rollup: &[Check]) -> Vec<&Check> {
    let mut checks = latest_attempts(rollup);
    checks.sort_by_cached_key(|check| (check.bucket(), check_name(check)));
    checks
}

/// `CI / test (ubuntu-latest)` for a job of a workflow, `deploy (status)`
/// for a commit status.
fn check_name(check: &Check) -> String {
    // An empty string reads as absent: GitHub sends both.
    let given = |field: &Option<String>| field.clone().filter(|value| !value.is_empty());
    match (
        given(&check.workflow_name),
        given(&check.name),
        given(&check.context),
    ) {
        (Some(workflow), Some(name), _) => format!("{workflow} / {name}"),
        (None, Some(name), _) => name,
        (_, None, Some(context)) => format!("{context} (status)"),
        _ => "(unnamed check)".to_string(),
    }
}

/// A time `gh` sent, or `None` for none: missing, unparsable, or the year
/// 1 GitHub sends for "not yet".
fn check_time(at: Option<&str>) -> Option<DateTime<Utc>> {
    let at = DateTime::parse_from_rfc3339(at?).ok()?.with_timezone(&Utc);
    (at.year() >= 2000).then_some(at)
}

/// How long a check took (`4m12s`), or since when it runs (`for 6m`);
/// empty when it never started, or was skipped. `now` is a parameter so
/// tests pin it.
fn duration_label(check: &Check, now: DateTime<Utc>) -> String {
    match check.bucket() {
        Bucket::Skip => String::new(),
        bucket => elapsed_label(
            check.started_at.as_deref(),
            check.completed_at.as_deref(),
            bucket == Bucket::Pending,
            now,
        ),
    }
}

/// `4m12s` from `started` to `finished`, or `for 6m` while it still
/// `running`; empty without a start (queued, or never run). Shared by the
/// checks of a PR and the jobs and steps of a run.
fn elapsed_label(
    started: Option<&str>,
    finished: Option<&str>,
    running: bool,
    now: DateTime<Utc>,
) -> String {
    let Some(started) = check_time(started) else {
        return String::new();
    };
    if running {
        let minutes = (now - started).num_minutes().max(0);
        return format!("for {minutes}m");
    }
    match check_time(finished) {
        Some(done) if done >= started => {
            let seconds = (done - started).num_seconds();
            format!("{}m{:02}s", seconds / 60, seconds % 60)
        }
        _ => String::new(),
    }
}

/// The glyph, the word and the color of a check, as the Actions tab draws
/// a run (`columns::run_look`).
fn check_look(check: &Check) -> (&'static str, &'static str, Color) {
    match check.bucket() {
        Bucket::Fail => ("✗", "failure", Color::Red),
        Bucket::Pending if check.status == "IN_PROGRESS" => ("●", "running", Color::Yellow),
        Bucket::Pending => ("●", "pending", Color::Yellow),
        Bucket::Pass => ("✓", "success", Color::Green),
        Bucket::Cancel => ("-", "cancelled", Color::DarkGray),
        Bucket::Skip => ("·", "skipped", Color::DarkGray),
    }
}

/// The Overview's `checks` fact: how many in each bucket, the empty ones
/// left out.
fn checks_summary(checks: &[&Check]) -> Line<'static> {
    let count = |bucket| checks.iter().filter(|c| c.bucket() == bucket).count();
    let parts: Vec<Span<'static>> = [
        (Bucket::Fail, "✗ ", "failing", Color::Red),
        (Bucket::Pending, "● ", "running", Color::Yellow),
        (Bucket::Pass, "✓ ", "passed", Color::Green),
        (Bucket::Cancel, "", "cancelled", Color::DarkGray),
        (Bucket::Skip, "", "skipped", Color::DarkGray),
    ]
    .into_iter()
    .filter_map(|(bucket, glyph, word, color)| {
        let n = count(bucket);
        (n > 0).then(|| Span::styled(format!("{glyph}{n} {word}"), Style::new().fg(color)))
    })
    .collect();

    let mut spans = vec![fact_label("checks")];
    if parts.is_empty() {
        spans.push(Span::styled("none", Style::new().fg(Color::DarkGray)));
    }
    for (i, part) in parts.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" · "));
        }
        spans.push(part);
    }
    Line::from(spans)
}

/// `name` cut to `width` characters, an ellipsis marking the cut.
fn cut_end(name: &str, width: usize) -> String {
    if name.chars().count() <= width {
        return name.to_string();
    }
    let kept: String = name.chars().take(width.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// The Checks section, `width` columns wide: one line per check (glyph,
/// word, name, duration), then how many, and how many older attempts the
/// list leaves out.
pub fn checks_lines(d: &PrDetail, now: DateTime<Utc>, width: u16) -> Vec<Line<'static>> {
    let checks = d.checks();
    if checks.is_empty() {
        return vec![Line::styled(
            "No checks on this PR.",
            Style::new().fg(Color::DarkGray),
        )];
    }
    // As wide as the longest name, so the durations sit next to the names,
    // but no wider than what `✗ failure   ` before it and ` for 6m` after
    // it leave: one check, one line.
    let longest = checks
        .iter()
        .map(|check| check_name(check).chars().count())
        .max()
        .unwrap_or(0);
    let name_width = longest.min(usize::from(width).saturating_sub(20).max(10));
    let mut lines: Vec<Line<'static>> = checks
        .iter()
        .map(|check| {
            let (glyph, word, color) = check_look(check);
            Line::from(vec![
                Span::styled(format!("{glyph} {word:<9}"), Style::new().fg(color)),
                Span::raw(format!(
                    " {:<name_width$} {:>7}",
                    cut_end(&check_name(check), name_width),
                    duration_label(check, now)
                )),
            ])
        })
        .collect();

    let all = d.status_check_rollup.as_ref().map_or(0, Vec::len);
    let folded = all - checks.len();
    let mut total = format!(
        "{} {}",
        checks.len(),
        if checks.len() == 1 { "check" } else { "checks" }
    );
    if folded > 0 {
        let attempts = if folded == 1 { "attempt" } else { "attempts" };
        total.push_str(&format!(" · {folded} older {attempts} folded"));
    }
    // One check's own page needs a cursor in the list: v1 opens them all.
    total.push_str(" · enter opens the Checks page on GitHub");
    lines.push(Line::default());
    lines.push(Line::styled(total, Style::new().fg(Color::DarkGray)));
    lines
}

// --- the files ---

/// How wide the `+++−−` bar of the Files section is.
const BAR_WIDTH: usize = 24;

/// `A`dded, `D`eleted, `R`enamed, `C`opied, or `M`odified, as `git status`
/// writes them.
fn change_letter(change_type: &str) -> char {
    match change_type {
        "ADDED" => 'A',
        "DELETED" => 'D',
        "RENAMED" => 'R',
        "COPIED" => 'C',
        _ => 'M',
    }
}

/// The `+` and `−` halves of a file's bar, `git diff --stat` style: scaled
/// to the biggest file of the PR, so the bars compare. A change of one
/// line still shows one character.
fn stat_bar(additions: u64, deletions: u64, biggest: u64, width: usize) -> (String, String) {
    if biggest == 0 {
        return (String::new(), String::new());
    }
    let scale = |lines: u64| match lines {
        0 => 0,
        // At most `width` since `lines <= biggest`: the cast cannot cut.
        _ => (lines * width as u64 / biggest).max(1) as usize,
    };
    ("+".repeat(scale(additions)), "−".repeat(scale(deletions)))
}

/// `path` cut to `width` characters from the LEFT, an ellipsis marking the
/// cut: the end of a path is the part that names the file.
fn cut_start(path: &str, width: usize) -> String {
    let length = path.chars().count();
    if length <= width {
        return path.to_string();
    }
    let kept: String = path.chars().skip(length + 1 - width).collect();
    format!("…{kept}")
}

/// The Files section, `width` columns wide: one line per file (change,
/// path, `+n −n`, bar), then the totals. `gh` lists 100 files at most: the
/// last line says when some are missing.
pub fn files_lines(d: &PrDetail, width: u16) -> Vec<Line<'static>> {
    if d.files.is_empty() {
        return vec![Line::styled(
            "No files changed.",
            Style::new().fg(Color::DarkGray),
        )];
    }
    // As wide as the longest path, so the counts sit next to the paths, but
    // no wider than what `A  ` before it, ` +n −n  ` and the bar after it
    // leave: one file, one line.
    let longest = d
        .files
        .iter()
        .map(|f| f.path.chars().count())
        .max()
        .unwrap_or(0);
    let room = usize::from(width).saturating_sub(3 + 1 + 5 + 1 + 5 + 2 + BAR_WIDTH);
    let path_width = longest.min(room.max(16));
    let biggest = d
        .files
        .iter()
        .map(|f| f.additions + f.deletions)
        .max()
        .unwrap_or(0);
    let mut lines: Vec<Line<'static>> = d
        .files
        .iter()
        .map(|f| {
            let (plus, minus) = stat_bar(f.additions, f.deletions, biggest, BAR_WIDTH);
            let letter = change_letter(&f.change_type);
            let letter_color = match letter {
                'A' => Color::Green,
                'D' => Color::Red,
                'M' => Color::Yellow,
                _ => Color::Cyan,
            };
            Line::from(vec![
                Span::styled(format!("{letter}  "), Style::new().fg(letter_color)),
                Span::raw(format!("{:<path_width$} ", cut_start(&f.path, path_width))),
                Span::styled(
                    format!("{:>5}", format!("+{}", f.additions)),
                    Style::new().fg(Color::Green),
                ),
                Span::raw(" "),
                Span::styled(
                    format!("{:>5}", format!("−{}", f.deletions)),
                    Style::new().fg(Color::Red),
                ),
                Span::raw("  "),
                Span::styled(plus, Style::new().fg(Color::Green)),
                Span::styled(minus, Style::new().fg(Color::Red)),
            ])
        })
        .collect();

    let shown = d.files.len() as u64;
    let count = if shown < d.changed_files {
        format!("showing {shown} of {} files", d.changed_files)
    } else {
        format!("{shown} {}", if shown == 1 { "file" } else { "files" })
    };
    lines.push(Line::default());
    lines.push(Line::styled(
        format!(
            "{count} · +{} −{} · enter opens the Files page on GitHub",
            d.additions, d.deletions
        ),
        Style::new().fg(Color::DarkGray),
    ));
    lines
}

// --- the comments ---

/// One entry of the Comments section: a comment of the conversation, or a
/// review. Borrowed from the `PrDetail`, nothing copied.
struct Entry<'a> {
    who: &'a str,
    /// The review's verdict (`approved`…); `None` for a plain comment.
    what: Option<&'static str>,
    /// When, as `gh` sent it (RFC 3339, UTC).
    at: &'a str,
    body: &'a str,
    /// Why a maintainer hid it; `None` when it shows.
    hidden: Option<&'a str>,
    /// Folded to its header line (see `review_entry`).
    folded: bool,
    /// Made on an older commit than the head.
    older: bool,
}

/// The conversation and every review, oldest first.
fn timeline(d: &PrDetail) -> Vec<Entry<'_>> {
    let comments = d.comments.iter().map(comment_entry);
    let reviews = d
        .reviews
        .iter()
        .filter_map(|r| review_entry(r, &d.head_ref_oid));
    let mut entries: Vec<Entry> = comments.chain(reviews).collect();
    // RFC 3339 times in UTC sort as text.
    entries.sort_by(|a, b| a.at.cmp(b.at));
    entries
}

/// A comment of the conversation as an entry: never folded, but hidden
/// when a maintainer minimized it.
fn comment_entry(c: &Comment) -> Entry<'_> {
    Entry {
        who: &c.author.login,
        what: None,
        at: &c.created_at,
        body: &c.body,
        hidden: c.is_minimized.then_some(c.minimized_reason.as_str()),
        folded: false,
        older: false,
    }
}

/// A review as an entry, folded to one line when there is nothing to read
/// or it is no longer current: no summary (a bare approval, or comments
/// on the diff only), dismissed, or made on an older commit — GitHub does
/// not tell `gh pr view` which threads are resolved, and a review of code
/// that has moved on since is the closest sign. `None` for a pending
/// review, the reviewer's own unpublished draft.
fn review_entry<'a>(r: &'a Review, head: &str) -> Option<Entry<'a>> {
    let what = review_word(&r.state)?;
    let commit = r.commit.as_ref().map_or("", |c| c.oid.as_str());
    let older = !commit.is_empty() && !head.is_empty() && commit != head;
    let silent = r.body.trim().is_empty();
    Some(Entry {
        who: &r.author.login,
        // A silent "commented" review is made of comments on the diff.
        what: Some(if silent && what == "commented" {
            "commented on the diff"
        } else {
            what
        }),
        at: &r.submitted_at,
        body: &r.body,
        hidden: None,
        folded: silent || older || r.state == "DISMISSED",
        older,
    })
}

/// `2026-10-02 18:40` for `2026-10-02T16:40:00Z` at UTC+2: what the clock
/// said where the user is. Left as sent when it does not parse.
fn local_time(at: &str, offset: FixedOffset) -> String {
    match DateTime::parse_from_rfc3339(at) {
        Ok(time) => time
            .with_timezone(&offset)
            .format("%Y-%m-%d %H:%M")
            .to_string(),
        Err(_) => at.to_string(),
    }
}

/// The Comments section: each entry as a bold `@who · what · when` header
/// over its body, indented, through the same markdown pass as the PR's. A
/// folded entry is its header alone, gray, behind `▸` — consecutive ones
/// stay together — and `show_folded` (`space`) opens the ones with
/// something to read, behind `▾`. `offset` is the user's time zone, a
/// parameter so tests pin it.
pub fn comments_lines(d: &PrDetail, offset: FixedOffset, show_folded: bool) -> Vec<Line<'static>> {
    entries_lines(&timeline(d), offset, show_folded)
}

/// A conversation's lines, whatever the row it belongs to (see
/// `comments_lines`).
fn entries_lines(entries: &[Entry], offset: FixedOffset, show_folded: bool) -> Vec<Line<'static>> {
    if entries.is_empty() {
        return vec![Line::styled(
            "No comments yet.",
            Style::new().fg(Color::DarkGray),
        )];
    }
    let gray = Style::new().fg(Color::DarkGray);
    let mut lines = Vec::new();
    let mut after_folded_line = false;
    for entry in entries {
        let open = !entry.folded || (show_folded && !entry.body.trim().is_empty());
        let one_line = !open;
        // A blank line between entries, except between two folded lines.
        if !lines.is_empty() && !(one_line && after_folded_line) {
            lines.push(Line::default());
        }
        after_folded_line = one_line;

        let mut header = Vec::new();
        if entry.folded {
            header.push(Span::styled(if open { "▾ " } else { "▸ " }, gray));
        }
        let who = format!("@{}", entry.who);
        header.push(if one_line {
            Span::styled(who, gray)
        } else {
            Span::styled(who, Style::new().bold())
        });
        if let Some(what) = entry.what {
            let what = format!(" · {what}");
            header.push(if one_line {
                Span::styled(what, gray)
            } else {
                Span::raw(what)
            });
        }
        header.push(Span::styled(
            format!(" · {}", local_time(entry.at, offset)),
            gray,
        ));
        if entry.older {
            header.push(Span::styled(" · older commit", gray));
        }
        lines.push(Line::from(header));
        if one_line {
            continue;
        }

        if let Some(reason) = entry.hidden {
            let why = match reason.to_lowercase().replace('_', " ") {
                reason if reason.is_empty() => "(hidden)".to_string(),
                reason => format!("(hidden: {reason})"),
            };
            lines.push(Line::styled(format!("  {why}"), gray));
            continue;
        }
        for line in markdown_lines(entry.body) {
            // Indented under its header; a blank line stays blank.
            if line.width() == 0 {
                lines.push(line);
            } else {
                let mut spans = vec![Span::raw("  ")];
                spans.extend(line.spans);
                lines.push(Line::from(spans).style(line.style));
            }
        }
    }
    lines
}

// --- markdown, lightly ---

/// Makes untrusted text safe and faithful to draw. ratatui already drops
/// every control character (so no escape sequence reaches the terminal),
/// but silently: a tab-indented code block would lose its indentation and
/// a lone `\r` would glue two lines. So line ends are normalized and tabs
/// expanded first. Bidirectional overrides are not control characters and
/// would get through, reordering what a line shows ("trojan source"):
/// they go too.
fn sanitize(text: &str) -> String {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\t', "    ")
        .chars()
        .filter(|&c| c == '\n' || !(c.is_control() || is_bidi_override(c)))
        .collect()
}

/// The embeddings, overrides and isolates of Unicode's bidi algorithm.
fn is_bidi_override(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// A line that opens or closes a fenced code block.
fn is_fence(line: &str) -> bool {
    let line = line.trim_start();
    line.starts_with("```") || line.starts_with("~~~")
}

/// Removes every `<!-- … -->`, even across lines, as GitHub hides them. A
/// PR template is mostly such comments: without this pass the screen shows
/// the template's instructions instead of the PR. An unterminated comment
/// hides the rest, as on GitHub. Fenced code is left alone: a comment
/// there is code.
fn strip_html_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_comment = false;
    let mut in_fence = false;
    for (i, line) in text.split('\n').enumerate() {
        // Inside a comment, the line break goes with the hidden text: the
        // text before the comment and the text after it join on one line.
        if i > 0 && !in_comment {
            out.push('\n');
        }
        // A fence line, and everything between two of them, is kept as is.
        if !in_comment && is_fence(line) {
            in_fence = !in_fence;
            out.push_str(line);
            continue;
        }
        if in_fence {
            out.push_str(line);
            continue;
        }
        let mut rest = line;
        loop {
            if in_comment {
                let Some(end) = rest.find("-->") else {
                    break; // the whole rest of the line is hidden
                };
                rest = &rest[end + "-->".len()..];
                in_comment = false;
            } else if let Some(start) = rest.find("<!--") {
                out.push_str(&rest[..start]);
                rest = &rest[start + "<!--".len()..];
                in_comment = true;
            } else {
                out.push_str(rest);
                break;
            }
        }
    }
    out
}

/// `![alt](url)` and `<img … alt="alt">` become `[image: alt]`: a long URL
/// means nothing in a terminal.
fn replace_images(line: &str) -> String {
    let mut out = String::new();
    let mut rest = line;
    loop {
        // Whichever of the two comes first, and what it would turn into.
        let (at, image) = match (rest.find("!["), rest.find("<img")) {
            (Some(md), Some(html)) if html < md => (html, html_image(&rest[html..])),
            (Some(md), _) => (md, markdown_image(&rest[md..])),
            (None, Some(html)) => (html, html_image(&rest[html..])),
            (None, None) => break,
        };
        match image {
            Some((alt, len)) => {
                out.push_str(&rest[..at]);
                out.push_str(&image_label(alt));
                rest = &rest[at + len..];
            }
            // Not an image after all (`![b] c`): keep the opener as typed
            // and look further.
            None => {
                out.push_str(&rest[..at + 2]);
                rest = &rest[at + 2..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// `![alt](url)` at the start of `s`: its alt text and its length.
fn markdown_image(s: &str) -> Option<(&str, usize)> {
    let close = s.find("](")?;
    let end = close + 2 + s[close + 2..].find(')')?;
    Some((&s[2..close], end + 1))
}

/// `<img …>` at the start of `s`: its `alt="…"` (or nothing) and its length.
fn html_image(s: &str) -> Option<(&str, usize)> {
    let end = s.find('>')?;
    let tag = &s[..end];
    let alt = tag
        .find("alt=\"")
        .and_then(|at| {
            let value = &tag[at + "alt=\"".len()..];
            value.find('"').map(|close| &value[..close])
        })
        .unwrap_or_default();
    Some((alt, end + 1))
}

fn image_label(alt: &str) -> String {
    match alt.trim() {
        "" => "[image]".to_string(),
        alt => format!("[image: {alt}]"),
    }
}

/// A PR body as lines to draw, lightly styled, one source line at a time:
/// headings, list items, quotes, rules and fenced code read at a glance;
/// inline markup (`**`, `` ` ``, links) stays as typed. Line breaks are
/// kept, as GitHub shows them in a PR body: wrapping is the `Paragraph`'s
/// job.
pub fn markdown_lines(body: &str) -> Vec<Line<'static>> {
    let text = strip_html_comments(&sanitize(body));
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        if is_fence(line) {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            lines.push(Line::styled(
                format!("    {line}"),
                Style::new().fg(Color::Yellow),
            ));
        } else if line.trim().is_empty() {
            push_blank(&mut lines);
        } else if let Some(title) = heading(line) {
            // Air around a heading, as GitHub gives it.
            push_blank(&mut lines);
            lines.push(Line::styled(
                format!("▍{title}"),
                Style::new().bold().fg(Color::Cyan),
            ));
            push_blank(&mut lines);
        } else {
            lines.push(prose_line(line));
        }
    }
    while lines.last().is_some_and(|line| line.width() == 0) {
        lines.pop();
    }
    lines
}

/// A blank line, unless the text starts here or one is already there: runs
/// of blank lines collapse to one.
fn push_blank(lines: &mut Vec<Line<'static>>) {
    if lines.last().is_some_and(|line| line.width() > 0) {
        lines.push(Line::default());
    }
}

/// The text of a `#` … `######` heading.
fn heading(line: &str) -> Option<&str> {
    let line = line.trim_start();
    let level = line.chars().take_while(|&c| c == '#').count();
    let title = line[level..].strip_prefix(' ')?;
    (1..=6).contains(&level).then(|| title.trim())
}

/// `---`, `***` or `___` (three or more): a thematic break.
fn is_rule(line: &str) -> bool {
    let line = line.trim();
    line.len() >= 3
        && ['-', '*', '_']
            .iter()
            .any(|&mark| line.chars().all(|c| c == mark))
}

/// Any line outside a fence that is not blank nor a heading.
fn prose_line(line: &str) -> Line<'static> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    if is_rule(trimmed) {
        return Line::styled("─".repeat(40), Style::new().fg(Color::DarkGray));
    }
    if let Some(item) = ["- ", "* ", "+ "]
        .iter()
        .find_map(|bullet| trimmed.strip_prefix(bullet))
    {
        // Two columns per nesting level, as the source indents them.
        let pad = "  ".repeat(indent / 2 + 1);
        let is_task = ["[ ] ", "[x] ", "[X] "]
            .iter()
            .any(|task| item.starts_with(task));
        let bullet = if is_task { "" } else { "• " };
        return Line::raw(format!("{pad}{bullet}{}", replace_images(item)));
    }
    if let Some(quoted) = trimmed.strip_prefix('>') {
        return Line::styled(
            format!("│ {}", replace_images(quoted.trim_start())),
            Style::new().fg(Color::DarkGray),
        );
    }
    Line::raw(replace_images(line))
}

/// A `PrDetail` from JSON, the way `gh` sends it, for the tests of every
/// module (as `issues::sample_issue`).
#[cfg(test)]
pub fn sample_detail() -> PrDetail {
    serde_json::from_str(
        r#"{"title": "Add rate limiting", "url": "https://github.com/acme/api/pull/412",
            "body": "Adds a limiter.",
            "state": "OPEN", "isDraft": false, "author": {"login": "alice"},
            "baseRefName": "main", "headRefName": "feat/rate-limit",
            "additions": 248, "deletions": 31, "changedFiles": 9,
            "reviewDecision": "APPROVED", "mergeable": "MERGEABLE",
            "labels": [{"name": "enhancement", "color": "a2eeef"}],
            "reviewRequests": [{"__typename": "User", "login": "Venatum"}],
            "headRefOid": "c0ffee",
            "latestReviews": [
                {"author": {"login": "bob"}, "state": "APPROVED",
                 "body": "Nice and small.", "submittedAt": "2026-10-02T16:40:00Z"},
                {"author": {"login": "carol"}, "state": "COMMENTED",
                 "body": "Per key or per org?", "submittedAt": "2026-10-03T10:05:00Z"}
            ],
            "reviews": [
                {"author": {"login": "bob"}, "state": "APPROVED", "body": "Nice and small.",
                 "submittedAt": "2026-10-02T16:40:00Z", "commit": {"oid": "c0ffee"}},
                {"author": {"login": "carol"}, "state": "COMMENTED", "body": "Per key or per org?",
                 "submittedAt": "2026-10-03T10:05:00Z", "commit": {"oid": "c0ffee"}}
            ]}"#,
    )
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(repo: &str, number: u64) -> DetailKey {
        DetailKey::Pr {
            repo: repo.into(),
            number,
        }
    }

    /// The fields come camelCase from `gh`, and the body keeps its CRLF:
    /// cleaning it is the renderer's job, not the parser's.
    #[test]
    fn a_pr_view_answer_deserializes() {
        let d: PrDetail = serde_json::from_str(
            // Three `#`: the body's `"##` would end a `r#"…"#` or `r##"…"##`.
            r###"{"title": "Add rate limiting", "url": "https://github.com/acme/api/pull/412",
                "body": "## Summary\r\n\r\nAdds a limiter.",
                "state": "OPEN", "isDraft": false, "author": {"login": "alice"},
                "baseRefName": "main", "headRefName": "feat/rate-limit",
                "additions": 248, "deletions": 31, "changedFiles": 9}"###,
        )
        .unwrap();
        assert_eq!(d.title, "Add rate limiting");
        assert_eq!(d.body, "## Summary\r\n\r\nAdds a limiter.");
        // What a PR nobody reviewed nor labelled leaves out: defaults.
        assert!(d.labels.is_empty() && d.latest_reviews.is_empty());
        assert_eq!(d.review_decision, "");
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn sanitize_normalizes_line_ends_expands_tabs_and_drops_bidi_overrides() {
        // ratatui drops \t and \r itself (control characters), silently: a
        // tab-indented code block would lose its indentation.
        assert_eq!(sanitize("a\r\n\tb\rc"), "a\n    b\nc");
        // U+202E reorders what a line shows ("trojan source"): gone.
        assert_eq!(sanitize("x\u{202e}y\u{2066}z"), "xyz");
        // Other control characters (an ESC sequence) go too.
        assert_eq!(sanitize("\u{1b}[31mred"), "[31mred");
    }

    #[test]
    fn html_comments_vanish_even_across_lines() {
        assert_eq!(strip_html_comments("a<!-- x -->b"), "ab");
        assert_eq!(strip_html_comments("a\n<!--\nfill\nthis\n-->\nb"), "a\n\nb");
        assert_eq!(strip_html_comments("a<!-- 1 -->b<!-- 2 -->c"), "abc");
        // Unterminated: GitHub hides the rest, so do we.
        assert_eq!(strip_html_comments("a<!-- never closed\nb"), "a");
    }

    #[test]
    fn a_comment_inside_a_fence_is_code_not_a_comment() {
        assert_eq!(
            strip_html_comments("```\n<!-- x -->\n```"),
            "```\n<!-- x -->\n```"
        );
    }

    #[test]
    fn images_become_their_alt_text() {
        assert_eq!(
            replace_images("see ![the dashboard](https://x/y.png)."),
            "see [image: the dashboard]."
        );
        assert_eq!(replace_images("![](https://x/y.png)"), "[image]");
        assert_eq!(
            replace_images(r#"<img src="https://x" alt="logo" width="40">"#),
            "[image: logo]"
        );
        assert_eq!(replace_images(r#"<img src="https://x">"#), "[image]");
        // Not an image after all: left as typed.
        assert_eq!(replace_images("a ![b] c"), "a ![b] c");
    }

    /// A PR template is mostly comments: what is left is the PR.
    #[test]
    fn a_template_body_reads_as_its_headings_and_text() {
        let body = "<!--\r\nThanks! Fill every section.\r\n-->\r\n\r\n## Summary\r\n\r\nAdds a limiter.\r\n\r\n\r\n\r\n## How to test\r\n<!-- paste commands -->\r\n```sh\r\ncargo test\r\n```\r\n";
        assert_eq!(
            texts(&markdown_lines(body)),
            // The comment-only line leaves a blank, kept (runs collapse to one).
            [
                "▍Summary",
                "",
                "Adds a limiter.",
                "",
                "▍How to test",
                "",
                "    cargo test"
            ]
        );
    }

    #[test]
    fn lists_quotes_and_rules_get_a_light_touch() {
        let md = "- one\n  * nested\n- [x] done\n> quoted\n---";
        assert_eq!(
            texts(&markdown_lines(md)),
            [
                "  • one",
                "    • nested",
                "  [x] done",
                "│ quoted",
                &"─".repeat(40)
            ]
        );
    }

    #[test]
    fn nothing_inside_a_fence_is_interpreted() {
        let md = "```\n# not a heading\n<!-- kept -->\n\tindented\n```";
        assert_eq!(
            texts(&markdown_lines(md)),
            [
                "    # not a heading",
                "    <!-- kept -->",
                "        indented"
            ]
        );
    }

    /// An untouched template is as empty as no body at all. Said in the
    /// Overview only: a comment's empty body just shows nothing.
    #[test]
    fn an_empty_body_says_so() {
        assert!(markdown_lines("<!-- only the template -->\r\n").is_empty());
        let mut d = sample_detail();
        d.body.clear();
        let overview = texts(&overview_lines(&d, 20));
        assert_eq!(overview.last().unwrap(), "No description provided.");
    }

    /// The facts line starting with `label` (a later section adds lines:
    /// tests look them up by label, not by index).
    fn fact(d: &PrDetail, label: &str) -> String {
        texts(&facts_lines(d))
            .into_iter()
            .find(|l| l.starts_with(label))
            .unwrap_or_else(|| panic!("no `{label}` line"))
    }

    #[test]
    fn the_facts_say_who_where_how_big_and_who_is_waited_on() {
        let d = sample_detail();
        assert_eq!(
            texts(&facts_lines(&d))[0],
            "● open · @alice · feat/rate-limit → main · +248 −31 in 9 files"
        );
        assert_eq!(
            fact(&d, "review"),
            "review   approved · bob approved · carol commented · waiting on @Venatum"
        );
        assert_eq!(
            fact(&d, "merge"),
            "merge    no conflict · labels ● enhancement"
        );
    }

    #[test]
    fn a_pr_nobody_reviewed_says_so() {
        let mut d = sample_detail();
        d.review_decision.clear();
        d.latest_reviews.clear();
        d.review_requests.clear();
        assert_eq!(fact(&d, "review"), "review   none yet");
    }

    /// A team has a name, not a login.
    #[test]
    fn a_team_is_waited_on_by_its_name() {
        let mut d = sample_detail();
        d.review_requests = serde_json::from_str(
            r#"[{"__typename": "User", "login": "Venatum"},
                {"__typename": "Team", "name": "core"}]"#,
        )
        .unwrap();
        assert!(fact(&d, "review").ends_with("waiting on @Venatum, @core"));
    }

    #[test]
    fn the_state_word_covers_draft_merged_and_closed() {
        let mut d = sample_detail();
        d.is_draft = true;
        assert!(texts(&facts_lines(&d))[0].starts_with("● draft"));
        d.is_draft = false;
        d.state = "MERGED".into();
        assert!(texts(&facts_lines(&d))[0].starts_with("● merged"));
        d.state = "CLOSED".into();
        assert!(texts(&facts_lines(&d))[0].starts_with("● closed"));
    }

    #[test]
    fn a_conflict_and_an_unknown_merge_state_read_differently() {
        let mut d = sample_detail();
        d.mergeable = "CONFLICTING".into();
        assert!(fact(&d, "merge").starts_with("merge    ! conflict"));
        // GitHub computes it lazily: UNKNOWN is never a red "conflict".
        d.mergeable = "UNKNOWN".into();
        assert!(fact(&d, "merge").starts_with("merge    computing…"));
        // A merged PR has nothing left to merge.
        d.state = "MERGED".into();
        assert!(fact(&d, "merge").starts_with("merge    -"));
    }

    #[test]
    fn the_overview_is_the_facts_a_rule_then_the_body() {
        let lines = texts(&overview_lines(&sample_detail(), 20));
        let rule = lines
            .iter()
            .position(|l| l == &"─".repeat(20))
            .expect("a rule as wide as the view");
        assert_eq!(lines[rule + 1], "");
        assert_eq!(lines[rule + 2], "Adds a limiter.");
    }

    /// A reload that fails must not blank a screen the user is reading.
    #[test]
    fn a_failed_reload_keeps_what_is_on_screen() {
        let mut view = DetailView::new(key("api", 412), "t".into());
        view.apply(Ok(Box::new(Detail::Pr(sample_detail()))));
        view.loading = true;
        view.apply(Err("`gh pr view` failed: HTTP 502".into()));
        assert!(view.detail.is_some(), "the text stays");
        assert_eq!(view.error.as_deref(), Some("`gh pr view` failed: HTTP 502"));
        assert!(!view.loading);
    }

    /// `max_scroll` and `page` are what the last render measured.
    #[test]
    fn scrolling_stops_at_both_ends() {
        let mut view = DetailView::new(key("api", 412), "t".into());
        view.max_scroll = 30;
        view.page = 10;
        view.scroll_by(-1);
        assert_eq!(view.scroll(), 0);
        view.page_down();
        view.page_down();
        assert_eq!(view.scroll(), 20);
        view.page_down();
        view.page_down();
        assert_eq!(view.scroll(), 30, "the last screenful, not past it");
        view.page_up();
        assert_eq!(view.scroll(), 20);
        view.home();
        assert_eq!(view.scroll(), 0);
        view.end();
        assert_eq!(view.scroll(), 30);
    }

    // --- sections and checks ---

    fn run(workflow: &str, name: &str, status: &str, conclusion: &str, started: &str) -> Check {
        Check {
            workflow_name: Some(workflow.into()),
            name: Some(name.into()),
            status: status.into(),
            conclusion: conclusion.into(),
            started_at: Some(started.into()),
            ..Default::default()
        }
    }

    fn at(time: &str) -> DateTime<Utc> {
        time.parse().unwrap()
    }

    /// The rollup keeps every attempt of the head commit; `gh pr checks`
    /// shows the latest of each (32 → 20 measured on cli/cli#14564). A job
    /// that failed then was re-run green is green. Failures come first.
    #[test]
    fn a_rerun_supersedes_the_earlier_run_of_the_same_check() {
        let rollup = [
            run("CI", "test", "COMPLETED", "FAILURE", "2026-10-04T09:00:00Z"),
            run("CI", "test", "COMPLETED", "SUCCESS", "2026-10-04T10:00:00Z"),
            run(
                "Lint",
                "test",
                "COMPLETED",
                "FAILURE",
                "2026-10-04T09:00:00Z",
            ),
        ];
        let latest = latest_checks(&rollup);
        assert_eq!(latest.len(), 2, "same name, other workflow: another check");
        assert_eq!(check_name(latest[0]), "Lint / test");
        assert_eq!(latest[0].bucket(), Bucket::Fail);
        assert_eq!(check_name(latest[1]), "CI / test");
        assert_eq!(latest[1].bucket(), Bucket::Pass, "the re-run");
    }

    #[test]
    fn checks_sort_by_bucket_then_by_name() {
        let rollup = [
            run("CI", "zeta", "COMPLETED", "SKIPPED", "t"),
            run("CI", "beta", "COMPLETED", "SUCCESS", "t"),
            run("CI", "alpha", "COMPLETED", "SUCCESS", "t"),
            run("CI", "gamma", "IN_PROGRESS", "", "t"),
        ];
        let names: Vec<String> = latest_checks(&rollup).into_iter().map(check_name).collect();
        assert_eq!(
            names,
            ["CI / gamma", "CI / alpha", "CI / beta", "CI / zeta"]
        );
    }

    #[test]
    fn a_status_context_is_named_after_its_context() {
        let status = Check {
            context: Some("deploy-preview".into()),
            state: "SUCCESS".into(),
            ..Default::default()
        };
        assert_eq!(check_name(&status), "deploy-preview (status)");
        // A check run outside any workflow (a third-party app).
        let app = Check {
            name: Some("codecov/patch".into()),
            ..Default::default()
        };
        assert_eq!(check_name(&app), "codecov/patch");
    }

    #[test]
    fn a_duration_reads_in_minutes_and_a_running_check_says_since_when() {
        let now = at("2026-10-04T09:18:00Z");
        let mut done = run("CI", "test", "COMPLETED", "SUCCESS", "2026-10-04T09:12:00Z");
        done.completed_at = Some("2026-10-04T09:16:12Z".into());
        assert_eq!(duration_label(&done, now), "4m12s");
        let running = run("CI", "test", "IN_PROGRESS", "", "2026-10-04T09:12:00Z");
        assert_eq!(duration_label(&running, now), "for 6m");
        // GitHub sends year 1 for a check that never started.
        let queued = run("CI", "test", "QUEUED", "", "0001-01-01T00:00:00Z");
        assert_eq!(duration_label(&queued, now), "");
        // A commit status has no times at all.
        assert_eq!(duration_label(&Check::default(), now), "");
        // A skipped job "ran" for no time: `0m00s` would only be noise.
        let mut skipped = run(
            "Docs",
            "links",
            "COMPLETED",
            "SKIPPED",
            "2026-10-04T09:12:00Z",
        );
        skipped.completed_at = Some("2026-10-04T09:12:00Z".into());
        assert_eq!(duration_label(&skipped, now), "");
    }

    #[test]
    fn the_checks_fact_counts_each_bucket() {
        let rollup = [
            run("CI", "a", "COMPLETED", "FAILURE", "t"),
            run("CI", "b", "IN_PROGRESS", "", "t"),
            run("CI", "c", "COMPLETED", "SUCCESS", "t"),
            run("CI", "d", "COMPLETED", "SKIPPED", "t"),
        ];
        let summary = checks_summary(&latest_checks(&rollup)).to_string();
        assert_eq!(
            summary,
            "checks   ✗ 1 failing · ● 1 running · ✓ 1 passed · 1 skipped"
        );
        assert_eq!(checks_summary(&[]).to_string(), "checks   none");
        // A bucket with nothing in it is not mentioned.
        let green = [run("CI", "a", "COMPLETED", "SUCCESS", "t")];
        assert_eq!(
            checks_summary(&latest_checks(&green)).to_string(),
            "checks   ✓ 1 passed"
        );
    }

    /// The Overview's `checks` fact sits between the review and the merge
    /// state.
    #[test]
    fn the_overview_counts_the_checks() {
        let mut d = sample_detail();
        d.status_check_rollup = Some(vec![
            run("CI", "test", "COMPLETED", "FAILURE", "2026-10-04T09:00:00Z"),
            run("CI", "test", "COMPLETED", "SUCCESS", "2026-10-04T10:00:00Z"),
        ]);
        let facts = texts(&facts_lines(&d));
        assert_eq!(facts[2], "checks   ✓ 1 passed");
        assert!(facts[3].starts_with("merge"));
    }

    #[test]
    fn a_null_rollup_means_no_checks() {
        let d: PrDetail = serde_json::from_str(
            r#"{"title": "t", "url": "u", "state": "OPEN", "isDraft": false,
                "author": {"login": "alice"}, "additions": 0, "deletions": 0,
                "changedFiles": 0, "statusCheckRollup": null}"#,
        )
        .unwrap();
        assert!(d.checks().is_empty());
        assert_eq!(
            texts(&checks_lines(&d, at("2026-10-04T09:00:00Z"), 80)),
            ["No checks on this PR."]
        );
    }

    #[test]
    fn one_line_per_check_then_how_many() {
        let mut d = sample_detail();
        let mut done = run("CI", "test", "COMPLETED", "FAILURE", "2026-10-04T09:12:00Z");
        done.completed_at = Some("2026-10-04T09:16:12Z".into());
        d.status_check_rollup = Some(vec![
            done,
            run("CI", "test", "COMPLETED", "SUCCESS", "2026-10-04T08:00:00Z"),
            run("CI", "lint", "IN_PROGRESS", "", "2026-10-04T09:12:00Z"),
        ]);
        let lines = texts(&checks_lines(&d, at("2026-10-04T09:18:00Z"), 60));
        // The name column is as wide as the longest name (9), not as the 40
        // columns 60 would leave it: the durations sit next to the names.
        assert_eq!(
            lines,
            [
                format!("✗ failure   {:<9} {:>7}", "CI / test", "4m12s"),
                format!("● running   {:<9} {:>7}", "CI / lint", "for 6m"),
                String::new(),
                "2 checks · 1 older attempt folded · enter opens the Checks page on GitHub"
                    .to_string(),
            ]
        );
    }

    /// A name too long for its column is cut, never wrapped onto a second
    /// row: one check, one line.
    #[test]
    fn a_long_check_name_is_cut() {
        let mut d = sample_detail();
        d.status_check_rollup = Some(vec![run(
            "Integration tests",
            "postgres (ubuntu-latest, 16, with-extensions)",
            "COMPLETED",
            "SUCCESS",
            "t",
        )]);
        let first = texts(&checks_lines(&d, at("2026-10-04T09:18:00Z"), 50))[0].clone();
        assert_eq!(first.chars().count(), 50, "{first}");
        assert!(first.contains("Integration tests / postgres… "), "{first}");
    }

    #[test]
    fn sections_cycle_and_each_keeps_its_scroll() {
        let mut view = DetailView::new(key("api", 412), "t".into());
        view.max_scroll = 50;
        view.scroll_by(7);
        view.next_section();
        assert_eq!(view.section, Section::Checks);
        assert_eq!(view.scroll(), 0);
        view.prev_section();
        assert_eq!(view.scroll(), 7, "back where we were");
        view.prev_section();
        assert_eq!(view.section, *PR_SECTIONS.last().unwrap(), "wraps around");
    }

    /// The section bar reads like the header's tabs: the active one on
    /// cyan, each with how many rows it holds.
    #[test]
    fn the_section_bar_names_the_sections_with_their_counts() {
        let mut d = sample_detail();
        d.status_check_rollup = Some(vec![run("CI", "a", "COMPLETED", "SUCCESS", "t")]);
        let bar = section_bar(Section::Overview, &PR_SECTIONS, &Detail::Pr(d));
        assert_eq!(
            bar.to_string(),
            " Overview   Checks 1   Files 9   Comments 2 "
        );
        let active = bar
            .spans
            .iter()
            .find(|span| span.content.contains("Overview"))
            .unwrap();
        assert_eq!(active.style.bg, Some(Color::Cyan));
    }

    // --- files ---

    fn file(path: &str, additions: u64, deletions: u64, change_type: &str) -> ChangedFile {
        ChangedFile {
            path: path.into(),
            additions,
            deletions,
            change_type: change_type.into(),
        }
    }

    #[test]
    fn change_types_read_as_one_letter() {
        assert_eq!(change_letter("ADDED"), 'A');
        assert_eq!(change_letter("DELETED"), 'D');
        assert_eq!(change_letter("RENAMED"), 'R');
        assert_eq!(change_letter("COPIED"), 'C');
        assert_eq!(change_letter("MODIFIED"), 'M');
        assert_eq!(change_letter("CHANGED"), 'M');
    }

    #[test]
    fn the_stat_bar_scales_to_the_biggest_file_and_never_hides_a_change() {
        assert_eq!(stat_bar(98, 0, 98, 24), ("+".repeat(24), String::new()));
        assert_eq!(
            stat_bar(1, 0, 98, 24),
            ("+".into(), String::new()),
            "1 line still shows"
        );
        assert_eq!(stat_bar(9, 4, 98, 24), ("++".into(), "−".into()));
        assert_eq!(stat_bar(0, 0, 0, 24), (String::new(), String::new()));
    }

    /// The end of a path is the part that names the file: a path too long
    /// for its column loses its start.
    #[test]
    fn a_long_path_is_cut_from_the_left() {
        assert_eq!(cut_start("src/limit/bucket.rs", 30), "src/limit/bucket.rs");
        assert_eq!(
            cut_start("crates/api/src/limit/bucket.rs", 16),
            "…limit/bucket.rs"
        );
    }

    #[test]
    fn one_line_per_file_then_the_totals() {
        let mut d = sample_detail();
        d.changed_files = 2;
        d.files = vec![
            file("src/limit/bucket.rs", 98, 0, "ADDED"),
            file("src/throttle.rs", 0, 26, "DELETED"),
        ];
        let lines = texts(&files_lines(&d, 70));
        // The path column is as wide as the longest path (19), so the
        // counts sit right after it: 70 columns would leave it 29.
        assert_eq!(
            lines,
            [
                format!(
                    "A  {:<19} {:>5} {:>5}  {}",
                    "src/limit/bucket.rs",
                    "+98",
                    "−0",
                    "+".repeat(24)
                ),
                format!(
                    "D  {:<19} {:>5} {:>5}  {}",
                    "src/throttle.rs",
                    "+0",
                    "−26",
                    "−".repeat(6)
                ),
                String::new(),
                "2 files · +248 −31 · enter opens the Files page on GitHub".to_string(),
            ]
        );
    }

    /// `gh` lists 100 files at most (cli/cli#14515: 960 changed, 100 listed).
    #[test]
    fn a_capped_file_list_says_how_many_are_missing() {
        let mut d = sample_detail();
        d.changed_files = 960;
        d.files = (0..100)
            .map(|i| file(&format!("f{i}"), 1, 0, "ADDED"))
            .collect();
        let last = files_lines(&d, 80).last().unwrap().to_string();
        assert_eq!(
            last,
            "showing 100 of 960 files · +248 −31 · enter opens the Files page on GitHub"
        );
    }

    #[test]
    fn files_come_after_checks() {
        let mut view = DetailView::new(key("api", 412), "t".into());
        view.next_section();
        view.next_section();
        assert_eq!(view.section, Section::Files);
    }

    // --- comments ---

    /// `sample_detail` (bob approved on 10-02 and carol commented on
    /// 10-03, both with a summary), plus the conversation and a review
    /// without a summary.
    fn conversation() -> PrDetail {
        let mut d = sample_detail();
        d.comments = serde_json::from_str(
            r#"[{"author": {"login": "alice"}, "createdAt": "2026-10-03T11:22:00Z",
                 "body": "Per key on purpose for v1."},
                {"author": {"login": "app/github-actions"}, "createdAt": "2026-10-04T09:20:00Z",
                 "body": "<!-- bot marker -->\r\nPreview deployed."}]"#,
        )
        .unwrap();
        d.reviews.push(review(
            r#"{"author": {"login": "dave"}, "state": "APPROVED", "body": "",
                "submittedAt": "2026-10-04T08:00:00Z", "commit": {"oid": "c0ffee"}}"#,
        ));
        d
    }

    fn review(json: &str) -> Review {
        serde_json::from_str(json).unwrap()
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    /// Every review is in, a bare approval too: "Comments 0" on a PR full
    /// of reviews read as if nobody had looked at it.
    #[test]
    fn comments_and_every_review_interleave_by_date() {
        let d = conversation();
        let who: Vec<&str> = timeline(&d).iter().map(|e| e.who).collect();
        assert_eq!(who, ["bob", "carol", "alice", "dave", "app/github-actions"]);
    }

    /// A review with nothing to read — a bare approval, or one made only of
    /// comments on the diff, which `gh pr view` does not bring — is one
    /// gray line, and consecutive ones stay together.
    #[test]
    fn a_review_without_a_summary_is_one_folded_line() {
        let mut d = conversation();
        d.reviews.push(review(
            r#"{"author": {"login": "erin"}, "state": "COMMENTED", "body": "",
                "submittedAt": "2026-10-04T08:30:00Z", "commit": {"oid": "c0ffee"}}"#,
        ));
        let lines = texts(&comments_lines(&d, utc(), false));
        let dave = lines.iter().position(|l| l.contains("@dave")).unwrap();
        assert_eq!(lines[dave], "▸ @dave · approved · 2026-10-04 08:00");
        assert_eq!(
            lines[dave + 1],
            "▸ @erin · commented on the diff · 2026-10-04 08:30"
        );
    }

    /// GitHub does not tell `gh pr view` which threads are resolved. A
    /// review of an older commit is the closest sign: the code moved on
    /// since. It folds to its header until `space`.
    #[test]
    fn a_review_of_an_older_commit_folds_until_asked() {
        let mut d = conversation();
        d.reviews[1].commit = Some(ReviewCommit { oid: "0ld".into() });

        let folded = texts(&comments_lines(&d, utc(), false));
        assert!(
            folded.contains(&"▸ @carol · commented · 2026-10-03 10:05 · older commit".to_string())
        );
        assert!(!folded.iter().any(|l| l.contains("Per key or per org?")));

        let unfolded = texts(&comments_lines(&d, utc(), true));
        let carol = unfolded.iter().position(|l| l.contains("@carol")).unwrap();
        assert_eq!(
            unfolded[carol],
            "▾ @carol · commented · 2026-10-03 10:05 · older commit"
        );
        assert_eq!(unfolded[carol + 1], "  Per key or per org?");
    }

    #[test]
    fn a_dismissed_review_is_folded() {
        let mut d = conversation();
        d.reviews[0].state = "DISMISSED".into();
        let lines = texts(&comments_lines(&d, utc(), false));
        assert_eq!(lines[0], "▸ @bob · dismissed · 2026-10-02 16:40");
        assert!(!lines.iter().any(|l| l.contains("Nice and small.")));
    }

    /// A draft review is the reviewer's own, unpublished: not shown.
    #[test]
    fn a_pending_review_is_left_out() {
        let mut d = conversation();
        d.reviews[0].state = "PENDING".into();
        assert!(!timeline(&d).iter().any(|e| e.who == "bob"));
    }

    #[test]
    fn space_folds_and_unfolds() {
        let mut view = DetailView::new(key("api", 412), "t".into());
        assert!(!view.show_folded);
        view.toggle_folded();
        assert!(view.show_folded);
    }

    #[test]
    fn a_comment_header_reads_who_what_and_when() {
        let lines = texts(&comments_lines(&conversation(), utc(), false));
        assert_eq!(lines[0], "@bob · approved · 2026-10-02 16:40");
        assert_eq!(lines[1], "  Nice and small.");
        assert_eq!(lines[2], "");
        assert_eq!(lines[3], "@carol · commented · 2026-10-03 10:05");
        // A conversation comment is no review: no verdict.
        assert_eq!(lines[6], "@alice · 2026-10-03 11:22");
    }

    /// `gh` sends UTC; the view shows the time where the user is.
    #[test]
    fn a_comment_time_is_local() {
        let paris = FixedOffset::east_opt(2 * 3600).unwrap();
        let lines = texts(&comments_lines(&conversation(), paris, false));
        assert_eq!(lines[0], "@bob · approved · 2026-10-02 18:40");
    }

    #[test]
    fn comment_bodies_go_through_the_same_markdown_pass() {
        let lines = texts(&comments_lines(&conversation(), utc(), false));
        let bot = lines
            .iter()
            .position(|l| l.starts_with("@app/github-actions"))
            .unwrap();
        assert_eq!(lines[bot + 1..], ["  Preview deployed."]);
    }

    #[test]
    fn a_hidden_comment_shows_why_not_what() {
        let mut d = conversation();
        d.comments[0].is_minimized = true;
        d.comments[0].minimized_reason = "OFF_TOPIC".into();
        let lines = texts(&comments_lines(&d, utc(), false));
        assert!(lines.contains(&"  (hidden: off topic)".to_string()));
        assert!(!lines.iter().any(|l| l.contains("Per key on purpose")));
    }

    #[test]
    fn no_conversation_says_so() {
        let mut d = sample_detail();
        d.reviews.clear();
        assert_eq!(
            texts(&comments_lines(&d, utc(), false)),
            ["No comments yet."]
        );
    }

    #[test]
    fn comments_come_last() {
        let mut view = DetailView::new(key("api", 412), "t".into());
        view.prev_section();
        assert_eq!(view.section, Section::Comments);
    }

    // --- the browser ---

    #[test]
    fn enter_opens_the_page_of_the_section() {
        let mut view = DetailView::new(key("api", 412), "t".into());
        assert_eq!(view.url(), None, "nothing loaded yet");
        view.apply(Ok(Box::new(Detail::Pr(sample_detail()))));
        let base = "https://github.com/acme/api/pull/412";
        assert_eq!(view.url().as_deref(), Some(base));
        view.next_section();
        assert_eq!(view.url(), Some(format!("{base}/checks")));
        view.next_section();
        assert_eq!(view.url(), Some(format!("{base}/files")));
        view.next_section();
        assert_eq!(view.url().as_deref(), Some(base), "the conversation");
    }
}
