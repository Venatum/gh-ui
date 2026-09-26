//! The filters: their state, how we evolve them from the keyboard, how we
//! translate them into `gh` arguments, and how we save/reload them.

use crate::config;
use serde::{Deserialize, Serialize};

/// Time window, equivalent to `--since`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum Since {
    #[default]
    Off,
    D1,
    D3,
    W1,
    W2,
    M1,
}

impl Since {
    /// Number of days to subtract, or `None` if disabled.
    fn days(self) -> Option<i64> {
        match self {
            Since::Off => None,
            Since::D1 => Some(1),
            Since::D3 => Some(3),
            Since::W1 => Some(7),
            Since::W2 => Some(14),
            Since::M1 => Some(30),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Since::Off => "off",
            Since::D1 => "1d",
            Since::D3 => "3d",
            Since::W1 => "1w",
            Since::W2 => "2w",
            Since::M1 => "1m",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Since::Off => Since::D1,
            Since::D1 => Since::D3,
            Since::D3 => Since::W1,
            Since::W1 => Since::W2,
            Since::W2 => Since::M1,
            Since::M1 => Since::Off,
        }
    }
    pub fn prev(self) -> Self {
        match self {
            Since::Off => Since::M1,
            Since::D1 => Since::Off,
            Since::D3 => Since::D1,
            Since::W1 => Since::D3,
            Since::W2 => Since::W1,
            Since::M1 => Since::W2,
        }
    }

    /// The `updated:>=YYYY-MM-DD` search qualifier, or `None` when off.
    /// Shared by the PR and the issue filters.
    pub fn updated_qualifier(self) -> Option<String> {
        let days = self.days()?;
        // Cutoff date = today - N days. `chrono` handles the calendar; a
        // NaiveDate already displays in YYYY-MM-DD format.
        let cutoff = chrono::Local::now().date_naive() - chrono::Duration::days(days);
        Some(format!("updated:>={cutoff}"))
    }
}

/// `gh`'s keyword for the logged-in user. An ordinary login value here, exactly
/// as `gh` treats it: it goes into the query as is.
pub const ME: &str = "@me";

/// The `author:` qualifier, sign included. One field for "me", "not me" and
/// any other login, so two of them can never be set at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AuthorFilter {
    /// No author qualifier.
    #[default]
    Any,
    /// `--author <login>`.
    Is(String),
    /// `-author:<login>`, which `gh pr list` only accepts inside `--search`.
    IsNot(String),
}

impl AuthorFilter {
    pub fn me() -> Self {
        AuthorFilter::Is(ME.to_string())
    }
    pub fn not_me() -> Self {
        AuthorFilter::IsNot(ME.to_string())
    }

    /// Reads the author prompt: `octocat`, `-octocat` to exclude, blank for
    /// any. The GitHub search syntax, so there is nothing new to learn.
    pub fn parse(text: &str) -> Self {
        let text = text.trim();
        let (negated, login) = match text.strip_prefix('-') {
            Some(rest) => (true, rest.trim_start()),
            None => (false, text),
        };
        // The author column prints `@octocat`, so users type it, but `gh`
        // wants the bare login. `@me` is `gh`'s own keyword: it keeps its `@`.
        let login = if login == ME {
            login
        } else {
            login.strip_prefix('@').unwrap_or(login)
        };
        if login.is_empty() {
            return AuthorFilter::Any;
        }
        if negated {
            AuthorFilter::IsNot(login.to_string())
        } else {
            AuthorFilter::Is(login.to_string())
        }
    }

    /// What the prompt opens with. `parse(to_input())` gives the value back.
    pub fn to_input(&self) -> String {
        match self {
            AuthorFilter::Any => String::new(),
            AuthorFilter::Is(login) => login.clone(),
            AuthorFilter::IsNot(login) => format!("-{login}"),
        }
    }

    /// The value in the summary line: `any`, `@me`, `-@me`, `octocat`.
    pub fn summary(&self) -> String {
        match self {
            AuthorFilter::Any => "any".to_string(),
            _ => self.to_input(),
        }
    }

