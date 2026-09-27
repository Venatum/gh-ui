//! Everything that talks to the outside world: finding the git repos, and
//! running `gh` (and `git`) to fetch the PRs, runs and repos, or to clone.

use crate::filters::Filters;
use crate::issues::{Issue, IssueFilters};
use crate::model::{Pr, Run};
use crate::repos::{LocalRepo, Repo, RepoFilters, parse_origin};
use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use std::path::Path;
use std::process::{Command, Output, Stdio};

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

/// Runs `gh <args>` — in `dir` when given, where `gh` reads which repo it
/// is — and parses what it prints into `T`, whatever list that is. Generic
/// over `S` so both `["run", "list"]` and a built `Vec<String>` fit.
fn run_gh_json<T, S>(args: &[S], dir: Option<&Path>) -> Result<T>
where
    T: DeserializeOwned,
    S: AsRef<str>,
{
    let args: Vec<&str> = args.iter().map(AsRef::as_ref).collect();
    let mut cmd = Command::new("gh");
    cmd.args(&args);
    if let Some(dir) = dir {
        cmd.current_dir(dir);
    }
    let output = cmd
        .output()
        .context("launching `gh` (is it installed and in the PATH?)")?;
    // The first two words name the command: `gh pr list`, `gh repo list`…
    let what = format!("gh {}", args[..args.len().min(2)].join(" "));
    parse_gh_output(&what, &output)
}

/// What `gh` answered, as `T`: its JSON when it succeeded, otherwise its
/// own message. Apart from `run_gh_json` so a test can feed it an `Output`
/// without running `gh`.
fn parse_gh_output<T: DeserializeOwned>(what: &str, output: &Output) -> Result<T> {
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("`{what}` failed: {}", stderr.trim());
    }
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("parsing the JSON returned by `{what}`"))
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
    // In the repo's folder: `gh` reads which repo it is from there.
    run_gh_json(&args, Some(repo_dir))
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
    let args = [
        "run",
        "list",
        "--limit",
        RUN_LIMIT,
        "--json",
        RUN_JSON_FIELDS,
    ];
    run_gh_json(&args, Some(repo_dir))
}

/// Maximum number of open issues fetched per repo, as `PR_LIMIT` for PRs.
const ISSUE_LIMIT: &str = "50";

/// JSON fields requested from `gh` for each issue. Deliberately without
/// `comments`: `gh` has no comment count, and that field brings every
/// comment's body — about forty times the payload, three times the wait.
const ISSUE_JSON_FIELDS: &str =
    "number,title,author,assignees,labels,createdAt,updatedAt,url,closedByPullRequestsReferences";

/// The `gh issue list` command line for `filters`: open issues only, the
/// filter arguments last. Split out of `fetch_issues` so a test can read it
/// without running `gh`.
fn issue_list_args(filters: &IssueFilters) -> Vec<String> {
    let mut args: Vec<String> = [
        "issue",
        "list",
        "--state",
        "open",
        "--limit",
        ISSUE_LIMIT,
        "--json",
        ISSUE_JSON_FIELDS,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    args.extend(filters.to_gh_args());
    args
}

/// Runs `gh issue list` (with the filters) in `repo_dir` and parses the JSON.
/// A repo with issues disabled makes `gh` fail: the caller counts it as a
/// failed repo, the others still show.
pub fn fetch_issues(repo_dir: &Path, filters: &IssueFilters) -> Result<Vec<Issue>> {
    run_gh_json(&issue_list_args(filters), Some(repo_dir))
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
    run_gh_json(&repo_list_args(owner, filters), None)
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

/// Every entry of the scanned folder, with the `owner/name` of the
/// `origin` remote of the git repos, asked of `git` itself: local, instant,
/// no network. A repo whose origin is missing or not on GitHub gets `None`,
/// and so does anything that is not a repo — it still blocks a clone.
pub fn local_origins(root: &Path) -> Vec<LocalRepo> {
    let repos = discover_repos(root).unwrap_or_default();
    // Two `flatten`s: the folder may be unreadable (a `Result`), then each
    // entry may be (another `Result`); either way it is simply skipped.
    let others: Vec<LocalRepo> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| !repos.contains(name))
        .map(|folder| LocalRepo {
            folder,
            is_git: false,
            origin: None,
        })
        .collect();
    repos
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
            LocalRepo {
                folder,
                is_git: true,
                origin,
            }
        })
        .chain(others)
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
    use crate::issues::{AssigneeFilter, IssueFilters};

    /// What `gh` printed and how it exited, without running it. The raw
    /// status is the `wait()` encoding: the exit code sits in the high byte.
    #[cfg(unix)]
    fn gh_output(code: i32, stdout: &str, stderr: &str) -> std::process::Output {
        use std::os::unix::process::ExitStatusExt;
        std::process::Output {
            status: std::process::ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_successful_gh_gives_its_json() {
        let numbers: Vec<u64> = parse_gh_output("gh pr list", &gh_output(0, "[1, 2]", "")).unwrap();
        assert_eq!(numbers, [1, 2]);
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_gh_says_which_command_and_why() {
        let err =
            parse_gh_output::<Vec<u64>>("gh pr list", &gh_output(1, "", "HTTP 404\n")).unwrap_err();
        assert_eq!(err.to_string(), "`gh pr list` failed: HTTP 404");
    }

    #[cfg(unix)]
    #[test]
    fn unexpected_json_names_the_command_it_came_from() {
        let err = parse_gh_output::<Vec<u64>>("gh run list", &gh_output(0, "{}", "")).unwrap_err();
        assert_eq!(
            err.to_string(),
            "parsing the JSON returned by `gh run list`"
        );
    }

    #[test]
    fn the_local_entries_include_what_is_not_a_repo() {
        let root = std::env::temp_dir().join(format!("gh-ui-locals-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("web").join(".git")).unwrap();
        std::fs::create_dir_all(root.join("api")).unwrap();

        let mut locals = local_origins(&root);
        locals.sort_by(|a, b| a.folder.cmp(&b.folder));
        std::fs::remove_dir_all(&root).ok();

        let seen: Vec<(&str, bool)> = locals
            .iter()
            .map(|l| (l.folder.as_str(), l.is_git))
            .collect();
        assert_eq!(seen, [("api", false), ("web", true)]);
    }

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

    #[test]
    fn issue_list_asks_for_open_issues_then_the_filters() {
        let f = IssueFilters {
            assignee: AssigneeFilter::Me,
            ..Default::default()
        };
        assert_eq!(
            issue_list_args(&f),
            [
                "issue",
                "list",
                "--state",
                "open",
                "--limit",
                "50",
                "--json",
                "number,title,author,assignees,labels,createdAt,updatedAt,url,closedByPullRequestsReferences",
                "--assignee",
                "@me",
            ]
        );
    }

    /// `comments` would bring every comment's body: 177 KB and 2.2 s instead
    /// of 4.6 KB and 0.6 s for 50 issues, only to print a count.
    #[test]
    fn issue_list_never_asks_for_the_comments() {
        assert!(!ISSUE_JSON_FIELDS.split(',').any(|f| f == "comments"));
    }
}
