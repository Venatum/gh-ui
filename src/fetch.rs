//! The background loading: a thread does the slow work (`gh` calls) and returns
//! the result over a channel, so the UI never freezes.

use crate::filters::Filters;
use crate::gh;
use crate::model::{Pr, Run};
use crate::repos::{LocalRepo, Repo, RepoFilters};
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

    // Which repos to scan? All of them, or only the selected one.
    let to_scan: Vec<&String> = match &filters.repo {
        Some(sel) => all_repos.iter().filter(|r| *r == sel).collect(),
        None => all_repos.iter().collect(),
    };

    // `thread::scope`: "scoped" threads that can BORROW `root` and `filters`
    // (the scope guarantees they finish before it ends, so there's no need to
    // clone everything). We spawn one thread per repo, then join them.
    let joined = thread::scope(|scope| {
        let handles: Vec<_> = to_scan
            .iter()
            .map(|&repo| {
                scope.spawn(move || {
                    let dir = root.join(repo);
                    (repo.clone(), gh::fetch_prs(&dir, filters))
                })
            })
            .collect();
        // `join()` waits for each thread; the handles' order = the repos' order,
        // so the result stays deterministic (PRs grouped by repo).
        handles.into_iter().map(|h| h.join()).collect::<Vec<_>>()
    });

    let mut prs = Vec::new();
    let mut errors = 0;
    for result in joined {
        match result {
            Ok((repo, Ok(mut list))) => {
                for pr in &mut list {
                    pr.repo = repo.clone();
                }
                prs.extend(list);
            }
            // `gh` failed for this repo, or the thread panicked.
            Ok((_, Err(_))) | Err(_) => errors += 1,
        }
    }

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

    let to_scan: Vec<&String> = match &filters.repo {
        Some(sel) => all_repos.iter().filter(|r| *r == sel).collect(),
        None => all_repos.iter().collect(),
    };

    let joined = thread::scope(|scope| {
        let handles: Vec<_> = to_scan
            .iter()
            .map(|&repo| scope.spawn(move || (repo.clone(), gh::fetch_runs(&root.join(repo)))))
            .collect();
        handles.into_iter().map(|h| h.join()).collect::<Vec<_>>()
    });

    let mut runs = Vec::new();
    let mut errors = 0;
    for result in joined {
        match result {
            Ok((repo, Ok(mut list))) => {
                for run in &mut list {
                    run.repo = repo.clone();
                }
                runs.extend(list);
            }
            Ok((_, Err(_))) | Err(_) => errors += 1,
        }
    }

    RunsResult {
        runs,
        scanned: to_scan.len(),
        errors,
        all_repos,
    }
}