    /// ←/→ in the panel: any → @me → not @me → any. A typed login sits at the
    /// end of the cycle, so leaving it forgets it (it can be typed again).
    pub fn next(&self) -> Self {
        match self {
            AuthorFilter::Any => AuthorFilter::me(),
            AuthorFilter::Is(login) if login == ME => AuthorFilter::not_me(),
            // not @me, or a typed login: back to the start.
            _ => AuthorFilter::Any,
        }
    }
    pub fn prev(&self) -> Self {
        match self {
            AuthorFilter::Any => AuthorFilter::not_me(),
            AuthorFilter::IsNot(login) if login == ME => AuthorFilter::me(),
            AuthorFilter::Is(login) if login == ME => AuthorFilter::Any,
            // A typed login is the last value: the one before it is not @me.
            _ => AuthorFilter::not_me(),
        }
    }

    /// Adds this author to a `gh … list` command line: a plain `--author`,
    /// or an exclusion in the search — the list commands have no flag for
    /// one. Shared by the PR and the issue filters.
    pub fn push_gh_args(&self, args: &mut Vec<String>, search: &mut Vec<String>) {
        match self {
            AuthorFilter::Any => {}
            AuthorFilter::Is(login) => {
                args.push("--author".to_string());
                args.push(login.clone());
            }
            AuthorFilter::IsNot(login) => search.push(format!("-author:{login}")),
        }
    }
}

/// The repo selection after `current`: all → repo0 → repo1 → … → all.
/// Free functions rather than `Filters` methods: the issue filters hold a
/// selection of their own and cycle it the same way.
pub fn next_repo(current: Option<&str>, repos: &[String]) -> Option<String> {
    match current {
        None => repos.first().cloned(),
        Some(current) => match repos.iter().position(|r| r == current) {
            // not the last → the next one
            Some(i) if i + 1 < repos.len() => Some(repos[i + 1].clone()),
            // last (or not found) → back to "all"
            _ => None,
        },
    }
}

/// Same cycle, backwards: all → last → … → repo0 → all.
pub fn prev_repo(current: Option<&str>, repos: &[String]) -> Option<String> {
    match current {
        None => repos.last().cloned(),
        Some(current) => match repos.iter().position(|r| r == current) {
            Some(0) | None => None, // first (or not found) → "all"
            Some(i) => Some(repos[i - 1].clone()),
        },
    }
}

/// Drops a repo selection that this folder does not hold, and returns the
/// name we let go. The config is global while a selection names a folder,
/// so a selection made in `~/dev/perso` would filter EVERYTHING out once
/// gh-ui is pointed at `~/dev/client` — with nothing on screen saying why.
///
/// An empty `repos` is left alone on purpose: it means either an empty
/// folder or a `read_dir` that failed, and there is nothing to show in
/// either case. Nor does the caller save afterwards: the selection stays
/// valid in the folder it was made for, and each run repairs itself in
/// memory.
pub fn reconcile_repo_selection(
    selection: &mut Option<String>,
    repos: &[String],
) -> Option<String> {
    if repos.is_empty() {
        return None;
    }
    // `take_if` hands us the value only when the closure says so, leaving
    // the selection as `None` in that case — exactly the fallback we want.
    selection.take_if(|sel| !repos.contains(sel))
}

/// The set of active filters. `Default` gives the "everything, nothing checked" state.
///
/// One field per `gh` search qualifier, so two fields can never fight over the
/// same one. Read through `StoredFilters`, which also understands the files
/// written before `author` took over `filter: Me` and `not_mine`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(from = "StoredFilters")]
pub struct Filters {
    /// The `author:` qualifier, sign included.
    pub author: AuthorFilter,
    /// `review-requested:@me`.
    pub review_requested: bool,
    pub no_draft: bool,
    pub unreviewed: bool,
    pub since: Since,
    /// `None` = all repos; `Some(name)` = a single one.
    pub repo: Option<String>,
    /// Filter by labels (logical AND). Empty = no filter.
    pub labels: Vec<String>,
}

