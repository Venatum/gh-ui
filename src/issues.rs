//! The Issues tab's logic: an issue as `gh issue list` returns it, the tab's
//! own filters, and the linked-PR label. Pure: `gh.rs` runs the command,
//! `fetch.rs` carries the result, this module decides what it means — so
//! every rule here is unit-tested without running `gh`.

#![allow(dead_code)]

use crate::config;
use crate::filters::{self, AuthorFilter, ME, Since};
use crate::model::{Author, Label};
use serde::{Deserialize, Serialize};

/// An issue, as `gh issue list --json ...` returns it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub author: Author,
    /// Same `{ "login": … }` shape as the author.
    #[serde(default)]
    pub assignees: Vec<Author>,
    #[serde(default)]
    pub labels: Vec<Label>,
    pub created_at: String,
    pub updated_at: String,
    pub url: String,
    /// The PRs linked to the issue: GitHub's "Development" section, or a
    /// `closes #N` in the PR. A PR that merely mentions it is not here.
    /// Renamed: `rename_all` would look for `linkedPrs`.
    #[serde(default, rename = "closedByPullRequestsReferences")]
    pub linked_prs: Vec<PrRef>,
    /// Not in the JSON: the folder the issue comes from, filled in by the
    /// loader, as `Pr::repo`.
    #[serde(skip)]
    pub repo: String,
}

/// A PR linked to an issue: its number and where it lives, which may be
/// another repo than the issue's.
#[derive(Debug, Deserialize)]
pub struct PrRef {
    pub number: u64,
    pub repository: RepoRef,
}

#[derive(Debug, Deserialize)]
pub struct RepoRef {
    pub name: String,
    pub owner: Author,
}

/// The PRs column's text: blank without a linked PR, `#123` for one in the
/// issue's own repo, `owner/name#123` for one elsewhere, `3 PRs` beyond one.
/// "Own repo" compares the PR repo's name with the issue's folder name,
/// ignoring case as GitHub does.
pub fn linked_prs_label(issue: &Issue) -> String {
    // Slice patterns: match on how many elements there are, and bind them.
    match issue.linked_prs.as_slice() {
        [] => String::new(),
        [pr] if pr.repository.name.eq_ignore_ascii_case(&issue.repo) => {
            format!("#{}", pr.number)
        }
        [pr] => format!(
            "{}/{}#{}",
            pr.repository.owner.login, pr.repository.name, pr.number
        ),
        many => format!("{} PRs", many.len()),
    }
}

/// Whose issues: anyone's, mine, or nobody's (the triage view).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum AssigneeFilter {
    #[default]
    Any,
    Me,
    Nobody,
}

impl AssigneeFilter {
    /// The label shown in the header and in the filter panel.
    pub fn label(self) -> &'static str {
        match self {
            AssigneeFilter::Any => "any",
            AssigneeFilter::Me => ME,
            AssigneeFilter::Nobody => "nobody",
        }
    }

    pub fn next(self) -> Self {
        match self {
            AssigneeFilter::Any => AssigneeFilter::Me,
            AssigneeFilter::Me => AssigneeFilter::Nobody,
            AssigneeFilter::Nobody => AssigneeFilter::Any,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            AssigneeFilter::Any => AssigneeFilter::Nobody,
            AssigneeFilter::Me => AssigneeFilter::Any,
            AssigneeFilter::Nobody => AssigneeFilter::Me,
        }
    }
}

/// The Issues tab's filters: their own set, saved in `issuefilters.json`,
/// so `author:@me` on the PRs tab does not force the same on the issues.
/// `#[serde(default)]`: a field missing from the file takes its default
/// instead of failing the whole file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct IssueFilters {
    pub author: AuthorFilter,
    pub assignee: AssigneeFilter,
    /// Logical AND. Empty = no filter.
    pub labels: Vec<String>,
    pub since: Since,
    /// `None` = all repos; `Some(name)` = a single folder.
    pub repo: Option<String>,
}

impl IssueFilters {
    /// The `gh issue list` arguments (except the repo, which chooses WHICH
    /// folders to scan). The search qualifiers go into one `--search`.
    pub fn to_gh_args(&self) -> Vec<String> {
        let mut args: Vec<String> = Vec::new();
        let mut search: Vec<String> = Vec::new();

        self.author.push_gh_args(&mut args, &mut search);
        match self.assignee {
            AssigneeFilter::Any => {}
            AssigneeFilter::Me => {
                args.push("--assignee".to_string());
                args.push(ME.to_string());
            }
            // No flag for "nobody": only the search has it.
            AssigneeFilter::Nobody => search.push("no:assignee".to_string()),
        }
        for label in &self.labels {
            args.push("--label".to_string());
            args.push(label.clone());
        }
        if let Some(updated) = self.since.updated_qualifier() {
            search.push(updated);
        }

        if !search.is_empty() {
            args.push("--search".to_string());
            args.push(search.join(" "));
        }
        args
    }

