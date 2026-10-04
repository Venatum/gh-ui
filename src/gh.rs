//! Everything that talks to the outside world: finding the git repos, and
//! running `gh` (and `git`) to fetch the PRs, runs, issues, releases and
//! repos, or to clone.

use crate::detail::{Detail, DetailKey, IssueDetail, PrDetail, RunDetail};
use crate::filters::Filters;
use crate::issues::{Issue, IssueFilters};
use crate::model::{Pr, Run};
use crate::releases::{self, Release, ReleaseSource, RepoRelease};
use crate::repos::{LocalRepo, Repo, RepoFilters, is_name, parse_origin, parse_remote};
use anyhow::{Context, Result};
use serde::de::DeserializeOwned;
use std::path::Path;
use std::process::{Command, Output, Stdio};

/// Maximum number of PRs fetched per repo (like the script's `--limit`).
const PR_LIMIT: &str = "50";

/// JSON fields requested from `gh` for each PR. A field more never costs a
/// request more: `gh` builds one GraphQL query per repo whatever we ask for.
/// It can cost time, though: `statusCheckRollup` (the full array of checks,
/// every attempt, no way to narrow it) is by far the heaviest. Measured on
/// `cli/cli`, 50 PRs: 0.65 s and 4 KB without it, 3.8 s and 259 KB with it,
/// on every load and auto-refresh. It is what feeds the `State` column's ✗
/// and ● glyphs; a normal repo, with a few PRs and checks, pays far less.
const JSON_FIELDS: &str = "number,title,author,reviewDecision,isDraft,url,updatedAt,additions,deletions,labels,headRefName,mergeable,statusCheckRollup";

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
    "databaseId,workflowName,displayTitle,headBranch,status,conclusion,event,createdAt,number,url";

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

/// JSON fields of `gh pr view` for the detail view. It grows with what
/// `detail.rs` reads: a field nobody reads is payload for nothing.
const PR_VIEW_FIELDS: &str = "title,url,body,state,isDraft,author,baseRefName,headRefName,additions,deletions,changedFiles,reviewDecision,mergeable,labels,reviewRequests,latestReviews,reviews,headRefOid,statusCheckRollup,files,comments";

