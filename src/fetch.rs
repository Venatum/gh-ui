//! The background loading: a thread does the slow work (`gh` calls) and returns
//! the result over a channel, so the UI never freezes.

use crate::filters::Filters;
use crate::gh;
use crate::model::Pr;
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

/// Starts the load in a background thread.
/// `root` and `filters` are cloned then MOVED into the thread (`move`).
pub fn spawn(root: PathBuf, filters: Filters, tx: Sender<FetchResult>) {
    thread::spawn(move || {
        let result = load(&root, &filters);
        let _ = tx.send(result);
    });
}

/// The slow work: discover the repos, filter, fetch the PRs.
/// The repos are queried IN PARALLEL (one thread each), so the total duration
/// ≈ that of the slowest repo, instead of their sum.
fn load(root: &Path, filters: &Filters) -> FetchResult {
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