impl Filters {
    // --- keyboard-driven mutations ---
    pub fn toggle_no_draft(&mut self) {
        self.no_draft = !self.no_draft;
    }
    pub fn toggle_unreviewed(&mut self) {
        self.unreviewed = !self.unreviewed;
    }
    pub fn toggle_review_requested(&mut self) {
        self.review_requested = !self.review_requested;
    }
    pub fn cycle_author(&mut self) {
        self.author = self.author.next();
    }
    pub fn cycle_author_back(&mut self) {
        self.author = self.author.prev();
    }
    /// The `m` shortcut: the `Mine` preset on, or the author back to `any`.
    /// Neither direction touches the other filters.
    pub fn toggle_mine(&mut self) {
        if Preset::Mine.is_active(self) {
            self.author = AuthorFilter::Any;
        } else {
            Preset::Mine.apply(self);
        }
    }
    pub fn cycle_since(&mut self) {
        self.since = self.since.next();
    }
    pub fn cycle_since_back(&mut self) {
        self.since = self.since.prev();
    }

    /// Scrolls through the repo selection: all → repo0 → repo1 → … → all.
    pub fn cycle_repo(&mut self, repos: &[String]) {
        self.repo = next_repo(self.repo.as_deref(), repos);
    }

    /// Same cycle, backwards: all → last → … → repo0 → all.
    pub fn cycle_repo_prev(&mut self, repos: &[String]) {
        self.repo = prev_repo(self.repo.as_deref(), repos);
    }

    /// See `reconcile_repo_selection`.
    pub fn reconcile_repo(&mut self, repos: &[String]) -> Option<String> {
        reconcile_repo_selection(&mut self.repo, repos)
    }

    /// The part of the summary that ALSO applies to the Actions tab.
    /// Today only the repo filter restricts both flows.
    pub fn summary_common(&self) -> String {
        match &self.repo {
            Some(r) => format!("repo:{r}"),
            None => "repo:all".to_string(),
        }
    }

    /// The part that only goes to `gh pr list`, read like a GitHub query:
    /// "author:@me · since:1w · no-draft". `author:` always comes first, so the
    /// line is never empty and says at once whose PRs are shown.
    pub fn summary_prs(&self) -> String {
        let mut parts = vec![format!("author:{}", self.author.summary())];
        if self.review_requested {
            parts.push("review-asked".to_string());
        }
        if self.since != Since::Off {
            parts.push(format!("since:{}", self.since.label()));
        }
        if self.no_draft {
            parts.push("no-draft".to_string());
        }
        if self.unreviewed {
            parts.push("unreviewed".to_string());
        }
        if !self.labels.is_empty() {
            parts.push(format!("labels:{}", self.labels.join(",")));
        }
        parts.join(" · ")
    }

    /// Stores the author typed in the prompt (see `AuthorFilter::parse`).
    pub fn set_author(&mut self, text: &str) {
        self.author = AuthorFilter::parse(text);
    }

    /// Stores the labels from an input (separated by spaces).
    pub fn set_labels(&mut self, value: &str) {
        self.labels = value.split_whitespace().map(str::to_string).collect();
    }

    /// Translates the filters into `gh pr list` arguments (except the repo, which
    /// is handled elsewhere since it chooses WHICH directories to scan). Reproduces
    /// the script's logic.
    pub fn to_gh_args(&self) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        // `search`: the qualifiers joined into a single `--search "a b c"`.
        let mut search: Vec<String> = Vec::new();

        self.author.push_gh_args(&mut args, &mut search);
        if self.review_requested {
            search.push("review-requested:@me".to_string());
        }

        // Labels: one `--label` per label (gh combines them with AND).
        for label in &self.labels {
            args.push("--label".to_string());
            args.push(label.clone());
        }

        if let Some(updated) = self.since.updated_qualifier() {
            search.push(updated);
        }

        if self.no_draft {
            search.push("draft:false".to_string());
        }
        if self.unreviewed {
            search.push("-reviewed-by:@me".to_string());
        }

