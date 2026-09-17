//! Everything that talks to the outside world: finding the git repos, and
//! running `gh` to fetch the PRs.

use crate::filters::Filters;
use crate::model::Pr;
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Maximum number of PRs fetched per repo (like the script's `--limit`).
const PR_LIMIT: &str = "50";

/// JSON fields requested from `gh` for each PR.
const JSON_FIELDS: &str =
    "number,title,author,reviewDecision,isDraft,url,updatedAt,additions,deletions,labels";

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
