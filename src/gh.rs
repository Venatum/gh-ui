//! Everything that talks to the outside world: finding the git repos, and
//! running `gh` to fetch the PRs.

use crate::filters::Filters;
use crate::model::{Pr, Run};
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Maximum number of PRs fetched per repo (like the script's `--limit`).
const PR_LIMIT: &str = "50";

/// JSON fields requested from `gh` for each PR.
const JSON_FIELDS: &str = "number,title,author,reviewDecision,isDraft,url,updatedAt,additions,deletions,labels,headRefName";

/// Number of runs fetched per repo (most recent runs, all branches).
/// 100 is the largest page GitHub serves, so it costs exactly the same single
/// HTTP request as a smaller number would.
const RUN_LIMIT: &str = "100";

/// How many of those runs the Actions tab shows per repo when the "my PRs"
/// filter is off. Deliberately smaller than `RUN_LIMIT`: see `fetch_runs`.
pub const RUN_DISPLAY_LIMIT: usize = 20;

/// JSON fields requested from `gh` for each run.
const RUN_JSON_FIELDS: &str =
    "workflowName,displayTitle,headBranch,status,conclusion,event,createdAt,number,url";

/// Lists the subdirectories of `root` that are git repos (contain `.git`).
/// Returns their names, sorted, as `list-prs.sh` does with `for dir in */`.
pub fn discover_repos(root: &Path) -> Result<Vec<String>> {
    let mut repos = Vec::new();

    // `read_dir` can fail (nonexistent dir, no permissions...) → `?`.
    for entry in std::fs::read_dir(root).context("reading the root directory")? {
        let entry = entry?; // each entry is itself a Result
        let path = entry.path();

        // We keep only the directories that contain a `.git`.
        if path.is_dir() && path.join(".git").exists() {
            // `file_name()` returns an Option (no name for "..") → we filter.
            if let Some(name) = entry.file_name().to_str() {
                repos.push(name.to_string());
            }
        }
    }

    repos.sort();
    Ok(repos)
}

/// Runs `gh pr list` (with the filters) in `repo_dir` and parses the JSON.
pub fn fetch_prs(repo_dir: &Path, filters: &Filters) -> Result<Vec<Pr>> {
    // The fields we request from gh, exactly as in the script.
    // We assemble the arguments into a Vec so we can add the filter ones
    // (variable count). The `.to_string()` calls unify the type.
    let mut args: Vec<String> = vec![
        "pr".to_string(),
        "list".to_string(),
        "--state".to_string(),
        "open".to_string(),
        "--limit".to_string(),
        PR_LIMIT.to_string(),
        "--json".to_string(),
        JSON_FIELDS.to_string(),
    ];
    args.extend(filters.to_gh_args());

    // Runs the command with the repo's directory as the current directory.
    let output = Command::new("gh")
        .args(&args)
        .current_dir(repo_dir)
        .output()
        .context("launching `gh` (is it installed and in the PATH?)")?;

    // `output.status` = return code. If it failed, we surface stderr.
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("`gh` failed: {}", stderr.trim());
    }

    // `output.stdout` is a Vec<u8> (raw bytes). serde_json can parse it directly
    // into a list of Pr thanks to the `Deserialize` derived on `Pr`.
    let prs: Vec<Pr> =
        serde_json::from_slice(&output.stdout).context("parsing the JSON returned by gh")?;

    Ok(prs)
}

/// Runs `gh run list` in `repo_dir` and parses the JSON into `Vec<Run>`.
/// No filters here: we fetch the N most recent raw runs; the cross-referencing
/// with the PRs is done on the App side (see `App::visible_runs`).
///
/// N is deliberately much larger than what the tab displays. That
/// cross-reference keeps only the runs sitting on a branch that carries a PR,
/// and a base branch such as `main` or `develop` can easily produce the whole
/// window on its own — one merge fires every workflow at once — leaving the
/// tab empty for no visible reason. Widening the window is free: 100 is the
/// largest page the API serves, so it is the same single HTTP request as 20,
/// only with a bigger body. The alternative, one `gh run list --branch X` per
/// open PR, would be exact but would cost one request per PR per repo on every
/// auto-refresh.
pub fn fetch_runs(repo_dir: &Path) -> Result<Vec<Run>> {
    let output = Command::new("gh")
        .args([
            "run",
            "list",
            "--limit",
            RUN_LIMIT,
            "--json",
            RUN_JSON_FIELDS,
        ])
        .current_dir(repo_dir)
        .output()
        .context("launching `gh` (is it installed and in the PATH?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("`gh run list` failed: {}", stderr.trim());
    }

    let runs: Vec<Run> = serde_json::from_slice(&output.stdout)
        .context("parsing the JSON returned by `gh run list`")?;
    Ok(runs)
}

/// The login of the authenticated user, shown in the header. Asked of `gh`
/// itself rather than of the REST endpoint (`gh api user`): the whole app
/// talks to `gh`, not to the API. `--active` picks the account currently in
/// use — a user can be logged into several on the same host — and the `.login`
/// under `hosts` is the one it resolves to.
pub fn fetch_login() -> Result<String> {
    let output = Command::new("gh")
        .args([
            "auth",
            "status",
            "--active",
            "--json",
            "hosts",
            "--jq",
            ".hosts[][] | select(.active) | .login",
        ])
        .output()
        .context("launching `gh` (is it installed and in the PATH?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("`gh auth status` failed: {}", stderr.trim());
    }

    // Several accounts can be active across hosts; the first line is ours.
    let login = String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    if login.is_empty() {
        anyhow::bail!("`gh auth status` returned no active account");
    }
    Ok(login)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fetch window and the displayed slice are two different numbers, and
    /// the gap between them is the whole point: `App::visible_runs` drops the
    /// runs whose branch carries no PR, so a window as narrow as the slice can
    /// be swallowed whole by a busy base branch and leave the tab empty.
    #[test]
    fn the_run_fetch_window_is_wider_than_the_displayed_slice() {
        let limit: usize = RUN_LIMIT.parse().expect("RUN_LIMIT must be a number");
        assert!(
            limit > RUN_DISPLAY_LIMIT,
            "RUN_LIMIT={limit} leaves no room for the PR cross-reference"
        );
    }

    /// GitHub serves at most 100 items per page: asking for more makes `gh`
    /// paginate, which doubles the HTTP cost of every repo on every refresh.
    #[test]
    fn the_run_fetch_stays_within_one_api_page() {
        let limit: usize = RUN_LIMIT.parse().expect("RUN_LIMIT must be a number");
        assert!(
            limit <= 100,
            "RUN_LIMIT={limit} would force `gh` to paginate"
        );
    }
}