/// `gh pr view <number> --json …`, run in the PR's folder like `pr list`.
fn pr_view_args(number: u64) -> Vec<String> {
    ["pr", "view", &number.to_string(), "--json", PR_VIEW_FIELDS]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// One PR, everything the detail view shows. ONE GraphQL request whatever
/// the field list (measured with `GH_DEBUG=api`), so the view never makes
/// a second call.
fn fetch_pr_detail(repo_dir: &Path, number: u64) -> Result<PrDetail> {
    run_gh_json(&pr_view_args(number), Some(repo_dir))
}

/// JSON fields requested from `gh issue view`: exactly what
/// `detail::IssueDetail` reads.
const ISSUE_VIEW_FIELDS: &str = "title,url,body,state,stateReason,author,createdAt,labels,assignees,closedByPullRequestsReferences,comments";

/// `gh issue view <number> --json …`, run in the issue's folder.
fn issue_view_args(number: u64) -> Vec<String> {
    [
        "issue",
        "view",
        &number.to_string(),
        "--json",
        ISSUE_VIEW_FIELDS,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// JSON fields requested from `gh run view`: exactly what
/// `detail::RunDetail` reads, its jobs and their steps included.
const RUN_VIEW_FIELDS: &str = "workflowName,displayTitle,number,attempt,status,conclusion,event,headBranch,headSha,url,startedAt,updatedAt,jobs";

fn run_json_view(repo_dir: &Path, id: u64) -> Result<RunDetail> {
    run_gh_json(&run_view_args(id), Some(repo_dir))
}

/// `gh run view <id> --json …`, run in the run's folder.
fn run_view_args(id: u64) -> Vec<String> {
    ["run", "view", &id.to_string(), "--json", RUN_VIEW_FIELDS]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// The detail of the row `key` names, in its folder: one `gh … view`.
pub fn fetch_detail(repo_dir: &Path, key: &DetailKey) -> Result<Detail> {
    Ok(match key {
        DetailKey::Pr { number, .. } => Detail::Pr(fetch_pr_detail(repo_dir, *number)?),
        DetailKey::Issue { repo, number } => {
            let mut issue: IssueDetail = run_gh_json(&issue_view_args(*number), Some(repo_dir))?;
            issue.repo = repo.clone();
            Detail::Issue(issue)
        }
        DetailKey::Run { id, .. } => Detail::Run(run_json_view(repo_dir, *id)?),
        DetailKey::Release { tag, .. } => {
            // The repo the Releases tab listed it from: `origin`'s.
            let source = release_source(repo_dir);
            let repo = source.as_ref().and_then(ReleaseSource::gh_repo);
            let args = release_view_args(RELEASE_DETAIL_FIELDS, repo.as_deref(), Some(tag));
            Detail::Release(run_gh_json(&args, Some(repo_dir))?)
        }
    })
}

/// Releases listed for a repo with no "Latest" (see `latest_release`):
/// room enough to find its newest pre-release.
const RELEASE_LIMIT: &str = "10";

/// JSON fields requested from `gh release list`, which has no `url`.
const RELEASE_JSON_FIELDS: &str = "tagName,name,publishedAt,isLatest,isDraft,isPrerelease";

/// JSON fields requested from `gh release view`: the same, but `url`
/// instead of `isLatest`, which it does not offer.
const RELEASE_VIEW_JSON_FIELDS: &str = "tagName,name,publishedAt,isDraft,isPrerelease,url";

/// JSON fields requested from `gh release view` for the detail view:
/// exactly what `detail::ReleaseDetail` reads, the notes and files
/// included.
const RELEASE_DETAIL_FIELDS: &str =
    "tagName,name,url,body,isDraft,isPrerelease,author,publishedAt,targetCommitish,assets";

/// The `gh release list` command line, on the `origin` repo when there is
/// one. A function, like `issue_list_args`, so a test can read it.
fn release_list_args(repo: Option<&str>) -> Vec<&str> {
    let mut args = vec![
        "release",
        "list",
        "--exclude-drafts",
        "--limit",
        RELEASE_LIMIT,
        "--json",
        RELEASE_JSON_FIELDS,
    ];
    // `-R`: in a fork, `gh` reads the upstream when it is the folder's
    // default (`gh repo set-default`); the tab shows the folder's own repo,
    // the one its page and its unreleased count are taken from.
    if let Some(repo) = repo {
        args.extend(["-R", repo]);
    }
    args
}

/// The `gh release view` command line, asking for `fields`: GitHub's "Latest" without a tag,
/// that release with one. The tag comes last, after `--`, so it can only
/// ever be read as a tag.
fn release_view_args<'a>(
    fields: &'a str,
    repo: Option<&'a str>,
    tag: Option<&'a str>,
) -> Vec<&'a str> {
    let mut args = vec!["release", "view", "--json", fields];
    if let Some(repo) = repo {
        args.extend(["-R", repo]);
    }
    if let Some(tag) = tag {
        args.extend(["--", tag]);
    }
    args
}

/// Whether `gh release view` failed only because the repo has no "Latest"
/// release — nothing published, or pre-releases only.
fn is_release_not_found(err: &anyhow::Error) -> bool {
    err.to_string().contains("release not found")
}

/// The Releases tab's row for `repo_dir`: its latest release (if any) and
/// its releases page. ONE row whatever `gh` lists, in a `Vec` so that it
/// goes through `fan_out` like the other tabs' lists — a repo without a
/// release still gets its row, and a failed `gh` still counts as a failed
/// repo.
pub fn fetch_release(repo_dir: &Path) -> Result<Vec<RepoRelease>> {
    let source = release_source(repo_dir);
    let repo = source.as_ref().and_then(ReleaseSource::gh_repo);
    let release = latest_release(repo_dir, repo.as_deref())?;
    let unreleased = release
        .as_ref()
        .and_then(|r| unreleased_commits(repo_dir, &r.tag_name));
    Ok(vec![RepoRelease {
        repo: String::new(), // stamped by the loader
        releases_page: source.as_ref().map(ReleaseSource::releases_page),
        release,
        unreleased,
    }])
}

/// A repo's latest release, its page included. GitHub's own "Latest"
/// first: one call, whatever the number of pre-releases published since.
/// Without one, the newest of the releases listed — a repo that only ever
/// shipped pre-releases still has something to show — then that one's
/// page, asked of `gh` rather than built from a tag.
fn latest_release(repo_dir: &Path, repo: Option<&str>) -> Result<Option<Release>> {
    match run_gh_json::<Release, _>(
        &release_view_args(RELEASE_VIEW_JSON_FIELDS, repo, None),
        Some(repo_dir),
    ) {
        Ok(release) => return Ok(Some(release)),
        Err(err) if !is_release_not_found(&err) => return Err(err),
        Err(_) => {} // no "Latest": look further
    }
    let list: Vec<Release> = run_gh_json(&release_list_args(repo), Some(repo_dir))?;
    let Some(newest) = releases::latest(list) else {
        return Ok(None);
    };
    // Its page is a nicety: without it, `enter` opens the releases page.
    let args = release_view_args(RELEASE_VIEW_JSON_FIELDS, repo, Some(&newest.tag_name));
    Ok(Some(run_gh_json(&args, Some(repo_dir)).unwrap_or(newest)))
}

/// The repo the folder's `origin` points to, on its real host: a
/// `~/.ssh/config` alias (`git@github-perso:o/n`) is resolved to the host
/// it stands for, so `gh` is asked about the right GitHub. `None` without
/// a usable `origin`: `gh` then picks the repo itself, as in the other
/// tabs.
fn release_source(dir: &Path) -> Option<ReleaseSource> {
    let output = git_in(dir)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    let remote = parse_remote(&String::from_utf8_lossy(&output.stdout))?;
    let host = if remote.ssh {
        // The ssh port, if any, is ssh's business, not GitHub's.
        let alias = remote.host.split(':').next().unwrap_or_default();
        ssh_hostname(alias).unwrap_or_else(|| alias.to_string())
    } else {
        remote.host
    };
    Some(ReleaseSource {
        host,
        repo: remote.repo,
    })
}

/// The host an ssh alias stands for, from `ssh -G`: it prints the
/// configuration ssh would use, read from `~/.ssh/config`, without
/// connecting anywhere.
fn ssh_hostname(alias: &str) -> Option<String> {
    let output = Command::new("ssh")
        .args(["-G", "--", alias])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    parse_ssh_hostname(&String::from_utf8_lossy(&output.stdout))
}

/// The `hostname` line of what `ssh -G` printed, if it names a host.
fn parse_ssh_hostname(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|line| line.strip_prefix("hostname "))
        .map(str::trim)
        .filter(|host| is_name(host))
        .map(str::to_string)
}

/// How many commits the default branch holds past `tag`, from what this
/// clone already knows: `git rev-list --count <tag>..origin/HEAD`, local
/// only — never a fetch, so the count is as fresh as the last `git fetch`.
/// `None` when the tag or `origin/HEAD` is missing here (a clone made with
/// `git init` + `remote add` has no `origin/HEAD`).
pub fn unreleased_commits(repo_dir: &Path, tag: &str) -> Option<u64> {
    // `refs/tags/` spelled out: a branch of the same name cannot be picked
    // instead, and a tag starting with `-` cannot pass for an option.
    let range = format!("refs/tags/{tag}..origin/HEAD");
    let output = git_in(repo_dir)
        .args(["rev-list", "--count", &range])
        .output()
        .ok()
        .filter(|output| output.status.success())?;
    parse_count(&String::from_utf8_lossy(&output.stdout))
}

/// The number `git rev-list --count` prints, or `None` for anything else.
fn parse_count(stdout: &str) -> Option<u64> {
    stdout.trim().parse().ok()
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
        .map(|folder| LocalRepo {
            origin: origin_of(&root.join(&folder)),
            folder,
            is_git: true,
        })
        .chain(others)
        .collect()
}

/// `git` run on the repo in `dir`, and on no other. `-C` alone is not
/// enough: `GIT_DIR` and `GIT_WORK_TREE`, when set, win over it — `git
/// rebase --exec` sets them for the command it runs, so a `cargo test`
/// under it had the tests' scratch commits and tags land in the repo being
/// rebased.
fn git_in(dir: &Path) -> Command {
    let mut git = Command::new("git");
    git.arg("-C").arg(dir);
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
    ] {
        git.env_remove(var);
    }
    git
}

