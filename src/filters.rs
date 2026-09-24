//! The filters: their state, how we evolve them from the keyboard, how we
//! translate them into `gh` arguments, and how we save/reload them.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The main mode, equivalent to the script's `--filter`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum FilterMode {
    #[default]
    All,
    Me,
    ReviewAsked,
}

impl FilterMode {
    pub fn label(self) -> &'static str {
        match self {
            FilterMode::All => "all",
            FilterMode::Me => "me",
            FilterMode::ReviewAsked => "review-asked",
        }
    }
    fn next(self) -> Self {
        match self {
            FilterMode::All => FilterMode::Me,
            FilterMode::Me => FilterMode::ReviewAsked,
            FilterMode::ReviewAsked => FilterMode::All,
        }
    }
    fn prev(self) -> Self {
        match self {
            FilterMode::All => FilterMode::ReviewAsked,
            FilterMode::Me => FilterMode::All,
            FilterMode::ReviewAsked => FilterMode::Me,
        }
    }
}

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
    fn next(self) -> Self {
        match self {
            Since::Off => Since::D1,
            Since::D1 => Since::D3,
            Since::D3 => Since::W1,
            Since::W1 => Since::W2,
            Since::W2 => Since::M1,
            Since::M1 => Since::Off,
        }
    }
    fn prev(self) -> Self {
        match self {
            Since::Off => Since::M1,
            Since::D1 => Since::Off,
            Since::D3 => Since::D1,
            Since::W1 => Since::D3,
            Since::W2 => Since::W1,
            Since::M1 => Since::W2,
        }
    }
}

/// The set of active filters. `Default` gives the "everything, nothing checked" state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Filters {
    pub filter: FilterMode,
    pub no_draft: bool,
    pub unreviewed: bool,
    pub not_mine: bool,
    pub since: Since,
    /// `None` = all repos; `Some(name)` = a single one.
    pub repo: Option<String>,
    /// Author filter (typed on the keyboard). `None` = no filter.
    pub author: Option<String>,
    /// Filter by labels (logical AND). Empty = no filter.
    #[serde(default)]
    pub labels: Vec<String>,
}

impl Filters {
    // --- keyboard-driven mutations ---
    pub fn cycle_filter(&mut self) {
        self.filter = self.filter.next();
    }
    pub fn cycle_filter_back(&mut self) {
        self.filter = self.filter.prev();
    }
    pub fn toggle_no_draft(&mut self) {
        self.no_draft = !self.no_draft;
    }
    pub fn toggle_unreviewed(&mut self) {
        self.unreviewed = !self.unreviewed;
    }
    pub fn toggle_not_mine(&mut self) {
        self.not_mine = !self.not_mine;
    }
    /// The `m` shortcut: "my PRs" on, or back to all. Turning it on also drops
    /// what would defeat it — an explicit author wins over `me` in
    /// `to_gh_args`, and `not-mine` would leave an always-empty list. Turning
    /// it off only touches the mode.
    pub fn toggle_mine(&mut self) {
        if self.filter == FilterMode::Me {
            self.filter = FilterMode::All;
        } else {
            self.filter = FilterMode::Me;
            self.author = None;
            self.not_mine = false;
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
        if repos.is_empty() {
            self.repo = None;
            return;
        }
        self.repo = match &self.repo {
            None => Some(repos[0].clone()),
            Some(current) => match repos.iter().position(|r| r == current) {
                // not the last → the next one
                Some(i) if i + 1 < repos.len() => Some(repos[i + 1].clone()),
                // last (or not found) → back to "all"
                _ => None,
            },
        };
    }

    /// Same cycle, backwards: all → last → … → repo0 → all.
    pub fn cycle_repo_prev(&mut self, repos: &[String]) {
        if repos.is_empty() {
            self.repo = None;
            return;
        }
        self.repo = match &self.repo {
            None => Some(repos[repos.len() - 1].clone()),
            Some(current) => match repos.iter().position(|r| r == current) {
                Some(0) | None => None, // first (or not found) → "all"
                Some(i) => Some(repos[i - 1].clone()),
            },
        };
    }

    /// Drops a repo selection that this folder does not hold, and returns the
    /// name we let go. The config is global while `repo` names a folder, so a
    /// selection made in `~/dev/perso` would filter EVERYTHING out once gh-ui
    /// is pointed at `~/dev/client` — with nothing on screen saying why.
    ///
    /// An empty `repos` is left alone on purpose: it means either an empty
    /// folder or a `read_dir` that failed, and there is nothing to show in
    /// either case. Nor do we save afterwards: the selection stays valid in
    /// the folder it was made for, and each run repairs itself in memory.
    pub fn reconcile_repo(&mut self, repos: &[String]) -> Option<String> {
        if repos.is_empty() {
            return None;
        }
        // `take_if` hands us the value only when the closure says so, leaving
        // `self.repo` as `None` in that case — exactly the fallback we want.
        self.repo.take_if(|sel| !repos.contains(sel))
    }

    /// The part of the summary that ALSO applies to the Actions tab.
    /// Today only the repo filter restricts both flows.
    pub fn summary_common(&self) -> String {
        match &self.repo {
            Some(r) => format!("repo:{r}"),
            None => "repo:all".to_string(),
        }
    }

    /// The part that only goes to `gh pr list`: "filter:me · since:1w · no-draft".
    pub fn summary_prs(&self) -> String {
        let mut parts = vec![format!("filter:{}", self.filter.label())];
        if self.since != Since::Off {
            parts.push(format!("since:{}", self.since.label()));
        }
        if self.no_draft {
            parts.push("no-draft".to_string());
        }
        if self.unreviewed {
            parts.push("unreviewed".to_string());
        }
        if self.not_mine {
            parts.push("not-mine".to_string());
        }
        if let Some(a) = &self.author {
            parts.push(format!("author:{a}"));
        }
        if !self.labels.is_empty() {
            parts.push(format!("labels:{}", self.labels.join(",")));
        }
        parts.join(" · ")
    }

    /// Stores the author filter (empty string → no filter).
    pub fn set_author(&mut self, value: &str) {
        let value = value.trim();
        self.author = if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        };
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

        // Author: an explicit `--author` wins over the `me` mode (like the
        // script). `or_else` computes the second case only if the first is None.
        let author = self.author.clone().or_else(|| {
            if self.filter == FilterMode::Me {
                Some("@me".to_string())
            } else {
                None
            }
        });
        if let Some(a) = author {
            args.push("--author".to_string());
            args.push(a);
        }
        if self.filter == FilterMode::ReviewAsked {
            search.push("review-requested:@me".to_string());
        }

        // Labels: one `--label` per label (gh combines them with AND).
        for label in &self.labels {
            args.push("--label".to_string());
            args.push(label.clone());
        }

        if let Some(days) = self.since.days() {
            // Cutoff date = today - N days. `chrono` handles the calendar.
            let cutoff = chrono::Local::now().date_naive() - chrono::Duration::days(days);
            // A NaiveDate already displays in YYYY-MM-DD format.
            search.push(format!("updated:>={cutoff}"));
        }

        if self.no_draft {
            search.push("draft:false".to_string());
        }
        if self.unreviewed {
            search.push("-reviewed-by:@me".to_string());
        }
        if self.not_mine {
            search.push("-author:@me".to_string());
        }

        if !search.is_empty() {
            args.push("--search".to_string());
            args.push(search.join(" "));
        }
        args
    }

