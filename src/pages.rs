//! The PRs tab's infinite scroll. `gh pr list` has no cursor: the only way
//! to see the PRs past the first page is to ask for more of them, from the
//! first one again. So each repo keeps its own limit, which grows by a page
//! each time the cursor reaches the end of what the repo sent, and which
//! every reload asks for again: a refresh must not shrink the list under
//! the cursor.

use crate::filters::Filters;
use crate::gh::PR_PAGE;
use crate::model::Pr;
use crate::search;
use std::collections::{HashMap, HashSet};

/// How many PRs each repo is asked for, and which ones have more.
#[derive(Default)]
pub struct PrPages {
    /// The filters the limits were reached with, the repo filter aside.
    query: Option<Filters>,
    /// The repos asked for more than one page; absent = `PR_PAGE`.
    limits: HashMap<String, usize>,
    /// The repos whose last answer filled their limit: they probably have
    /// more open PRs than they sent.
    full: HashSet<String>,
    /// The repo whose next page is on its way.
    pub loading: Option<String>,
    /// A next page asked for while another load was in flight.
    pub pending: Option<String>,
}

impl PrPages {
    /// Starts every repo over at one page when the filters changed: the
    /// limits were reached for another query. The repo filter does not
    /// count, it picks which repos are shown, not what they are asked.
    pub fn sync(&mut self, filters: &Filters) {
        let query = Filters {
            repo: None,
            ..filters.clone()
        };
        if self.query.as_ref() != Some(&query) {
            self.limits.clear();
            self.query = Some(query);
        }
    }

    /// How many PRs `repo` is asked for.
    pub fn limit(&self, repo: &str) -> usize {
        self.limits.get(repo).copied().unwrap_or(PR_PAGE)
    }

    /// Every limit past the first page, for a reload to ask again.
    pub fn limits(&self) -> HashMap<String, usize> {
        self.limits.clone()
    }

    /// Does `repo` probably have more open PRs than it sent?
    pub fn has_more(&self, repo: &str) -> bool {
        self.full.contains(repo)
    }

    /// A whole load landed: `full` replaces what was known.
    pub fn set_full(&mut self, full: Vec<String>) {
        self.full = full.into_iter().collect();
    }

    /// `repo` sent `count` PRs for a limit of `limit`: a full page means
    /// there are probably more.
    pub fn record(&mut self, repo: &str, count: usize, limit: usize) {
        if count >= limit {
            self.full.insert(repo.to_string());
        } else {
            self.full.remove(repo);
        }
    }

    /// Grows `repo`'s limit by a page and returns it.
    pub fn next_page(&mut self, repo: &str) -> usize {
        let limit = self.limit(repo) + PR_PAGE;
        self.limits.insert(repo.to_string(), limit);
        limit
    }

    /// Takes back the page `next_page` added, when it failed to load: the
    /// next attempt asks for the same one.
    pub fn drop_page(&mut self, repo: &str) {
        let limit = self.limit(repo).saturating_sub(PR_PAGE);
        if limit > PR_PAGE {
            self.limits.insert(repo.to_string(), limit);
        } else {
            self.limits.remove(repo);
        }
    }
}

/// A row of the PRs table.
#[derive(Debug, Clone, Copy)]
pub enum PrRow<'a> {
    Pr(&'a Pr),
    /// The end of a repo that has more PRs than it sent: the cursor landing
    /// here loads the next page.
    More(&'a str),
}

/// The PRs table: `prs` (grouped by repo, as loaded) narrowed by `query`,
/// each repo with more closed by a `More` row. The row stays during a
/// search, even under a repo with no match: the PRs it stands for were never
/// searched.
pub fn pr_rows<'a>(prs: &'a [Pr], query: &str, pages: &PrPages) -> Vec<PrRow<'a>> {
    let mut rows = Vec::new();
    for block in prs.chunk_by(|a, b| a.repo == b.repo) {
        rows.extend(search::keep_prs(block, query).into_iter().map(PrRow::Pr));
        let repo = block[0].repo.as_str();
        if pages.has_more(repo) {
            rows.push(PrRow::More(repo));
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filters::AuthorFilter;
    use crate::model::sample_pr;

    fn numbers(rows: &[PrRow]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                PrRow::Pr(pr) => format!("{}#{}", pr.repo, pr.number),
                PrRow::More(repo) => format!("{repo} more"),
            })
            .collect()
    }

    #[test]
    fn a_full_repo_ends_with_a_more_row() {
        let prs = vec![
            sample_pr("api", 1),
            sample_pr("api", 2),
            sample_pr("web", 3),
        ];
        let mut pages = PrPages::default();
        pages.set_full(vec!["api".to_string()]);

        let rows = pr_rows(&prs, "", &pages);

        assert_eq!(numbers(&rows), ["api#1", "api#2", "api more", "web#3"]);
    }

    #[test]
    fn the_more_row_stays_under_a_search_that_hides_its_repo() {
        let mut prs = vec![sample_pr("api", 1), sample_pr("web", 2)];
        prs[1].title = "fix the login".to_string();
        let mut pages = PrPages::default();
        pages.set_full(vec!["api".to_string()]);

        let rows = pr_rows(&prs, "login", &pages);

        assert_eq!(numbers(&rows), ["api more", "web#2"]);
    }

    #[test]
    fn a_page_grows_the_limit_and_a_failed_one_gives_it_back() {
        let mut pages = PrPages::default();
        assert_eq!(pages.limit("api"), PR_PAGE);

        assert_eq!(pages.next_page("api"), 2 * PR_PAGE);
        assert_eq!(pages.next_page("api"), 3 * PR_PAGE);
        pages.drop_page("api");
        assert_eq!(pages.limit("api"), 2 * PR_PAGE);
        pages.drop_page("api");
        assert_eq!(pages.limit("api"), PR_PAGE);
        assert!(pages.limits().is_empty(), "the first page is the default");
    }

    #[test]
    fn a_short_answer_means_the_repo_has_no_more() {
        let mut pages = PrPages::default();
        pages.record("api", PR_PAGE, PR_PAGE);
        assert!(pages.has_more("api"));

        // Exactly fifty open PRs: the second page brings nothing new.
        pages.record("api", PR_PAGE, 2 * PR_PAGE);
        assert!(!pages.has_more("api"));
    }

    #[test]
    fn new_filters_start_every_repo_over_but_the_repo_filter_does_not() {
        let mut pages = PrPages::default();
        let mut filters = Filters::default();
        pages.sync(&filters);
        pages.next_page("api");

        filters.repo = Some("api".to_string());
        pages.sync(&filters);
        assert_eq!(pages.limit("api"), 2 * PR_PAGE);

        filters.author = AuthorFilter::me();
        pages.sync(&filters);
        assert_eq!(pages.limit("api"), PR_PAGE);
    }
}
