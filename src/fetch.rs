//! The background loading: a thread does the slow work (`gh` calls) and returns
//! the result over a channel, so the UI never freezes.

use crate::filters::Filters;
use crate::gh;
use crate::issues::{Issue, IssueFilters};
use crate::model::{Pr, Run};
use crate::repos::{CloneEvent, LocalRepo, Repo, RepoFilters};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread;

/// What the thread returns once the load is finished.
pub struct FetchResult {
    pub prs: Vec<Pr>,
    /// ALL the discovered repos (not just the scanned ones) → used for cycling.
    pub all_repos: Vec<String>,
    /// Number of repos actually queried (depends on the repo filter).
    pub scanned: usize,
    pub errors: usize,
}

/// Mirror of `FetchResult`, but for the runs.
pub struct RunsResult {
    pub runs: Vec<Run>,
    pub all_repos: Vec<String>,
    pub scanned: usize,
    pub errors: usize,
}

/// Mirror of `FetchResult`, for the issues.
pub struct IssuesResult {
    pub issues: Vec<Issue>,
    pub all_repos: Vec<String>,
    pub scanned: usize,
    pub errors: usize,
}

/// A repo-list load: for whom, what `gh` answered, and the folder's repos
/// read in the same pass (the row states are derived from both).
pub struct ReposResult {
    pub owner: String,
    /// `Err` carries `gh`'s message for the status line.
    pub repos: Result<Vec<Repo>, String>,
    pub locals: Vec<LocalRepo>,
}

/// What the background thread returns: PRs, runs, or both at once.
pub enum Loaded {
    Prs(FetchResult),
    Runs(RunsResult),
    Both(FetchResult, RunsResult),
    /// The authenticated account, or `None` if `gh` could not tell us.
    /// Not a load: it rides the same channel, but carries no status line.
    User(Option<String>),
    /// The owner's repos (Repos tab). Not a `Job`: the list has its own
    /// loading flag in `App`, so it never merges with the PR and run flows.
    Repos(ReposResult),
    /// The account's orgs, for the owner picker; empty when `gh` failed.
    Orgs(Vec<String>),
    /// One step of a clone batch.
    Clone(CloneEvent),
    /// The folder's open issues (Issues tab). Not a `Job`: the issues have
    /// their own loading flag, like the repo list.
    Issues(IssuesResult),
}

/// What we ask the thread to load.
/// `Copy` so the app can stash a job aside while a load is in flight.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Job {
    Prs,
    Runs,
    Both,
}

impl Job {
    /// The single job that covers `self` AND `other`. Used when a refresh is
    /// requested while another one is still running: instead of dropping one of
    /// them, we widen the pending job.
    pub fn merge(self, other: Job) -> Job {
        if self == other { self } else { Job::Both }
    }
}

/// What `fan_out` needs from a row: somewhere to write the folder it came
/// from. `gh` does not return it — it only knows the repo it ran in — so the
/// loader stamps every row itself.
pub trait FromRepo {
    fn set_repo(&mut self, repo: &str);
}

impl FromRepo for Pr {
    fn set_repo(&mut self, repo: &str) {
        self.repo = repo.to_string();
    }
}

impl FromRepo for Run {
    fn set_repo(&mut self, repo: &str) {
        self.repo = repo.to_string();
    }
}

impl FromRepo for Issue {
    fn set_repo(&mut self, repo: &str) {
        self.repo = repo.to_string();
    }
}

/// The repos a load queries: all of them, or only the selected one.
fn repos_to_scan<'a>(all: &'a [String], selected: Option<&str>) -> Vec<&'a String> {
    match selected {
        Some(sel) => all.iter().filter(|r| r.as_str() == sel).collect(),
        None => all.iter().collect(),
    }
}