    // --- persistence ---

    /// Path of the config file: ~/.config/gh-ui/filters.json (Linux/macOS).
    fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("gh-ui").join("filters.json"))
    }

    /// Reloads the filters, or returns the default values if missing/unreadable.
    pub fn load() -> Self {
        // `let ... else`: if the config can't be found, we bail out returning
        // the defaults. Otherwise we continue with `path`.
        let Some(path) = Self::config_path() else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Saves the filters as JSON (silent on failure).
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

// `#[cfg(test)]`: this module is compiled ONLY for `cargo test`.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_splits_common_from_pr_only_filters() {
        let f = Filters {
            repo: Some("api".to_string()),
            no_draft: true,
            author: Some("moi".to_string()),
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
            not_mine: true,
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
    fn explicit_author_wins_over_the_me_mode() {
        let f = Filters {
            filter: FilterMode::Me,
            author: Some("octocat".to_string()),
            ..Default::default()
        };
        let args = f.to_gh_args();
        // --author only once, with the explicit author (not @me)
        let authors: Vec<&String> = args
            .iter()
            .zip(args.iter().skip(1))
            .filter(|(k, _)| *k == "--author")
            .map(|(_, v)| v)
            .collect();
        assert_eq!(authors, vec!["octocat"]);
    }

    #[test]
    fn toggle_mine_goes_to_me_and_back_to_all() {
        let mut f = Filters::default();
        f.toggle_mine();
        assert_eq!(f.filter, FilterMode::Me);
        f.toggle_mine();
        assert_eq!(f.filter, FilterMode::All);
    }

    #[test]
    fn toggle_mine_from_review_asked_goes_to_me() {
        let mut f = Filters {
            filter: FilterMode::ReviewAsked,
            ..Default::default()
        };
        f.toggle_mine();
        assert_eq!(f.filter, FilterMode::Me);
    }

    /// An explicit author would override `me`, and `not-mine` would cancel it
    /// out: left alone, either one makes the key look broken.
    #[test]
    fn toggle_mine_on_clears_what_contradicts_it() {
        let mut f = Filters {
            author: Some("octocat".to_string()),
            not_mine: true,
            ..Default::default()
        };
        f.toggle_mine();
        assert_eq!(f.author, None);
        assert!(!f.not_mine);
        assert_eq!(f.to_gh_args(), vec!["--author", "@me"]);
    }

    #[test]
    fn toggle_mine_off_only_touches_the_mode() {
        let mut f = Filters {
            filter: FilterMode::Me,
            no_draft: true,
            since: Since::W1,
            labels: vec!["bug".to_string()],
            ..Default::default()
        };
        f.toggle_mine();
        assert_eq!(
            f,
            Filters {
                filter: FilterMode::All,
                no_draft: true,
                since: Since::W1,
                labels: vec!["bug".to_string()],
                ..Default::default()
            }
        );
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
    fn set_author_trims_and_empty_gives_none() {
        let mut f = Filters::default();
        f.set_author("  octocat  ");
        assert_eq!(f.author.as_deref(), Some("octocat")); // spaces stripped
        f.set_author("   ");
        assert_eq!(f.author, None); // empty input → no filter
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
            filter: FilterMode::ReviewAsked,
            no_draft: true,
            since: Since::W1,
            repo: Some("hello-world".to_string()),
            author: Some("octocat".to_string()),
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
}