/// The `owner/name` the `origin` remote of the repo in `dir` points to, or
/// `None` without a GitHub origin. Local `git`, no network.
fn origin_of(dir: &Path) -> Option<String> {
    git_in(dir)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| parse_origin(&String::from_utf8_lossy(&output.stdout)))
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

    /// `model::Pr` reads `mergeable` and `statusCheckRollup` with serde
    /// defaults: if the request stopped asking for them, every PR would
    /// silently read "no conflict, no checks" instead of failing loudly.
    #[test]
    fn the_pr_request_asks_for_the_fields_the_status_column_needs() {
        for field in [
            "isDraft",
            "reviewDecision",
            "mergeable",
            "statusCheckRollup",
        ] {
            assert!(
                JSON_FIELDS.split(',').any(|f| f == field),
                "JSON_FIELDS must request {field}"
            );
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

    /// A few published releases are enough to find the latest one: drafts
    /// are left to GitHub, so they cannot fill the window.
    #[test]
    fn release_list_asks_for_a_few_published_releases() {
        assert_eq!(
            release_list_args(None),
            [
                "release",
                "list",
                "--exclude-drafts",
                "--limit",
                "10",
                "--json",
                "tagName,name,publishedAt,isLatest,isDraft,isPrerelease",
            ]
        );
    }

    /// The releases are the folder's own repo's, the one `origin` points
    /// to: in a fork, `gh` alone would pick the upstream when it is set as
    /// the default (`gh repo set-default`), while the page and the
    /// unreleased count are the fork's.
    #[test]
    fn release_list_asks_for_the_origin_repo() {
        let args = release_list_args(Some("Venatum/bull-board-docker"));
        assert_eq!(args[args.len() - 2..], ["-R", "Venatum/bull-board-docker"]);
    }

    /// No tag: GitHub's own "Latest", whatever the number of pre-releases
    /// published since, with its page.
    #[test]
    fn release_view_asks_for_the_latest_and_its_page() {
        assert_eq!(
            release_view_args(RELEASE_VIEW_JSON_FIELDS, Some("acme/api"), None),
            [
                "release",
                "view",
                "--json",
                "tagName,name,publishedAt,isDraft,isPrerelease,url",
                "-R",
                "acme/api",
            ]
        );
    }

    /// A tag comes last, after `--`: whatever it holds, it is a tag.
    #[test]
    fn release_view_takes_a_tag_after_the_options() {
        let args = release_view_args(RELEASE_VIEW_JSON_FIELDS, None, Some("v1.0.0-beta"));
        assert_eq!(args[args.len() - 2..], ["--", "v1.0.0-beta"]);
        assert!(!args.contains(&"-R"));
    }

    /// What `gh release view` says for a repo with no "Latest" (nothing
    /// released, or pre-releases only), apart from any other failure.
    #[cfg(unix)]
    #[test]
    fn no_latest_release_is_told_apart_from_a_failure() {
        let none =
            parse_gh_output::<Release>("gh release view", &gh_output(1, "", "release not found\n"));
        let failure =
            parse_gh_output::<Release>("gh release view", &gh_output(1, "", "HTTP 403\n"));
        assert!(is_release_not_found(&none.unwrap_err()));
        assert!(!is_release_not_found(&failure.unwrap_err()));
    }

    /// `ssh -G <alias>` prints the configuration it would use, the real
    /// host among it; a host GitHub could not have is no host.
    #[test]
    fn the_real_host_is_what_ssh_g_says() {
        let out = "user git\nhostname github.com\nport 22\n";
        assert_eq!(parse_ssh_hostname(out).as_deref(), Some("github.com"));
        assert_eq!(parse_ssh_hostname("user git\n"), None);
        assert_eq!(parse_ssh_hostname("hostname evil&calc\n"), None);
    }

    #[test]
    fn a_commit_count_parses_from_what_git_prints() {
        assert_eq!(parse_count("3\n"), Some(3));
        assert_eq!(parse_count("0"), Some(0));
        assert_eq!(parse_count(""), None);
        assert_eq!(parse_count("fatal: bad revision"), None);
    }

    /// Runs `git` in `dir` for the test's setup. Signing and hooks off: the
    /// user's own git config must not decide whether the test passes.
    fn git(dir: &Path, args: &[&str]) {
        let status = git_in(dir)
            .args([
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "tag.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// A real repo: a tag, two commits after it, and an `origin/HEAD` on
    /// the last one — what a clone that fetched since the release looks
    /// like. No remote is ever contacted.
    #[test]
    fn unreleased_counts_the_local_commits_since_the_tag() {
        let dir = std::env::temp_dir().join(format!("gh-ui-unreleased-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "--quiet"]);
        git(&dir, &["commit", "--allow-empty", "--quiet", "-m", "one"]);
        git(&dir, &["tag", "v1.0.0"]);
        git(&dir, &["commit", "--allow-empty", "--quiet", "-m", "two"]);
        git(&dir, &["commit", "--allow-empty", "--quiet", "-m", "three"]);

        // Without origin/HEAD, there is nothing to compare with.
        assert_eq!(unreleased_commits(&dir, "v1.0.0"), None);

        git(&dir, &["update-ref", "refs/remotes/origin/HEAD", "HEAD"]);
        assert_eq!(unreleased_commits(&dir, "v1.0.0"), Some(2));
        // A tag that was never fetched here: unknown, not zero.
        assert_eq!(unreleased_commits(&dir, "v9.9.9"), None);

        // git resolves a bare name to `refs/<name>` before `refs/tags/<name>`:
        // another ref of the tag's name, on the last commit, must not be
        // what the count starts from.
        git(&dir, &["update-ref", "refs/v1.0.0", "HEAD"]);
        assert_eq!(unreleased_commits(&dir, "v1.0.0"), Some(2));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// One PR, by number, in the PR's own folder: `gh` reads the repo from
    /// there, as for `gh pr list`.
    #[test]
    fn pr_view_asks_for_one_pr_by_number() {
        assert_eq!(
            pr_view_args(412),
            ["pr", "view", "412", "--json", PR_VIEW_FIELDS]
        );
    }

    /// The issue's detail, in one call: description, people, linked PRs
    /// and the conversation together.
    #[test]
    fn issue_view_asks_for_what_the_detail_reads() {
        assert_eq!(
            issue_view_args(57),
            [
                "issue",
                "view",
                "57",
                "--json",
                "title,url,body,state,stateReason,author,createdAt,labels,assignees,closedByPullRequestsReferences,comments",
            ]
        );
    }

    /// The run by its id, with its jobs and their steps, in one call.
    #[test]
    fn run_view_asks_for_the_jobs_too() {
        let args = run_view_args(99);
        assert_eq!(args[..4], ["run", "view", "99", "--json"]);
        assert!(args[4].split(',').any(|f| f == "jobs"));
    }

    /// The list must bring each run's id: `gh run view` takes nothing else.
    #[test]
    fn run_list_asks_for_the_id_the_detail_needs() {
        assert!(RUN_JSON_FIELDS.split(',').any(|f| f == "databaseId"));
    }

    /// `detail::PrDetail` defaults most of its fields: if the request
    /// stopped asking for one, the view would silently read "no review, no
    /// label" instead of failing loudly.
    #[test]
    fn the_pr_view_asks_for_every_field_the_view_reads() {
        for field in [
            "body",
            "baseRefName",
            "headRefName",
            "reviewDecision",
            "mergeable",
            "labels",
            "reviewRequests",
            "latestReviews",
            "reviews",
            "headRefOid",
            "statusCheckRollup",
            "files",
            "comments",
        ] {
            assert!(
                PR_VIEW_FIELDS.split(',').any(|f| f == field),
                "PR_VIEW_FIELDS must request {field}"
            );
        }
    }

    /// `comments` would bring every comment's body: 177 KB and 2.2 s instead
    /// of 4.6 KB and 0.6 s for 50 issues, only to print a count.
    #[test]
    fn issue_list_never_asks_for_the_comments() {
        assert!(!ISSUE_JSON_FIELDS.split(',').any(|f| f == "comments"));
    }
}