/// Runs `fetch` in every repo of `repos` AT ONCE (one scoped thread each), so
/// the total wait is that of the slowest repo rather than their sum. Returns
/// the rows in the repos' order, each stamped with its repo, plus how many
/// repos failed (`gh` error or panicked thread): one bad repo never hides the
/// others.
///
/// Generic over the row type `T`, so the PRs, the runs and the issues share
/// this one loop. `F: Sync` because every thread borrows the same `fetch`;
/// `T: Send` because each thread hands its rows back to this one.
fn fan_out<T, F>(root: &Path, repos: &[&String], fetch: F) -> (Vec<T>, usize)
where
    T: FromRepo + Send,
    F: Fn(&Path) -> anyhow::Result<Vec<T>> + Sync,
{
    // A reference is `Copy`: each `move` closure below takes its own copy of
    // it instead of trying to move `fetch` itself into several threads.
    let fetch = &fetch;
    let joined = thread::scope(|scope| {
        let handles: Vec<_> = repos
            .iter()
            .map(|&repo| scope.spawn(move || (repo, fetch(&root.join(repo)))))
            .collect();
        // `join()` waits for each thread; the handles' order = the repos'
        // order, so the result stays deterministic (rows grouped by repo).
        handles.into_iter().map(|h| h.join()).collect::<Vec<_>>()
    });

    let mut rows = Vec::new();
    let mut errors = 0;
    for result in joined {
        match result {
            Ok((repo, Ok(mut list))) => {
                for row in &mut list {
                    row.set_repo(repo);
                }
                rows.extend(list);
            }
            // `gh` failed for this repo, or the thread panicked.
            Ok((_, Err(_))) | Err(_) => errors += 1,
        }
    }
    (rows, errors)
}

/// Resolves the authenticated user in its own one-shot thread. It rides the
/// loads' channel so the app keeps a single place to poll, and it is never
/// re-run: the account cannot change while gh-ui is open.
pub fn spawn_login(tx: Sender<Loaded>) {
    thread::spawn(move || {
        // A failure here is not worth an error banner — the header shows `@?`
        // and the status line already reports whatever broke the PR load.
        let _ = tx.send(Loaded::User(gh::fetch_login().ok()));
    });
}

/// Asks for the account's orgs, once, like `spawn_login`.
pub fn spawn_orgs(tx: Sender<Loaded>) {
    thread::spawn(move || {
        // A failure only shrinks the picker to the account itself.
        let _ = tx.send(Loaded::Orgs(gh::fetch_orgs().unwrap_or_default()));
    });
}

/// Loads `owner`'s repos, and reads the folder's origins alongside.
pub fn spawn_repos(root: PathBuf, owner: String, filters: RepoFilters, tx: Sender<Loaded>) {
    thread::spawn(move || {
        let repos = gh::fetch_repos(&owner, &filters).map_err(|e| e.to_string());
        let locals = gh::local_origins(&root);
        let _ = tx.send(Loaded::Repos(ReposResult {
            owner,
            repos,
            locals,
        }));
    });
}

/// Loads the issues of every repo (or of the selected one) in the background.
pub fn spawn_issues(root: PathBuf, filters: IssueFilters, tx: Sender<Loaded>) {
    thread::spawn(move || {
        let _ = tx.send(Loaded::Issues(load_issues(&root, &filters)));
    });
}

/// Clones `batch` — (`owner/name`, folder) pairs — one repo after the
/// other, reporting each step. Sequential on purpose: a batch is rare, and
/// one clone at a time keeps every error attributable.
pub fn spawn_clones(root: PathBuf, batch: Vec<(String, String)>, tx: Sender<Loaded>) {
    thread::spawn(move || {
        for (name_with_owner, name) in batch {
            let _ = tx.send(Loaded::Clone(CloneEvent::Started(name_with_owner.clone())));
            let event = match gh::clone_repo(&root, &name_with_owner, &name) {
                Ok(()) => CloneEvent::Done(name_with_owner),
                Err(e) => CloneEvent::Failed(name_with_owner, e.to_string()),
            };
            let _ = tx.send(Loaded::Clone(event));
        }
        let _ = tx.send(Loaded::Clone(CloneEvent::Finished));
    });
}