    /// The header's line, read like a query: `repo:all · author:any ·
    /// assignee:@me · since:1w · labels:bug,ui`.
    pub fn summary(&self) -> String {
        let mut parts = vec![
            format!("repo:{}", self.repo.as_deref().unwrap_or("all")),
            format!("author:{}", self.author.summary()),
            format!("assignee:{}", self.assignee.label()),
        ];
        if self.since != Since::Off {
            parts.push(format!("since:{}", self.since.label()));
        }
        if !self.labels.is_empty() {
            parts.push(format!("labels:{}", self.labels.join(",")));
        }
        parts.join(" · ")
    }

    // --- keyboard-driven mutations; `forward` = the ←/→ direction ---

    pub fn cycle_repo(&mut self, repos: &[String], forward: bool) {
        let current = self.repo.as_deref();
        self.repo = if forward {
            filters::next_repo(current, repos)
        } else {
            filters::prev_repo(current, repos)
        };
    }

    pub fn cycle_author(&mut self, forward: bool) {
        self.author = if forward {
            self.author.next()
        } else {
            self.author.prev()
        };
    }

    pub fn cycle_assignee(&mut self, forward: bool) {
        self.assignee = if forward {
            self.assignee.next()
        } else {
            self.assignee.prev()
        };
    }

    pub fn cycle_since(&mut self, forward: bool) {
        self.since = if forward {
            self.since.next()
        } else {
            self.since.prev()
        };
    }

    /// The `m` shortcut: my issues, or everyone's again. Touches nothing else.
    pub fn toggle_mine(&mut self) {
        self.assignee = if self.assignee == AssigneeFilter::Me {
            AssigneeFilter::Any
        } else {
            AssigneeFilter::Me
        };
    }

    /// Stores the author typed in the prompt (see `AuthorFilter::parse`).
    pub fn set_author(&mut self, text: &str) {
        self.author = AuthorFilter::parse(text);
    }

    /// Stores the labels from the prompt, space-separated as on the PRs tab.
    pub fn set_labels(&mut self, text: &str) {
        self.labels = text.split_whitespace().map(str::to_string).collect();
    }

    /// See `filters::reconcile_repo_selection`.
    pub fn reconcile_repo(&mut self, repos: &[String]) -> Option<String> {
        filters::reconcile_repo_selection(&mut self.repo, repos)
    }

    // --- persistence ---

    /// Parses the file's contents; a malformed file gives the defaults.
    /// Split out of `load` so it is testable without the filesystem.
    fn from_json(content: &str) -> Self {
        serde_json::from_str(content).unwrap_or_default()
    }

