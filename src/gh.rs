//! Everything that talks to the outside world: finding the git repos, and
//! running `gh` (and `git`) to fetch the PRs, runs and repos, or to clone.

use crate::filters::Filters;
use crate::model::{Pr, Run};
use crate::repos::{LocalRepo, Repo, RepoFilters, parse_origin};
use anyhow::{Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

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

/// Most repos `gh repo list` returns for one owner: far above any org this
/// is meant for. `gh` pages through them itself.
const REPO_LIMIT: &str = "1000";

/// JSON fields requested from `gh` for each repo.
const REPO_JSON_FIELDS: &str =
    "nameWithOwner,name,description,visibility,isArchived,isFork,pushedAt,url";

/// Most orgs `gh org list` returns (its default is 30).
const ORG_LIMIT: &str = "100";

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

/// The arguments of `gh repo list` for `owner` under `filters`. Split out
/// so the flags are testable without running `gh`.
pub fn repo_list_args(owner: &str, filters: &RepoFilters) -> Vec<String> {
    let mut args: Vec<String> = [
        "repo",
        "list",
        owner,
        "--limit",
        REPO_LIMIT,
        "--json",
        REPO_JSON_FIELDS,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(filters.to_gh_args());
    args
}

/// Runs `gh repo list` for `owner` and parses the JSON into `Vec<Repo>`.
pub fn fetch_repos(owner: &str, filters: &RepoFilters) -> Result<Vec<Repo>> {
    let output = Command::new("gh")
        .args(repo_list_args(owner, filters))
        .output()
        .context("launching `gh` (is it installed and in the PATH?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("`gh repo list` failed: {}", stderr.trim());
    }

    serde_json::from_slice(&output.stdout).context("parsing the JSON returned by `gh repo list`")
}

/// The orgs of the authenticated account, for the owner picker.
pub fn fetch_orgs() -> Result<Vec<String>> {
    let output = Command::new("gh")
        .args(["org", "list", "--limit", ORG_LIMIT])
        .output()
        .context("launching `gh` (is it installed and in the PATH?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("`gh org list` failed: {}", stderr.trim());
    }
    Ok(parse_org_list(&String::from_utf8_lossy(&output.stdout)))
}

/// `gh org list` prints one login per line when its output is not a
/// terminal — the case here, since we capture it.
fn parse_org_list(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// The scanned folder's repos with the `owner/name` of their `origin`
/// remote, asked of `git` itself: local, instant, no network. A folder
/// whose origin is missing or not on GitHub gets `None`.
pub fn local_origins(root: &Path) -> Vec<LocalRepo> {
    discover_repos(root)
        .unwrap_or_default()
        .into_iter()
        .map(|folder| {
            let origin = Command::new("git")
                .arg("-C")
                .arg(root.join(&folder))
                .args(["remote", "get-url", "origin"])
                .output()
                .ok()
                .filter(|output| output.status.success())
                .and_then(|output| parse_origin(&String::from_utf8_lossy(&output.stdout)));
            LocalRepo { folder, origin }
        })
        .collect()
}

/// The `gh repo clone` command for one repo, into `root/name`. Built apart
/// from `clone_repo` so its safety settings are testable. The TUI owns the
/// terminal, so a prompt there would be invisible and wait forever: stdin is
/// closed and git's credential prompt disabled (https), and ssh — which
/// opens `/dev/tty` itself for a passphrase or an unknown host — is made to
/// ask an askpass that always fails, so it gives up instead. Unlike
/// overriding `GIT_SSH_COMMAND`, this leaves the user's ssh config alone.
fn clone_command(root: &Path, name_with_owner: &str, name: &str) -> Command {
    let mut cmd = Command::new("gh");
    cmd.args(["repo", "clone", name_with_owner])
        .arg(root.join(name))
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("SSH_ASKPASS", "/usr/bin/false")
        .stdin(Stdio::null());
    cmd
}

/// Clones `name_with_owner` into `root/name`. On failure, the error is the
/// last line `gh` printed: the one that says why.
pub fn clone_repo(root: &Path, name_with_owner: &str, name: &str) -> Result<()> {
    let output = clone_command(root, name_with_owner, name)
        .output()
        .context("launching `gh` (is it installed and in the PATH?)")?;
    if !output.status.success() {
        anyhow::bail!("{}", last_line(&String::from_utf8_lossy(&output.stderr)));
    }
    Ok(())
}

/// The last non-blank line of `text`. For a failed clone the first one is
/// git's `Cloning into '…'...`, which says nothing.
fn last_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .unwrap_or("unknown error")
        .to_string()
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

    #[test]
    fn repo_list_asks_for_the_owner_with_the_boxes_flags() {
        let args = repo_list_args("acme", &RepoFilters::default());
        assert_eq!(
            args,
            [
                "repo",
                "list",
                "acme",
                "--limit",
                REPO_LIMIT,
                "--json",
                REPO_JSON_FIELDS,
                "--no-archived",
                "--source",
            ]
        );
    }

    #[test]
    fn org_list_output_is_one_login_per_line() {
        assert_eq!(parse_org_list("acme\n  corp \n\n"), ["acme", "corp"]);
        assert!(parse_org_list("").is_empty());
    }

    /// Review focus 3: git prints `Cloning into '…'...` first, which says
    /// nothing about the failure; the reason comes last.
    #[test]
    fn a_failed_clone_reports_its_last_line() {
        let stderr = "Cloning into 'api'...\nfatal: repository not found\n\n";
        assert_eq!(last_line(stderr), "fatal: repository not found");
        assert_eq!(last_line("  \n"), "unknown error");
    }

    /// Review focus 1: the TUI owns the terminal, so a credential prompt
    /// would be invisible and wait forever. It must fail instead.
    #[test]
    fn the_clone_command_targets_the_folder_and_never_prompts() {
        let cmd = clone_command(Path::new("/ws"), "acme/api", "api");

        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args, ["repo", "clone", "acme/api", "/ws/api"]);
        assert!(
            cmd.get_envs()
                .any(|(key, value)| key == "GIT_TERMINAL_PROMPT"
                    && value == Some(std::ffi::OsStr::new("0"))),
            "git must not prompt for credentials"
        );
        // Final review I4: ssh opens /dev/tty itself (passphrase, unknown
        // host), past GIT_TERMINAL_PROMPT; routing every prompt to an askpass
        // that fails makes it give up instead of blocking the TUI.
        let env = |name: &str| {
            cmd.get_envs()
                .find(|(key, _)| *key == name)
                .and_then(|(_, value)| value)
                .map(|value| value.to_string_lossy().into_owned())
        };
        assert_eq!(env("SSH_ASKPASS_REQUIRE").as_deref(), Some("force"));
        assert_eq!(env("SSH_ASKPASS").as_deref(), Some("/usr/bin/false"));
    }
}