/// Starts loading `job` in a background thread.
/// `root` and `filters` are cloned then MOVED into the thread (`move`).
pub fn spawn(job: Job, root: PathBuf, filters: Filters, tx: Sender<Loaded>) {
    thread::spawn(move || {
        let result = match job {
            Job::Prs => Loaded::Prs(load_prs(&root, &filters)),
            Job::Runs => Loaded::Runs(load_runs(&root, &filters)),
            // Both flows at once, side by side rather than one after the other:
            // the total wait stays that of the slowest one.
            Job::Both => {
                let (prs, runs) = thread::scope(|scope| {
                    let h = scope.spawn(|| load_prs(&root, &filters));
                    let runs = load_runs(&root, &filters);
                    (h.join(), runs)
                });
                // The child thread only calls `gh`; if it panicked we would have
                // nothing to show, so an empty result is the honest fallback.
                let prs = prs.unwrap_or(FetchResult {
                    prs: Vec::new(),
                    all_repos: runs.all_repos.clone(),
                    scanned: 0,
                    errors: 1,
                });
                Loaded::Both(prs, runs)
            }
        };
        let _ = tx.send(result);
    });
}

/// The slow work: discover the repos, filter, fetch the PRs.
/// The repos are queried IN PARALLEL (one thread each), so the total duration
/// ≈ that of the slowest repo, instead of their sum.
fn load_prs(root: &Path, filters: &Filters) -> FetchResult {
    let all_repos = gh::discover_repos(root).unwrap_or_default();
    let to_scan = repos_to_scan(&all_repos, filters.repo.as_deref());
    let (prs, errors) = fan_out(root, &to_scan, |dir| gh::fetch_prs(dir, filters));
    FetchResult {
        prs,
        scanned: to_scan.len(),
        errors,
        all_repos,
    }
}

/// Like `load_prs`, but for the runs. The repo filter (if set)
/// also restricts the repos scanned here, for consistency with the PRs tab.
fn load_runs(root: &Path, filters: &Filters) -> RunsResult {
    let all_repos = gh::discover_repos(root).unwrap_or_default();
    let to_scan = repos_to_scan(&all_repos, filters.repo.as_deref());
    let (runs, errors) = fan_out(root, &to_scan, gh::fetch_runs);
    RunsResult {
        runs,
        scanned: to_scan.len(),
        errors,
        all_repos,
    }
}

/// Like `load_prs`, for the issues, with the Issues tab's own repo filter.
fn load_issues(root: &Path, filters: &IssueFilters) -> IssuesResult {
    let all_repos = gh::discover_repos(root).unwrap_or_default();
    let to_scan = repos_to_scan(&all_repos, filters.repo.as_deref());
    let (issues, errors) = fan_out(root, &to_scan, |dir| gh::fetch_issues(dir, filters));
    IssuesResult {
        issues,
        scanned: to_scan.len(),
        errors,
        all_repos,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row type of our own: the fan-out must not care what it carries.
    #[derive(Debug)]
    struct Row {
        name: String,
        repo: String,
    }

    impl FromRepo for Row {
        fn set_repo(&mut self, repo: &str) {
            self.repo = repo.to_string();
        }
    }

    #[test]
    fn fan_out_keeps_the_repo_order_stamps_each_row_and_counts_failures() {
        let all = ["api".to_string(), "broken".to_string(), "web".to_string()];
        let repos: Vec<&String> = all.iter().collect();
        let (rows, errors) = fan_out(Path::new("/root"), &repos, |dir| {
            let name = dir.file_name().unwrap().to_string_lossy().to_string();
            if name == "broken" {
                anyhow::bail!("issues are disabled");
            }
            Ok(vec![
                Row {
                    name: format!("{name}#1"),
                    repo: String::new(),
                },
                Row {
                    name: format!("{name}#2"),
                    repo: String::new(),
                },
            ])
        });

        // One bad repo is counted, and hides nothing of the others.
        assert_eq!(errors, 1);
        let got: Vec<(&str, &str)> = rows
            .iter()
            .map(|r| (r.name.as_str(), r.repo.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                ("api#1", "api"),
                ("api#2", "api"),
                ("web#1", "web"),
                ("web#2", "web")
            ]
        );
    }

    #[test]
    fn a_selected_repo_narrows_the_scan_to_itself() {
        let all = ["api".to_string(), "web".to_string()];
        assert_eq!(repos_to_scan(&all, None), [&all[0], &all[1]]);
        assert_eq!(repos_to_scan(&all, Some("web")), [&all[1]]);
        assert!(repos_to_scan(&all, Some("ghost")).is_empty());
    }
}
