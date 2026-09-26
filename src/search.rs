//! Free-text search over the rows already fetched. Purely client-side: it
//! narrows what is on screen, it never re-runs `gh`.

use crate::issues::Issue;
use crate::model::{Pr, Run};
use crate::repos::Repo;

/// Does `haystack` contain `query`, ignoring case? A blank query matches
/// everything: emptying the prompt must show the whole list again, not none
/// of it.
pub fn matches(query: &str, haystack: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }
    haystack.to_lowercase().contains(&query.to_lowercase())
}

/// The text a PR is searched on. A FIXED set of fields, deliberately
/// independent of which columns are currently visible: hiding the `author`
/// column must not silently change what a search finds.
pub fn pr_haystack(pr: &Pr) -> String {
    let labels = pr
        .labels
        .iter()
        .map(|l| l.name.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{} #{} {} {} {} {}",
        pr.repo, pr.number, pr.title, pr.author.login, pr.head_ref_name, labels
    )
}

/// The PRs matching `query`, in the order they were fetched. Borrowed: the
/// table only reads them, and `App` keeps owning the full list so a cleared
/// search brings every row straight back.
pub fn keep_prs<'a>(prs: &'a [Pr], query: &str) -> Vec<&'a Pr> {
    prs.iter()
        .filter(|pr| matches(query, &pr_haystack(pr)))
        .collect()
}

/// The text a run is searched on. Same rule as `pr_haystack`: a fixed set of
/// fields, whatever the columns panel currently shows.
pub fn run_haystack(run: &Run) -> String {
    format!(
        "{} #{} {} {} {} {}",
        run.repo, run.number, run.workflow_name, run.head_branch, run.display_title, run.event
    )
}

/// The runs matching `query`. Takes the view the Actions tab has already built
/// (its "only my PRs' branches" toggle runs first), so the two filters
/// compose instead of competing.
pub fn keep_runs<'a>(runs: Vec<&'a Run>, query: &str) -> Vec<&'a Run> {
    runs.into_iter()
        .filter(|run| matches(query, &run_haystack(run)))
        .collect()
}

/// The text a repo is searched on: `owner/name` and the description.
pub fn repo_haystack(repo: &Repo) -> String {
    format!(
        "{} {}",
        repo.name_with_owner,
        repo.description.as_deref().unwrap_or("")
    )
}

/// The repos matching `query`. Takes the list the Repos tab has already
/// built (its `hide cloned` box runs first), like `keep_runs`.
pub fn keep_repos<'a>(repos: Vec<&'a Repo>, query: &str) -> Vec<&'a Repo> {
    repos
        .into_iter()
        .filter(|repo| matches(query, &repo_haystack(repo)))
        .collect()
}

