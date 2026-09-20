//! Free-text search over the rows already fetched. Purely client-side: it
//! narrows what is on screen, it never re-runs `gh`.

use crate::model::Pr;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Author, Label, Pr};

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
        assert!(matches("feature/api-1", &hay), "got {hay}");
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
}