        if !search.is_empty() {
            args.push("--search".to_string());
            args.push(search.join(" "));
        }
        args
    }

    // --- persistence ---

    /// Reloads the filters, or returns the default values if missing/unreadable.
    pub fn load() -> Self {
        // `let ... else`: if the config can't be found, we bail out returning
        // the defaults. Otherwise we continue with `path`.
        let Some(path) = config::path("filters.json") else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Saves the filters as JSON (silent on failure).
    pub fn save(&self) {
        let Some(path) = config::path("filters.json") else {
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

// `#[cfg(test)]`: this module is compiled ONLY for `cargo test`.

/// A named set of filter values, applied on top of the filters — what
/// `gh pr status` does with its fixed "mine" / "review requested" queries. A
/// preset may set several filters at once; that is its point over a plain
/// filter value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Preset {
    /// My own PRs: `author:@me`, without "review requested", which would
    /// empty the list (nobody requests their own review).
    Mine,
}

impl Preset {
    pub fn apply(self, f: &mut Filters) {
        match self {
            Preset::Mine => {
                f.author = AuthorFilter::me();
                f.review_requested = false;
            }
        }
    }

    pub fn is_active(self, f: &Filters) -> bool {
        match self {
            Preset::Mine => f.author == AuthorFilter::me() && !f.review_requested,
        }
    }
}

/// What a filters file may hold: the current fields, or the legacy ones
/// written before `author` became an `AuthorFilter`. `Filters` deserializes
/// through it, so an old file keeps working and is converted on the next save.
#[derive(Deserialize, Default)]
#[serde(default)]
struct StoredFilters {
    /// Legacy only, and written by every older version: its presence is how
    /// we tell the two formats apart.
    filter: Option<LegacyMode>,
    /// Legacy only.
    not_mine: bool,
    /// A login string or `null` in a legacy file, an `AuthorFilter` in a new
    /// one. `AuthorFilter::Any` is the string `"Any"`, so the shape alone is
    /// ambiguous: `filter` decides how to read it.
    author: serde_json::Value,
    review_requested: bool,
    no_draft: bool,
    unreviewed: bool,
    since: Since,
    repo: Option<String>,
    labels: Vec<String>,
}

/// The old `filter` field.
#[derive(Deserialize, Clone, Copy, PartialEq)]
enum LegacyMode {
    All,
    Me,
    ReviewAsked,
}

impl From<StoredFilters> for Filters {
    fn from(s: StoredFilters) -> Self {
        let (author, review_requested) = match s.filter {
            Some(mode) => (
                legacy_author(&s.author, mode, s.not_mine),
                mode == LegacyMode::ReviewAsked,
            ),
            // A malformed author loses only itself, not the whole file.
            None => (
                serde_json::from_value(s.author).unwrap_or_default(),
                s.review_requested,
            ),
        };
        Filters {
            author,
            review_requested,
            no_draft: s.no_draft,
            unreviewed: s.unreviewed,
            since: s.since,
            repo: s.repo,
            labels: s.labels,
        }
    }
}

/// The legacy author, with the precedence the old `to_gh_args` applied: a
/// typed author won over `me`, and `me` + `not_mine` was an empty list anyway.
/// The typed login goes through the prompt rules, so `@octocat` is fixed too.
fn legacy_author(author: &serde_json::Value, mode: LegacyMode, not_mine: bool) -> AuthorFilter {
    match author.as_str().map(AuthorFilter::parse) {
        Some(typed) if typed != AuthorFilter::Any => typed,
        _ if mode == LegacyMode::Me => AuthorFilter::me(),
        _ if not_mine => AuthorFilter::not_me(),
        _ => AuthorFilter::Any,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_splits_common_from_pr_only_filters() {
        let f = Filters {
            repo: Some("api".to_string()),
            no_draft: true,
            author: AuthorFilter::Is("moi".to_string()),
            ..Default::default()
        };

        // "Common" = what also applies to the Actions tab: the repo, and only it.
        assert_eq!(f.summary_common(), "repo:api");

        // "PRs" = everything that is only sent to `gh pr list`.
        let prs = f.summary_prs();
        assert!(prs.contains("no-draft"), "got {prs}");
        assert!(prs.contains("author:moi"), "got {prs}");
        assert!(!prs.contains("repo:"), "repo must not appear twice: {prs}");
    }

    #[test]
    fn default_filters_produce_no_gh_args() {
        assert!(Filters::default().to_gh_args().is_empty());
    }

    #[test]
    fn reproduces_the_usual_command() {
        // ./list-prs.sh --since 1w --no-draft --unreviewed --not-mine
        let f = Filters {
            since: Since::W1,
            no_draft: true,
            unreviewed: true,
            author: AuthorFilter::not_me(),
            ..Default::default()
        };
        let args = f.to_gh_args();
        // a single --search containing all the qualifiers
        let search_pos = args.iter().position(|a| a == "--search").unwrap();
        let search = &args[search_pos + 1];
        assert!(search.contains("updated:>="));
        assert!(search.contains("draft:false"));
        assert!(search.contains("-reviewed-by:@me"));
        assert!(search.contains("-author:@me"));
    }

    #[test]
    fn cycle_repo_wraps_from_all_to_all() {
        let repos = vec!["a".to_string(), "b".to_string()];
        let mut f = Filters::default();
        assert_eq!(f.repo, None);
        f.cycle_repo(&repos);
        assert_eq!(f.repo.as_deref(), Some("a"));
        f.cycle_repo(&repos);
        assert_eq!(f.repo.as_deref(), Some("b"));
        f.cycle_repo(&repos);
        assert_eq!(f.repo, None); // after the last, back to "all"
    }

    #[test]
    fn cycle_repo_prev_wraps_backwards() {
        let repos = vec!["a".to_string(), "b".to_string()];
        let mut f = Filters::default();
        assert_eq!(f.repo, None);
        f.cycle_repo_prev(&repos);
        assert_eq!(f.repo.as_deref(), Some("b")); // all → last
        f.cycle_repo_prev(&repos);
        assert_eq!(f.repo.as_deref(), Some("a"));
        f.cycle_repo_prev(&repos);
        assert_eq!(f.repo, None); // first → all
    }

    #[test]
    fn cycle_repo_without_repos_stays_all() {
        // repo that no longer exists, and no list of repos available
        let mut f = Filters {
            repo: Some("ghost".to_string()),
            ..Default::default()
        };
        f.cycle_repo(&[]);
        assert_eq!(f.repo, None);
    }

    #[test]
    fn set_author_parses_the_prompt() {
        let mut f = Filters::default();
        f.set_author("  -octocat  ");
        assert_eq!(f.author, AuthorFilter::IsNot("octocat".to_string()));
        f.set_author("   ");
        assert_eq!(f.author, AuthorFilter::Any); // empty input → no filter
    }

    #[test]
    fn set_labels_splits_on_whitespace() {
        let mut f = Filters::default();
        f.set_labels("  bug   api ");
        assert_eq!(f.labels, vec!["bug".to_string(), "api".to_string()]);
        f.set_labels("");
        assert!(f.labels.is_empty());
    }

    #[test]
    fn serde_round_trip_preserves_the_filters() {
        let original = Filters {
            review_requested: true,
            no_draft: true,
            since: Since::W1,
            repo: Some("hello-world".to_string()),
            author: AuthorFilter::Is("octocat".to_string()),
            labels: vec!["bug".to_string()],
            ..Default::default()
        };
        // serialize to JSON then deserialize again: we must get back the identical value.
        let json = serde_json::to_string(&original).unwrap();
        let round_trip: Filters = serde_json::from_str(&json).unwrap();
        assert_eq!(original, round_trip);
    }

    #[test]
    fn labels_produce_label_flags() {
        let f = Filters {
            labels: vec!["bug".to_string(), "api".to_string()],
            ..Default::default()
        };
        let args = f.to_gh_args();
        // two --label <value> pairs
        let labels: Vec<&String> = args
            .iter()
            .zip(args.iter().skip(1))
            .filter(|(k, _)| *k == "--label")
            .map(|(_, v)| v)
            .collect();
        assert_eq!(labels, vec!["bug", "api"]);
    }

    /// A repo filter is a FOLDER NAME, but the config is global: pointing
    /// gh-ui at another folder would keep filtering on a repo that is not
    /// there, silently emptying both tabs. This is the guard against that.
    #[test]
    fn a_repo_missing_from_the_folder_is_dropped() {
        let mut filters = Filters {
            repo: Some("api".to_string()),
            ..Filters::default()
        };
        let here = vec!["docs".to_string(), "web".to_string()];

        assert_eq!(filters.reconcile_repo(&here), Some("api".to_string()));
        assert_eq!(filters.repo, None, "it must fall back to all repos");
    }

    #[test]
    fn a_repo_still_present_is_kept() {
        let mut filters = Filters {
            repo: Some("web".to_string()),
            ..Filters::default()
        };
        let here = vec!["docs".to_string(), "web".to_string()];

        assert_eq!(filters.reconcile_repo(&here), None);
        assert_eq!(filters.repo, Some("web".to_string()));
    }

    /// No repo discovered means either an empty folder or a `read_dir` that
    /// failed. Dropping the selection on a transient error would lose it for
    /// nothing, and there is no PR to show either way.
    #[test]
    fn an_empty_repo_list_leaves_the_selection_alone() {
        let mut filters = Filters {
            repo: Some("api".to_string()),
            ..Filters::default()
        };

        assert_eq!(filters.reconcile_repo(&[]), None);
        assert_eq!(filters.repo, Some("api".to_string()));
    }

    #[test]
    fn no_selection_is_nothing_to_reconcile() {
        let mut filters = Filters::default();
        assert_eq!(filters.reconcile_repo(&["web".to_string()]), None);
        assert_eq!(filters.repo, None);
    }

    #[test]
    fn parse_blank_or_lone_signs_is_any() {
        for text in ["", "  ", "-", " - ", "@", "-@"] {
            assert_eq!(AuthorFilter::parse(text), AuthorFilter::Any, "{text:?}");
        }
    }

    #[test]
    fn parse_reads_the_sign() {
        assert_eq!(
            AuthorFilter::parse("octocat"),
            AuthorFilter::Is("octocat".to_string())
        );
        assert_eq!(
            AuthorFilter::parse("-octocat"),
            AuthorFilter::IsNot("octocat".to_string())
        );
        assert_eq!(
            AuthorFilter::parse("  octocat  "),
            AuthorFilter::Is("octocat".to_string())
        );
    }

    /// The author column prints `@octocat`, so that is what gets typed — but
    /// `--author @octocat` silently matches nothing. `@me` is `gh`'s own
    /// keyword and must keep its `@`.
    #[test]
    fn parse_strips_the_at_of_a_login_but_not_of_me() {
        assert_eq!(
            AuthorFilter::parse("@octocat"),
            AuthorFilter::Is("octocat".to_string())
        );
        assert_eq!(
            AuthorFilter::parse("-@octocat"),
            AuthorFilter::IsNot("octocat".to_string())
        );
        assert_eq!(AuthorFilter::parse("@me"), AuthorFilter::me());
        assert_eq!(AuthorFilter::parse("-@me"), AuthorFilter::not_me());
    }

    #[test]
    fn parse_tolerates_a_space_after_the_dash() {
        assert_eq!(
            AuthorFilter::parse("- octocat"),
            AuthorFilter::IsNot("octocat".to_string())
        );
    }

    /// Opening the prompt and confirming it untouched must never change the
    /// filter.
    #[test]
    fn to_input_round_trips_through_parse() {
        for author in [
            AuthorFilter::Any,
            AuthorFilter::me(),
            AuthorFilter::not_me(),
            AuthorFilter::Is("octocat".to_string()),
            AuthorFilter::IsNot("octocat".to_string()),
        ] {
            assert_eq!(AuthorFilter::parse(&author.to_input()), author);
        }
    }

    #[test]
    fn summary_reads_like_the_query() {
        assert_eq!(AuthorFilter::Any.summary(), "any");
        assert_eq!(AuthorFilter::me().summary(), "@me");
        assert_eq!(AuthorFilter::not_me().summary(), "-@me");
        assert_eq!(AuthorFilter::Is("octocat".to_string()).summary(), "octocat");
    }

    // --- gh arguments ---

    #[test]
    fn author_is_goes_to_the_author_flag() {
        let f = Filters {
            author: AuthorFilter::Is("octocat".to_string()),
            ..Default::default()
        };
        assert_eq!(f.to_gh_args(), vec!["--author", "octocat"]);
    }

    #[test]
    fn author_is_not_goes_to_the_search() {
        let f = Filters {
            author: AuthorFilter::not_me(),
            ..Default::default()
        };
        assert_eq!(f.to_gh_args(), vec!["--search", "-author:@me"]);
    }

    #[test]
    fn review_requested_goes_to_the_search() {
        let f = Filters {
            review_requested: true,
            ..Default::default()
        };
        assert_eq!(f.to_gh_args(), vec!["--search", "review-requested:@me"]);
    }

    /// The one combination the model still allows that returns nothing:
    /// nobody requests their own review. Accepted on purpose (over-narrow, not
    /// contradictory, and both show in the summary): no cross-field rule.
    #[test]
    fn me_and_review_requested_are_both_sent() {
        let f = Filters {
            author: AuthorFilter::me(),
            review_requested: true,
            ..Default::default()
        };
        assert_eq!(
            f.to_gh_args(),
            vec!["--author", "@me", "--search", "review-requested:@me"]
        );
    }

    // --- summary ---

    #[test]
    fn the_summary_starts_with_the_author() {
        let with = |author| Filters {
            author,
            ..Default::default()
        };
        assert_eq!(Filters::default().summary_prs(), "author:any");
        assert_eq!(with(AuthorFilter::me()).summary_prs(), "author:@me");
        assert_eq!(with(AuthorFilter::not_me()).summary_prs(), "author:-@me");
        let f = Filters {
            review_requested: true,
            ..Default::default()
        };
        assert_eq!(f.summary_prs(), "author:any · review-asked");
    }

    // --- presets ---

    /// Every filter a preset must leave alone, set to a non-default value.
    fn the_rest() -> Filters {
        Filters {
            no_draft: true,
            unreviewed: true,
            since: Since::W1,
            repo: Some("api".to_string()),
            labels: vec!["bug".to_string()],
            ..Default::default()
        }
    }

    #[test]
    fn mine_sets_me_and_unticks_review_requested() {
        let mut f = Filters {
            author: AuthorFilter::Is("octocat".to_string()),
            review_requested: true,
            ..the_rest()
        };
        Preset::Mine.apply(&mut f);
        assert_eq!(
            f,
            Filters {
                author: AuthorFilter::me(),
                ..the_rest()
            }
        );
        assert!(Preset::Mine.is_active(&f));
    }

    #[test]
    fn mine_is_not_active_with_review_requested() {
        let f = Filters {
            author: AuthorFilter::me(),
            review_requested: true,
            ..Default::default()
        };
        assert!(!Preset::Mine.is_active(&f));
    }

    #[test]
    fn toggle_mine_twice_returns_to_any_and_keeps_the_rest() {
        let mut f = the_rest();
        f.toggle_mine();
        assert_eq!(
            f,
            Filters {
                author: AuthorFilter::me(),
                ..the_rest()
            }
        );
        f.toggle_mine();
        assert_eq!(f, the_rest());
    }

    #[test]
    fn toggle_mine_from_not_me_turns_it_on() {
        let mut f = Filters {
            author: AuthorFilter::not_me(),
            ..Default::default()
        };
        f.toggle_mine();
        assert_eq!(f.author, AuthorFilter::me());
    }

    // --- saved file ---

    /// Reads a saved file the way `Filters::load` does.
    fn from_file(json: &str) -> Filters {
        serde_json::from_str(json).expect("a readable filters file")
    }

    /// The user's real file at the time of the change, verbatim.
    #[test]
    fn the_current_file_keeps_every_other_filter() {
        let f = from_file(
            r#"{"filter":"All","no_draft":true,"unreviewed":true,"not_mine":false,
                "since":"W1","repo":"service-clm","author":null,"labels":[]}"#,
        );
        assert_eq!(
            f,
            Filters {
                no_draft: true,
                unreviewed: true,
                since: Since::W1,
                repo: Some("service-clm".to_string()),
                ..Default::default()
            }
        );
    }

    #[test]
    fn each_legacy_author_field_alone() {
        let author = |json| from_file(json).author;
        assert_eq!(
            author(r#"{"filter":"All","author":"octocat"}"#),
            AuthorFilter::Is("octocat".to_string())
        );
        assert_eq!(author(r#"{"filter":"Me"}"#), AuthorFilter::me());
        assert_eq!(
            author(r#"{"filter":"All","not_mine":true}"#),
            AuthorFilter::not_me()
        );
        assert_eq!(author(r#"{"filter":"All"}"#), AuthorFilter::Any);
    }

    #[test]
    fn legacy_review_asked_becomes_the_checkbox() {
        let f = from_file(r#"{"filter":"ReviewAsked"}"#);
        assert!(f.review_requested);
        assert_eq!(f.author, AuthorFilter::Any);
    }

    /// Same precedence as the old `to_gh_args`: a typed author already won
    /// over `me`, and `me` + `not_mine` was an empty list anyway.
    #[test]
    fn legacy_precedence_is_typed_author_then_me_then_not_mine() {
        assert_eq!(
            from_file(r#"{"filter":"Me","not_mine":true,"author":"octocat"}"#).author,
            AuthorFilter::Is("octocat".to_string())
        );
        assert_eq!(
            from_file(r#"{"filter":"Me","not_mine":true}"#).author,
            AuthorFilter::me()
        );
    }

    #[test]
    fn legacy_review_asked_combines_with_a_typed_author() {
        let f = from_file(r#"{"filter":"ReviewAsked","author":"octocat"}"#);
        assert_eq!(f.author, AuthorFilter::Is("octocat".to_string()));
        assert!(f.review_requested);
    }

    #[test]
    fn a_legacy_author_goes_through_the_prompt_rules() {
        assert_eq!(
            from_file(r#"{"filter":"All","author":"@octocat"}"#).author,
            AuthorFilter::Is("octocat".to_string())
        );
        assert_eq!(
            from_file(r#"{"filter":"All","author":"-octocat"}"#).author,
            AuthorFilter::IsNot("octocat".to_string())
        );
    }

    #[test]
    fn an_empty_legacy_author_falls_back_to_the_mode() {
        assert_eq!(
            from_file(r#"{"filter":"Me","author":""}"#).author,
            AuthorFilter::me()
        );
    }

    /// `filter` is the format discriminant: when it is there, the new keys
    /// are ignored, even if a hand edit left one behind.
    #[test]
    fn the_legacy_filter_key_decides_the_format() {
        let f =
            from_file(r#"{"filter":"ReviewAsked","review_requested":false,"author":"octocat"}"#);
        assert!(f.review_requested);
        assert_eq!(f.author, AuthorFilter::Is("octocat".to_string()));
    }

    /// `Any` is written as the string `"Any"`: it must come back as `Any`,
    /// not as a legacy login named "Any".
    #[test]
    fn the_new_format_round_trips_even_any() {
        for author in [
            AuthorFilter::Any,
            AuthorFilter::me(),
            AuthorFilter::not_me(),
            AuthorFilter::Is("octocat".to_string()),
        ] {
            let original = Filters {
                author,
                review_requested: true,
                ..the_rest()
            };
            let json = serde_json::to_string(&original).unwrap();
            assert_eq!(from_file(&json), original, "{json}");
        }
    }

    #[test]
    fn the_written_file_has_no_legacy_key() {
        let json = serde_json::to_string(&Filters::default()).unwrap();
        assert!(!json.contains("\"filter\""), "{json}");
        assert!(!json.contains("not_mine"), "{json}");
    }

    /// `load` falls back to the defaults when the file does not parse.
    #[test]
    fn a_malformed_file_gives_the_defaults() {
        let f: Filters = serde_json::from_str(r#"{"no_draft":"yes"}"#).unwrap_or_default();
        assert_eq!(f, Filters::default());
    }

    #[test]
    fn the_author_cycle_goes_any_me_not_me_and_back() {
        assert_eq!(AuthorFilter::Any.next(), AuthorFilter::me());
        assert_eq!(AuthorFilter::me().next(), AuthorFilter::not_me());
        assert_eq!(AuthorFilter::not_me().next(), AuthorFilter::Any);

        assert_eq!(AuthorFilter::Any.prev(), AuthorFilter::not_me());
        assert_eq!(AuthorFilter::not_me().prev(), AuthorFilter::me());
        assert_eq!(AuthorFilter::me().prev(), AuthorFilter::Any);
    }

    /// A typed login sits at the end of the cycle; leaving it forgets it.
    #[test]
    fn a_typed_login_sits_at_the_end_of_the_cycle() {
        for typed in [
            AuthorFilter::Is("octocat".to_string()),
            AuthorFilter::IsNot("octocat".to_string()),
        ] {
            assert_eq!(typed.next(), AuthorFilter::Any, "{typed:?}");
            assert_eq!(typed.prev(), AuthorFilter::not_me(), "{typed:?}");
        }
    }
}