    /// Reloads the filters, or the defaults if the file is missing/unreadable.
    pub fn load() -> Self {
        let Some(path) = config::path("issuefilters.json") else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(content) => Self::from_json(&content),
            Err(_) => Self::default(),
        }
    }

    /// Saves the filters as JSON (silent on failure).
    pub fn save(&self) {
        let Some(path) = config::path("issuefilters.json") else {
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

/// The Issues tab's state, as `ReposTab` holds the Repos tab's: `app.rs`
/// only wires it.
#[derive(Debug, Default)]
pub struct IssuesTab {
    /// Every repo's open issues, newest update first.
    pub issues: Vec<Issue>,
    pub filters: IssueFilters,
    /// Loaded at least once: the auto-refresh keeps it fresh from then on.
    pub loaded: bool,
    /// A load is in flight. Separate from `App::loading`: the issues are
    /// not a `Job`, they never merge with the PR and run flows.
    pub loading: bool,
    /// A reload asked for while one was in flight.
    pub pending: bool,
    /// The line of the last load, shown whenever the tab is on screen.
    pub status: String,
    /// A note for the next load's line — today, a repo filter we dropped.
    pub notice: Option<String>,
}

impl IssuesTab {
    /// Stores a load, newest update first across every repo. ISO 8601
    /// timestamps sort as text, so no date parsing is needed.
    pub fn apply_load(&mut self, mut issues: Vec<Issue>) {
        issues.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        self.issues = issues;
        self.loaded = true;
        self.loading = false;
    }
}

/// A minimal issue for the tests of every module.
#[cfg(test)]
pub fn sample_issue(repo: &str, number: u64, updated_at: &str) -> Issue {
    Issue {
        number,
        title: format!("issue {number}"),
        author: Author {
            login: "alice".to_string(),
        },
        assignees: Vec::new(),
        labels: Vec::new(),
        created_at: "2026-09-01T00:00:00Z".to_string(),
        updated_at: updated_at.to_string(),
        url: format!("https://github.com/acme/{repo}/issues/{number}"),
        linked_prs: Vec::new(),
        repo: repo.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pr_ref(owner: &str, name: &str, number: u64) -> PrRef {
        PrRef {
            number,
            repository: RepoRef {
                name: name.to_string(),
                owner: Author {
                    login: owner.to_string(),
                },
            },
        }
    }

    /// The `--search` value of a command line, if any.
    fn search_of(args: &[String]) -> Option<&str> {
        let i = args.iter().position(|a| a == "--search")?;
        args.get(i + 1).map(String::as_str)
    }

    #[test]
    fn an_issue_parses_from_gh_json() {
        let json = r#"{
            "number": 7, "title": "crash on start", "author": {"login": "alice"},
            "assignees": [{"login": "bob"}], "labels": [{"name": "bug"}],
            "createdAt": "2026-09-01T00:00:00Z", "updatedAt": "2026-09-20T10:00:00Z",
            "url": "https://github.com/acme/api/issues/7",
            "closedByPullRequestsReferences": [{
                "id": "PR_x", "number": 12, "url": "u",
                "repository": {"id": "R_x", "name": "api", "owner": {"id": "O_x", "login": "acme"}}
            }]
        }"#;
        let issue: Issue = serde_json::from_str(json).unwrap();
        assert_eq!(issue.number, 7);
        assert_eq!(issue.assignees[0].login, "bob");
        assert_eq!(issue.labels[0].name, "bug");
        assert_eq!(issue.linked_prs[0].number, 12);
        assert_eq!(issue.linked_prs[0].repository.owner.login, "acme");
        assert_eq!(issue.repo, "", "filled in by the loader, not the JSON");
    }

    #[test]
    fn an_issue_without_assignees_labels_or_prs_still_parses() {
        let json = r#"{
            "number": 1, "title": "t", "author": {"login": "a"},
            "createdAt": "c", "updatedAt": "u", "url": "l"
        }"#;
        let issue: Issue = serde_json::from_str(json).unwrap();
        assert!(issue.assignees.is_empty());
        assert!(issue.linked_prs.is_empty());
    }

    #[test]
    fn the_prs_column_reads_as_blank_a_number_a_full_ref_or_a_count() {
        let mut issue = sample_issue("api", 7, "2026-09-20T00:00:00Z");
        assert_eq!(linked_prs_label(&issue), "");

        issue.linked_prs = vec![pr_ref("acme", "api", 12)];
        assert_eq!(linked_prs_label(&issue), "#12");

        // GitHub ignores case in repo names: so do we.
        issue.linked_prs = vec![pr_ref("acme", "API", 12)];
        assert_eq!(linked_prs_label(&issue), "#12");

        issue.linked_prs = vec![pr_ref("acme", "web", 3)];
        assert_eq!(linked_prs_label(&issue), "acme/web#3");

        issue.linked_prs = vec![pr_ref("acme", "api", 12), pr_ref("acme", "web", 3)];
        assert_eq!(linked_prs_label(&issue), "2 PRs");
    }

    #[test]
    fn the_assignee_cycles_both_ways() {
        let mut a = AssigneeFilter::Any;
        a = a.next();
        assert_eq!(a, AssigneeFilter::Me);
        a = a.next();
        assert_eq!(a, AssigneeFilter::Nobody);
        assert_eq!(a.next(), AssigneeFilter::Any);
        assert_eq!(AssigneeFilter::Any.prev(), AssigneeFilter::Nobody);
        assert_eq!(AssigneeFilter::Me.prev(), AssigneeFilter::Any);
    }

    #[test]
    fn default_filters_add_no_argument() {
        assert!(IssueFilters::default().to_gh_args().is_empty());
    }

    #[test]
    fn my_issues_use_the_assignee_flag() {
        let f = IssueFilters {
            assignee: AssigneeFilter::Me,
            ..Default::default()
        };
        assert_eq!(f.to_gh_args(), ["--assignee", "@me"]);
    }

    #[test]
    fn unassigned_issues_go_through_the_search() {
        let f = IssueFilters {
            assignee: AssigneeFilter::Nobody,
            ..Default::default()
        };
        assert_eq!(search_of(&f.to_gh_args()), Some("no:assignee"));
    }

    #[test]
    fn an_author_is_a_flag_and_an_excluded_one_a_search_qualifier() {
        let f = IssueFilters {
            author: AuthorFilter::Is("octocat".to_string()),
            ..Default::default()
        };
        assert_eq!(f.to_gh_args(), ["--author", "octocat"]);

        let f = IssueFilters {
            author: AuthorFilter::not_me(),
            ..Default::default()
        };
        assert_eq!(search_of(&f.to_gh_args()), Some("-author:@me"));
    }

    #[test]
    fn labels_and_since_combine_with_the_rest_in_one_search() {
        let f = IssueFilters {
            assignee: AssigneeFilter::Nobody,
            labels: vec!["bug".to_string(), "ui".to_string()],
            since: Since::W1,
            ..Default::default()
        };
        let args = f.to_gh_args();
        assert_eq!(&args[..4], ["--label", "bug", "--label", "ui"]);
        // A single --search holding every qualifier.
        assert_eq!(args.iter().filter(|a| *a == "--search").count(), 1);
        let search = search_of(&args).unwrap();
        assert!(
            search.starts_with("no:assignee updated:>="),
            "got {search:?}"
        );
    }

    #[test]
    fn the_summary_reads_like_a_query() {
        let f = IssueFilters {
            assignee: AssigneeFilter::Me,
            since: Since::W1,
            labels: vec!["bug".to_string(), "ui".to_string()],
            ..Default::default()
        };
        assert_eq!(
            f.summary(),
            "repo:all · author:any · assignee:@me · since:1w · labels:bug,ui"
        );
    }

    #[test]
    fn m_toggles_my_issues_and_nothing_else() {
        let mut f = IssueFilters {
            assignee: AssigneeFilter::Nobody,
            since: Since::W1,
            ..Default::default()
        };
        f.toggle_mine();
        assert_eq!(f.assignee, AssigneeFilter::Me);
        f.toggle_mine();
        assert_eq!(f.assignee, AssigneeFilter::Any);
        assert_eq!(f.since, Since::W1);
    }

    #[test]
    fn the_repo_cycles_through_the_folder_both_ways() {
        let repos = vec!["api".to_string(), "web".to_string()];
        let mut f = IssueFilters::default();
        f.cycle_repo(&repos, true);
        assert_eq!(f.repo.as_deref(), Some("api"));
        f.cycle_repo(&repos, false);
        assert_eq!(f.repo, None);
        f.cycle_repo(&repos, false);
        assert_eq!(f.repo.as_deref(), Some("web"));
    }

    #[test]
    fn a_repo_the_folder_does_not_hold_is_dropped_and_named() {
        let mut f = IssueFilters {
            repo: Some("ghost".to_string()),
            ..Default::default()
        };
        let dropped = f.reconcile_repo(&["api".to_string()]);
        assert_eq!(dropped.as_deref(), Some("ghost"));
        assert_eq!(f.repo, None);
    }

    #[test]
    fn typed_author_and_labels_follow_the_prs_tab_rules() {
        let mut f = IssueFilters::default();
        f.set_author("@octocat");
        assert_eq!(f.author, AuthorFilter::Is("octocat".to_string()));
        f.set_labels("bug  ui");
        assert_eq!(f.labels, ["bug", "ui"]);
    }

    #[test]
    fn the_filters_survive_a_json_round_trip() {
        let f = IssueFilters {
            author: AuthorFilter::not_me(),
            assignee: AssigneeFilter::Nobody,
            labels: vec!["bug".to_string()],
            since: Since::M1,
            repo: Some("api".to_string()),
        };
        let json = serde_json::to_string(&f).unwrap();
        assert_eq!(IssueFilters::from_json(&json), f);
    }

    #[test]
    fn a_malformed_or_partial_file_falls_back_to_the_defaults() {
        assert_eq!(IssueFilters::from_json("not json"), IssueFilters::default());
        let partial = IssueFilters::from_json(r#"{"assignee":"Me"}"#);
        assert_eq!(partial.assignee, AssigneeFilter::Me);
        assert_eq!(partial.since, Since::Off);
    }

    #[test]
    fn a_load_is_sorted_newest_update_first_and_marks_the_tab_loaded() {
        let mut tab = IssuesTab {
            loading: true,
            ..Default::default()
        };
        tab.apply_load(vec![
            sample_issue("api", 1, "2026-09-01T00:00:00Z"),
            sample_issue("web", 2, "2026-09-20T00:00:00Z"),
            sample_issue("api", 3, "2026-09-10T00:00:00Z"),
        ]);
        let numbers: Vec<u64> = tab.issues.iter().map(|i| i.number).collect();
        assert_eq!(numbers, [2, 3, 1]);
        assert!(tab.loaded);
        assert!(!tab.loading);
    }
}