/// The text an issue is searched on — a fixed set of fields, whatever the
/// columns panel shows, as for the PRs.
#[allow(dead_code)]
pub fn issue_haystack(issue: &Issue) -> String {
    let assignees = issue
        .assignees
        .iter()
        .map(|a| a.login.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let labels = issue
        .labels
        .iter()
        .map(|l| l.name.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{} #{} {} {} {} {}",
        issue.repo, issue.number, issue.title, issue.author.login, assignees, labels
    )
}

/// The issues matching `query`, in the order they were loaded.
#[allow(dead_code)]
pub fn keep_issues<'a>(issues: &'a [Issue], query: &str) -> Vec<&'a Issue> {
    issues
        .iter()
        .filter(|issue| matches(query, &issue_haystack(issue)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::issues::sample_issue;
    use crate::model::{Author, Label, Pr, Run};

    /// A PR with the fields the search looks at, and plausible filler for the
    /// rest. Written out rather than parsed from JSON so each test can name
    /// exactly what it is matching on.
    fn pr(number: u64, title: &str, author: &str, branch: &str, repo: &str) -> Pr {
        Pr {
            number,
            title: title.to_string(),
            author: Author {
                login: author.to_string(),
            },
            review_decision: String::new(),
            is_draft: false,
            url: format!("https://example.test/{number}"),
            updated_at: "2026-09-19T10:00:00Z".to_string(),
            additions: 0,
            deletions: 0,
            labels: vec![Label {
                name: "bug".to_string(),
            }],
            head_ref_name: branch.to_string(),
            repo: repo.to_string(),
        }
    }

    fn run(number: u64, workflow: &str, branch: &str, title: &str, repo: &str) -> Run {
        Run {
            workflow_name: workflow.to_string(),
            display_title: title.to_string(),
            head_branch: branch.to_string(),
            status: "completed".to_string(),
            conclusion: "success".to_string(),
            event: "push".to_string(),
            created_at: "2026-09-19T10:00:00Z".to_string(),
            number,
            url: format!("https://example.test/run/{number}"),
            repo: repo.to_string(),
        }
    }

    #[test]
    fn a_blank_query_matches_everything() {
        // "no search" and "search for nothing" must be the same state: the
        // prompt is pre-filled and can be emptied back to nothing.
        assert!(matches("", "whatever"));
        assert!(matches("   ", "whatever"));
    }

    #[test]
    fn the_match_ignores_case_and_surrounding_spaces() {
        assert!(matches("API", "fix(api): guard the payload"));
        assert!(matches("  api  ", "fix(API): guard the payload"));
        assert!(!matches("api", "chore: bump ratatui"));
    }

    #[test]
    fn the_haystack_covers_every_field_the_user_may_remember() {
        let hay = pr_haystack(&pr(
            412,
            "guard the payload",
            "vincent",
            "feature/ISSUE-1",
            "hello-world",
        ));

        // Repo folder name: GitHub itself knows nothing about it, so this can
        // only ever be matched client-side.
        assert!(matches("hello-world", &hay), "got {hay}");
        assert!(matches("guard", &hay), "got {hay}");
        assert!(matches("vincent", &hay), "got {hay}");
        // The branch is not even a visible column on the PRs tab.
        assert!(matches("feature/issue-1", &hay), "got {hay}");
        // Both spellings of the number.
        assert!(matches("#412", &hay), "got {hay}");
        assert!(matches("412", &hay), "got {hay}");
        // Labels ride along for free.
        assert!(matches("bug", &hay), "got {hay}");
    }

    #[test]
    fn keep_prs_narrows_without_reordering() {
        let prs = vec![
            pr(1, "fix the parser", "ana", "fix/parser", "api"),
            pr(2, "docs", "bo", "docs", "web"),
            pr(3, "fix the header", "ana", "fix/header", "api"),
        ];

        let kept = keep_prs(&prs, "fix");
        assert_eq!(
            kept.iter().map(|p| p.number).collect::<Vec<_>>(),
            vec![1, 3]
        );

        // A blank query is the identity, not a wipe.
        assert_eq!(keep_prs(&prs, "").len(), 3);
        // No match is an empty view, not a panic.
        assert!(keep_prs(&prs, "zzz").is_empty());
    }

    #[test]
    fn the_run_haystack_covers_workflow_branch_title_and_event() {
        let hay = run_haystack(&run(
            7,
            "CI",
            "feature/ISSUE-1",
            "guard the payload",
            "hello-world",
        ));

        assert!(matches("ci", &hay), "got {hay}");
        assert!(matches("feature/issue-1", &hay), "got {hay}");
        assert!(matches("guard", &hay), "got {hay}");
        assert!(matches("push", &hay), "got {hay}");
        assert!(matches("hello-world", &hay), "got {hay}");
        assert!(matches("#7", &hay), "got {hay}");
    }

    #[test]
    fn keep_runs_narrows_the_already_filtered_view() {
        let runs = [
            run(1, "CI", "feature/a", "first", "api"),
            run(2, "release", "main", "second", "api"),
        ];
        // It takes the branch-filtered view the Actions tab already built,
        // hence a Vec of references rather than a slice of values.
        let view: Vec<&Run> = runs.iter().collect();

        assert_eq!(keep_runs(view.clone(), "release").len(), 1);
        assert_eq!(keep_runs(view.clone(), "").len(), 2);
        assert!(keep_runs(view, "zzz").is_empty());
    }

    #[test]
    fn a_repo_is_found_by_owner_name_or_description() {
        let mut api = crate::repos::sample_repo("acme/api");
        api.description = Some("The billing backend".to_string());
        let web = crate::repos::sample_repo("corp/web");
        let all = vec![&api, &web];

        assert_eq!(keep_repos(all.clone(), "acme").len(), 1);
        assert_eq!(keep_repos(all.clone(), "BILLING").len(), 1);
        assert_eq!(keep_repos(all.clone(), "corp/web").len(), 1);
        assert_eq!(keep_repos(all, "").len(), 2);
    }

    #[test]
    fn an_issue_is_found_by_number_title_author_assignee_or_label() {
        let mut issue = sample_issue("api", 7, "2026-09-20T00:00:00Z");
        issue.title = "Crash on start".to_string();
        issue.assignees = vec![Author {
            login: "bob".to_string(),
        }];
        issue.labels = vec![Label {
            name: "bug".to_string(),
        }];
        let issues = [issue, sample_issue("web", 8, "2026-09-19T00:00:00Z")];

        for query in ["#7", "crash", "alice", "bob", "bug", "api"] {
            let found: Vec<u64> = keep_issues(&issues, query)
                .iter()
                .map(|i| i.number)
                .collect();
            assert!(
                found.contains(&7),
                "{query:?} should find #7, got {found:?}"
            );
        }
        assert_eq!(keep_issues(&issues, "bob").len(), 1);
        assert_eq!(
            keep_issues(&issues, "  ").len(),
            2,
            "a blank query keeps all"
        );
    }
}
