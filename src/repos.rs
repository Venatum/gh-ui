//! The Repos tab's logic: an owner's repositories, which of them already sit
//! in the scanned folder, the ticks and the clone batch. Pure: `gh.rs` runs
//! the commands, `fetch.rs` carries their results, this module decides what
//! they mean — so every rule here is unit-tested without running `gh`.

use crate::config;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// A repository, as `gh repo list --json ...` returns it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repo {
    /// `owner/name`: the label shown in the table, and the key of the ticks
    /// and of the clone states.
    pub name_with_owner: String,
    /// The bare name: the folder `gh repo clone` creates by default.
    pub name: String,
    /// `null` for a repo without a description, hence the `Option`.
    #[serde(default)]
    pub description: Option<String>,
    /// "PUBLIC" | "PRIVATE" | "INTERNAL".
    pub visibility: String,
    pub is_archived: bool,
    pub is_fork: bool,
    /// Nullable on GitHub's side (a repo never pushed to): one `null` must
    /// not fail the parse of the whole list.
    #[serde(default)]
    pub pushed_at: Option<String>,
    pub url: String,
}

/// An entry of the scanned folder: its name, whether it is a git repo, and
/// the `owner/name` its `origin` remote points to (`None`: not a repo, no
/// origin, or not a GitHub URL). Plain folders count: a clone cannot land
/// on them either.
#[derive(Debug, Clone, PartialEq)]
pub struct LocalRepo {
    pub folder: String,
    pub is_git: bool,
    pub origin: Option<String>,
}

/// Where a remote repo stands against the scanned folder, plus the three
/// transient states of a clone batch.
#[derive(Debug, Clone, PartialEq)]
pub enum RepoState {
    /// No folder of that name: can be ticked and cloned.
    Clonable,
    /// A folder of that name whose origin is this very repo.
    Cloned,
    /// A folder of that name holding something else: the origin found there,
    /// `None` when it has none. Not clonable — `gh repo clone` would refuse
    /// an existing destination.
    NameTaken(Option<String>),
    /// Something of that name that is not a git repo (a plain folder, a
    /// file): not clonable either.
    FolderTaken,
    Queued,
    Cloning,
    /// The clone failed; carries `gh`'s message.
    Failed(String),
}

/// Where a remote URL points: its host, as written, and the repo's
/// `owner/name`.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteUrl {
    /// `github.com`, a GitHub Enterprise host, or — for the ssh forms — a
    /// `~/.ssh/config` alias (`github-perso`) standing for one of them.
    pub host: String,
    pub repo: String,
    /// One of the two ssh forms, whose host may be such an alias.
    pub ssh: bool,
}

/// Parses the three forms `git` writes — `https://host/o/n`, `git@host:o/n`
/// and `ssh://git@host/o/n` — each with or without the `.git` suffix.
/// `None` for anything else, and for any part holding a character GitHub
/// would refuse in a name: `.git/config` is a local file anyone can write
/// into a repo they hand over, and what comes out of it reaches command
/// lines and URLs.
pub fn parse_remote(url: &str) -> Option<RemoteUrl> {
    let url = url.trim();
    let (ssh, rest, separator) = if let Some(rest) = url.strip_prefix("https://") {
        (false, rest, '/')
    } else if let Some(rest) = url.strip_prefix("ssh://git@") {
        (true, rest, '/')
    } else {
        (true, url.strip_prefix("git@")?, ':')
    };
    let (host, path) = rest.split_once(separator)?;
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    // `:` in a host only for a port (`ssh://git@host:2222/o/n`).
    let host_ok = is_name(&host.replace(':', ""));
    if !host_ok || !is_name(owner) || !is_name(name) {
        return None;
    }
    Some(RemoteUrl {
        host: host.to_string(),
        repo: format!("{owner}/{name}"),
        ssh,
    })
}

/// A host, owner or repo name made only of what GitHub allows in one
/// (letters, digits, `-`, `_`, `.`), and not starting with `-`, which a
/// command would read as an option.
pub fn is_name(part: &str) -> bool {
    !part.is_empty()
        && !part.starts_with('-')
        && part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The `owner/name` a GitHub remote URL points to, or `None` for anything
/// else (see `parse_remote`). The https form must be github.com; the two
/// ssh forms take any host: a `~/.ssh/config` alias
/// (`git@github-perso:o/n`) is how one machine holds several GitHub
/// accounts, and the caller compares the result with the repo's own
/// `owner/name` anyway.
pub fn parse_origin(url: &str) -> Option<String> {
    let remote = parse_remote(url)?;
    (remote.ssh || remote.host == "github.com").then_some(remote.repo)
}

/// The state `repo` has from the folder alone (no clone in progress). The
/// folder is matched on its exact name, as `discover_repos` lists it; the
/// origin case-insensitively, as GitHub treats owners and names.
pub fn local_state(repo: &Repo, locals: &[LocalRepo]) -> RepoState {
    match locals.iter().find(|local| local.folder == repo.name) {
        None => RepoState::Clonable,
        Some(local) if !local.is_git => RepoState::FolderTaken,
        Some(local) => match &local.origin {
            Some(origin) if origin.eq_ignore_ascii_case(&repo.name_with_owner) => RepoState::Cloned,
            other => RepoState::NameTaken(other.clone()),
        },
    }
}

/// The Repos tab's filters. `owner` and the two boxes reach `gh`;
/// `hide_cloned` narrows the list in memory.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RepoFilters {
    /// Whose repos to list. `None` until the account is known.
    pub owner: Option<String>,
    /// Show archived repos too (unticked = `--no-archived`).
    pub archived: bool,
    /// Show forks too (unticked = `--source`).
    pub forks: bool,
    /// Hide the repos already cloned.
    pub hide_cloned: bool,
}

impl RepoFilters {
    /// The `gh repo list` flags the two boxes stand for. Filtering on
    /// GitHub's side keeps the response small.
    pub fn to_gh_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if !self.archived {
            args.push("--no-archived".to_string());
        }
        if !self.forks {
            args.push("--source".to_string());
        }
        args
    }

    /// Would `gh` answer both with the same list? `hide cloned` narrows
    /// in memory, so it never outdates an answer.
    pub fn asks_the_same(&self, other: &RepoFilters) -> bool {
        self.owner == other.owner && self.archived == other.archived && self.forks == other.forks
    }

    /// The header's summary: `owner:acme`, plus what departs from the
    /// defaults.
    pub fn summary(&self) -> String {
        let mut parts = vec![format!("owner:{}", self.owner.as_deref().unwrap_or("…"))];
        if self.archived {
            parts.push("+archived".to_string());
        }
        if self.forks {
            parts.push("+forks".to_string());
        }
        if self.hide_cloned {
            parts.push("hide cloned".to_string());
        }
        parts.join(" ")
    }
}

/// The owner picker's values: the account first, then its orgs, without
/// duplicates (compared case-insensitively, as GitHub does).
pub fn owners(login: Option<&str>, orgs: &[String]) -> Vec<String> {
    let mut list: Vec<String> = login.map(str::to_string).into_iter().collect();
    for org in orgs {
        if !list.iter().any(|o| o.eq_ignore_ascii_case(org)) {
            list.push(org.clone());
        }
    }
    list
}

/// The owner after `current` in `owners` (before it when `forward` is
/// false), wrapping. An owner the list no longer holds restarts at the
/// first one, the account.
pub fn cycle_owner(owners: &[String], current: Option<&str>, forward: bool) -> Option<String> {
    let n = owners.len();
    if n == 0 {
        return None;
    }
    let next = match current.and_then(|c| owners.iter().position(|o| o == c)) {
        Some(i) if forward => (i + 1) % n,
        Some(i) => (i + n - 1) % n,
        None => 0,
    };
    Some(owners[next].clone())
}

/// `saved` if the picker still offers it, otherwise the first owner (the
/// account). An org the account has left must not keep the tab empty.
pub fn valid_owner(saved: Option<&str>, owners: &[String]) -> Option<String> {
    match saved {
        Some(s) if owners.iter().any(|o| o.eq_ignore_ascii_case(s)) => Some(s.to_string()),
        _ => owners.first().cloned(),
    }
}

/// What the Repos tab persists: the owner last picked. The boxes are not
/// saved — a `+archived` left on yesterday would clutter today's list with
/// no word of why.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RepoSettings {
    #[serde(default)]
    pub owner: Option<String>,
}

impl RepoSettings {
    /// Parses the file's contents; malformed gives the defaults. Split out
    /// of `load` so it is testable without the filesystem.
    fn from_json(content: &str) -> Self {
        serde_json::from_str(content).unwrap_or_default()
    }

    /// Reloads the settings, falling back to the defaults if the file is
    /// missing or unreadable — the same lenient behaviour as `Filters::load`.
    pub fn load() -> Self {
        let Some(path) = config::path("repos.json") else {
            return Self::default();
        };
        match std::fs::read_to_string(&path) {
            Ok(content) => Self::from_json(&content),
            Err(_) => Self::default(),
        }
    }

    /// Saves the settings as JSON (silent on failure).
    pub fn save(&self) {
        let Some(path) = config::path("repos.json") else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }
}

/// What the clone thread reports, one message per step. Keyed by
/// `owner/name`.
#[derive(Debug, Clone, PartialEq)]
pub enum CloneEvent {
    Started(String),
    Done(String),
    /// Named fields: two `String`s side by side read either way round.
    Failed {
        repo: String,
        message: String,
    },
    /// The whole batch is over.
    Finished,
}

/// Everything the Repos tab holds. One field of `App`, so `app.rs` only
/// wires keys and messages to it.
#[derive(Debug, Default)]
pub struct ReposTab {
    /// The owner's repos, as last fetched.
    pub repos: Vec<Repo>,
    /// The scanned folder's repos and their origins, read with the list.
    pub locals: Vec<LocalRepo>,
    pub filters: RepoFilters,
    /// The account's orgs; `None` until `gh org list` answers.
    pub orgs: Option<Vec<String>>,
    /// Ticked repos, by `owner/name`.
    pub ticked: HashSet<String>,
    /// Clone-batch states, by `owner/name`. They override `local_state`
    /// until the owner changes, so a failure stays on screen after the
    /// reload that ends the batch.
    pub progress: HashMap<String, RepoState>,
    /// A clone batch is running.
    pub cloning: bool,
    /// The list has been fetched at least once for the current owner.
    pub loaded: bool,
    /// Outcome counts of the running batch, for its closing status line.
    batch_ok: usize,
    batch_failed: usize,
}

impl ReposTab {
    /// A tab listing `owner`'s repos (the saved one, or `None` until the
    /// account is known), everything else at its start. Built here, where
    /// the private counters are reachable.
    pub fn with_owner(owner: Option<String>) -> Self {
        Self {
            filters: RepoFilters {
                owner,
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// The state a row shows: the batch's if it holds one, the folder's
    /// otherwise.
    pub fn state_of(&self, repo: &Repo) -> RepoState {
        self.progress
            .get(&repo.name_with_owner)
            .cloned()
            .unwrap_or_else(|| local_state(repo, &self.locals))
    }

    /// Can `repo` be ticked? Only when nothing sits in its folder — a
    /// failed clone counts, so it can be retried.
    fn is_clonable(&self, repo: &Repo) -> bool {
        matches!(
            self.state_of(repo),
            RepoState::Clonable | RepoState::Failed(_)
        )
    }

    /// The rows before the search: every repo, minus the cloned ones when
    /// `hide cloned` is ticked.
    pub fn listed(&self) -> Vec<&Repo> {
        self.repos
            .iter()
            .filter(|r| !(self.filters.hide_cloned && self.state_of(r) == RepoState::Cloned))
            .collect()
    }

    pub fn cloned_count(&self) -> usize {
        self.repos
            .iter()
            .filter(|r| self.state_of(r) == RepoState::Cloned)
            .count()
    }

    pub fn failed_count(&self) -> usize {
        self.progress
            .values()
            .filter(|s| matches!(s, RepoState::Failed(_)))
            .count()
    }

    /// Ticks or unticks the repo `name_with_owner`. `Err` says why it
    /// cannot be ticked, for the status line.
    pub fn toggle_tick(&mut self, name_with_owner: &str) -> Result<(), String> {
        let Some(repo) = self
            .repos
            .iter()
            .find(|r| r.name_with_owner == name_with_owner)
        else {
            return Ok(());
        };
        let (state, folder) = (self.state_of(repo), repo.name.clone());
        match state {
            RepoState::Clonable | RepoState::Failed(_) => {
                if !self.ticked.remove(name_with_owner) {
                    self.ticked.insert(name_with_owner.to_string());
                }
                Ok(())
            }
            RepoState::Cloned => Err(format!("{name_with_owner} is already cloned")),
            RepoState::NameTaken(_) | RepoState::FolderTaken => {
                Err(format!("folder {folder}/ is taken"))
            }
            RepoState::Queued | RepoState::Cloning => {
                Err(format!("{name_with_owner} is being cloned"))
            }
        }
    }

    /// The clone button shows while something is ticked and no batch runs.
    pub fn show_button(&self) -> bool {
        !self.ticked.is_empty() && !self.cloning
    }

    /// Starts a batch: the ticked repos in list order, as (`owner/name`,
    /// folder). They turn `Queued` and the ticks clear.
    pub fn start_batch(&mut self) -> Vec<(String, String)> {
        let batch: Vec<(String, String)> = self
            .repos
            .iter()
            .filter(|r| self.ticked.contains(&r.name_with_owner))
            .map(|r| (r.name_with_owner.clone(), r.name.clone()))
            .collect();
        for (name_with_owner, _) in &batch {
            self.progress
                .insert(name_with_owner.clone(), RepoState::Queued);
        }
        self.ticked.clear();
        self.cloning = !batch.is_empty();
        self.batch_ok = 0;
        self.batch_failed = 0;
        batch
    }

    /// Applies one message of the clone thread; returns the status line it
    /// calls for.
    pub fn apply_event(&mut self, event: CloneEvent) -> String {
        match event {
            CloneEvent::Started(name) => {
                let line = format!("Cloning {name}…");
                self.progress.insert(name, RepoState::Cloning);
                line
            }
            CloneEvent::Done(name) => {
                self.batch_ok += 1;
                let line = format!("✓ {name} cloned");
                self.progress.insert(name, RepoState::Cloned);
                line
            }
            CloneEvent::Failed { repo, message } => {
                self.batch_failed += 1;
                let line = format!("✗ {repo}: {message}");
                self.progress.insert(repo, RepoState::Failed(message));
                line
            }
            CloneEvent::Finished => {
                self.cloning = false;
                format!(
                    "Clones finished: {} ok, {} failed",
                    self.batch_ok, self.batch_failed
                )
            }
        }
    }

    /// Stores a fresh list. Batch states the folder now confirms (`Cloned`)
    /// give way to it, and so does a failure whose folder is no longer free
    /// (cloned by hand since). A tick on a repo that is no longer clonable —
    /// cloned behind gh-ui's back, say — is dropped rather than left to fail.
    pub fn apply_load(&mut self, repos: Vec<Repo>, locals: Vec<LocalRepo>) {
        self.repos = repos;
        self.locals = locals;
        self.loaded = true;
        let free: HashSet<String> = self
            .repos
            .iter()
            .filter(|r| local_state(r, &self.locals) == RepoState::Clonable)
            .map(|r| r.name_with_owner.clone())
            .collect();
        self.progress.retain(|name, state| match state {
            RepoState::Cloned => false,
            RepoState::Failed(_) => free.contains(name),
            _ => true,
        });
        let clonable: HashSet<String> = self
            .repos
            .iter()
            .filter(|r| self.is_clonable(r))
            .map(|r| r.name_with_owner.clone())
            .collect();
        self.ticked.retain(|name| clonable.contains(name));
    }

    /// Switches owner. The list, the ticks and the batch states all
    /// belonged to the previous one: showing them under the new name would
    /// lie until the reload lands.
    pub fn set_owner(&mut self, owner: String) {
        self.filters.owner = Some(owner);
        self.repos.clear();
        self.ticked.clear();
        self.progress.clear();
        self.loaded = false;
    }
}

/// A repo with plausible filler, for the tests of every module that needs
/// one. `name` is derived from `owner/name`, as GitHub does.
#[cfg(test)]
pub fn sample_repo(name_with_owner: &str) -> Repo {
    let name = name_with_owner
        .split_once('/')
        .map_or(name_with_owner, |(_, name)| name);
    Repo {
        name_with_owner: name_with_owner.to_string(),
        name: name.to_string(),
        description: None,
        visibility: "PRIVATE".to_string(),
        is_archived: false,
        is_fork: false,
        pushed_at: Some("2026-09-20T10:00:00Z".to_string()),
        url: format!("https://github.com/{name_with_owner}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(folder: &str, origin: Option<&str>) -> LocalRepo {
        LocalRepo {
            folder: folder.to_string(),
            is_git: true,
            origin: origin.map(str::to_string),
        }
    }

    /// A folder (or file) of the scanned folder that is not a git repo.
    fn plain(folder: &str) -> LocalRepo {
        LocalRepo {
            folder: folder.to_string(),
            is_git: false,
            origin: None,
        }
    }

    #[test]
    fn the_https_form_gives_owner_and_name() {
        assert_eq!(
            parse_origin("https://github.com/acme/api.git"),
            Some("acme/api".to_string())
        );
        assert_eq!(
            parse_origin("https://github.com/acme/api"),
            Some("acme/api".to_string())
        );
        // `git remote get-url` ends its output with a newline.
        assert_eq!(
            parse_origin("https://github.com/acme/api.git\n"),
            Some("acme/api".to_string())
        );
    }

    #[test]
    fn both_ssh_forms_give_owner_and_name() {
        assert_eq!(
            parse_origin("git@github.com:acme/api.git"),
            Some("acme/api".to_string())
        );
        assert_eq!(
            parse_origin("ssh://git@github.com/acme/api"),
            Some("acme/api".to_string())
        );
    }

    #[test]
    fn an_ssh_host_alias_gives_owner_and_name() {
        // `~/.ssh/config` can name github.com anything, to pick a key per
        // account: the alias says nothing, the path is what matters.
        assert_eq!(
            parse_origin("git@github-perso:acme/api.git"),
            Some("acme/api".to_string())
        );
        assert_eq!(
            parse_origin("ssh://git@github-work/acme/api.git"),
            Some("acme/api".to_string())
        );
    }

    #[test]
    fn anything_but_a_github_repo_url_is_rejected() {
        assert_eq!(parse_origin("https://gitlab.com/acme/api.git"), None);
        assert_eq!(parse_origin("https://github.com/acme"), None);
        assert_eq!(parse_origin("https://github.com/acme/api/tree/main"), None);
        assert_eq!(parse_origin(""), None);
        assert_eq!(parse_origin("not a url"), None);
    }

    /// `.git/config` is a local file anyone can write into a repo they hand
    /// over: a name GitHub would refuse (`&`, `|`, `%`, quotes...) never
    /// passes for a repo, so it cannot reach a command line or a URL.
    #[test]
    fn a_name_github_would_refuse_is_rejected() {
        assert_eq!(parse_origin("git@github.com:acme/api&calc.git"), None);
        assert_eq!(parse_origin("https://github.com/ac|me/api"), None);
        assert_eq!(parse_origin("git@github.com:acme/%PATH%"), None);
        // An option, not a host: `ssh -G` would read it as one.
        assert_eq!(parse_remote("git@-oProxyCommand:acme/api"), None);
    }

    /// The host is kept, as written: an ssh alias for the ssh forms.
    #[test]
    fn a_remote_gives_its_host_and_repo() {
        let remote = |host: &str, ssh| {
            Some(RemoteUrl {
                host: host.to_string(),
                repo: "acme/api".to_string(),
                ssh,
            })
        };
        assert_eq!(
            parse_remote("https://github.com/acme/api.git"),
            remote("github.com", false)
        );
        assert_eq!(
            parse_remote("https://ghe.corp/acme/api"),
            remote("ghe.corp", false)
        );
        assert_eq!(
            parse_remote("git@github-perso:acme/api.git"),
            remote("github-perso", true)
        );
        assert_eq!(
            parse_remote("ssh://git@gitlab.com:2222/acme/api"),
            remote("gitlab.com:2222", true)
        );
    }

    #[test]
    fn a_folder_whose_origin_is_the_repo_is_cloned() {
        let repo = sample_repo("acme/api");
        assert_eq!(
            local_state(&repo, &[local("api", Some("acme/api"))]),
            RepoState::Cloned
        );
        // GitHub ignores case in owners and names: so do we.
        assert_eq!(
            local_state(&repo, &[local("api", Some("ACME/Api"))]),
            RepoState::Cloned
        );
    }

    #[test]
    fn a_folder_holding_something_else_is_a_taken_name() {
        let repo = sample_repo("acme/api");
        assert_eq!(
            local_state(&repo, &[local("api", Some("other/api"))]),
            RepoState::NameTaken(Some("other/api".to_string()))
        );
        assert_eq!(
            local_state(&repo, &[local("api", None)]),
            RepoState::NameTaken(None)
        );
    }

    #[test]
    fn a_new_tab_knows_its_owner_and_nothing_else() {
        let tab = ReposTab::with_owner(Some("acme".to_string()));

        assert_eq!(tab.filters.owner.as_deref(), Some("acme"));
        assert!(!tab.filters.archived && !tab.filters.forks);
        assert!(!tab.loaded && tab.repos.is_empty() && !tab.cloning);
    }

    /// `gh repo clone` refuses to write into it: not clonable, and not a
    /// repo either, so no origin to name.
    #[test]
    fn a_plain_folder_of_that_name_is_taken() {
        let mut tab = ReposTab::default();
        tab.apply_load(vec![sample_repo("acme/api")], vec![plain("api")]);

        assert_eq!(tab.state_of(&tab.repos[0]), RepoState::FolderTaken);
        assert_eq!(
            tab.toggle_tick("acme/api"),
            Err("folder api/ is taken".to_string())
        );
    }

    #[test]
    fn no_folder_of_that_name_means_clonable() {
        let repo = sample_repo("acme/api");
        assert_eq!(local_state(&repo, &[]), RepoState::Clonable);
        // The folder name is compared exactly, like `discover_repos` lists it.
        assert_eq!(
            local_state(&repo, &[local("API", Some("acme/api"))]),
            RepoState::Clonable
        );
    }

    #[test]
    fn a_repo_parses_from_gh_json_even_without_a_description() {
        let repo: Repo = serde_json::from_str(
            r#"{
                "nameWithOwner": "acme/api", "name": "api", "description": null,
                "visibility": "PUBLIC", "isArchived": false, "isFork": true,
                "pushedAt": "2026-09-20T10:00:00Z",
                "url": "https://github.com/acme/api"
            }"#,
        )
        .unwrap();
        assert_eq!(repo.name_with_owner, "acme/api");
        assert_eq!(repo.description, None);
        assert!(repo.is_fork);
    }

    /// A tab holding `repos` (as `owner/name`) against a folder holding
    /// `locals` (folder, origin).
    fn tab(repos: &[&str], locals: &[(&str, Option<&str>)]) -> ReposTab {
        ReposTab {
            repos: repos.iter().map(|r| sample_repo(r)).collect(),
            locals: locals
                .iter()
                .map(|(folder, origin)| local(folder, *origin))
                .collect(),
            loaded: true,
            ..Default::default()
        }
    }

    #[test]
    fn unticked_boxes_filter_on_githubs_side() {
        let f = RepoFilters::default();
        assert_eq!(f.to_gh_args(), ["--no-archived", "--source"]);

        let f = RepoFilters {
            archived: true,
            forks: true,
            ..Default::default()
        };
        assert!(f.to_gh_args().is_empty());

        let f = RepoFilters {
            forks: true,
            ..Default::default()
        };
        assert_eq!(f.to_gh_args(), ["--no-archived"]);
    }

    #[test]
    fn the_summary_names_the_owner_and_what_departs_from_the_defaults() {
        let f = RepoFilters {
            owner: Some("acme".to_string()),
            ..Default::default()
        };
        assert_eq!(f.summary(), "owner:acme");

        let f = RepoFilters {
            owner: None,
            archived: true,
            forks: true,
            hide_cloned: true,
        };
        assert_eq!(f.summary(), "owner:… +archived +forks hide cloned");
    }

    #[test]
    fn the_picker_lists_the_account_then_its_orgs() {
        let orgs = vec!["acme".to_string(), "Vincent".to_string()];
        assert_eq!(owners(Some("vincent"), &orgs), ["vincent", "acme"]);
        // Until the account is known, only the orgs.
        assert_eq!(owners(None, &orgs), ["acme", "Vincent"]);
    }

    #[test]
    fn the_owner_cycles_both_ways_and_wraps() {
        let list = vec!["me".to_string(), "acme".to_string(), "corp".to_string()];
        assert_eq!(
            cycle_owner(&list, Some("me"), true).as_deref(),
            Some("acme")
        );
        assert_eq!(
            cycle_owner(&list, Some("corp"), true).as_deref(),
            Some("me")
        );
        assert_eq!(
            cycle_owner(&list, Some("me"), false).as_deref(),
            Some("corp")
        );
        // An owner the list no longer holds restarts at the account.
        assert_eq!(
            cycle_owner(&list, Some("gone"), true).as_deref(),
            Some("me")
        );
        assert_eq!(cycle_owner(&[], Some("me"), true), None);
    }

    #[test]
    fn a_saved_owner_the_picker_lost_falls_back_to_the_account() {
        let list = vec!["me".to_string(), "acme".to_string()];
        assert_eq!(valid_owner(Some("acme"), &list).as_deref(), Some("acme"));
        assert_eq!(valid_owner(Some("ACME"), &list).as_deref(), Some("ACME"));
        assert_eq!(valid_owner(Some("left-org"), &list).as_deref(), Some("me"));
        assert_eq!(valid_owner(None, &list).as_deref(), Some("me"));
        assert_eq!(valid_owner(Some("acme"), &[]), None);
    }

    #[test]
    fn the_saved_owner_survives_a_json_round_trip() {
        let settings = RepoSettings {
            owner: Some("acme".to_string()),
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(RepoSettings::from_json(&json), settings);
        // A malformed file gives the defaults, like every other settings file.
        assert_eq!(RepoSettings::from_json("{ nope"), RepoSettings::default());
    }

    #[test]
    fn only_a_clonable_row_can_be_ticked() {
        let mut t = tab(
            &["acme/api", "acme/web", "acme/docs"],
            &[("web", Some("acme/web")), ("docs", Some("other/docs"))],
        );

        assert_eq!(t.toggle_tick("acme/api"), Ok(()));
        assert!(t.ticked.contains("acme/api"));
        assert!(t.show_button());
        assert_eq!(t.toggle_tick("acme/api"), Ok(()));
        assert!(t.ticked.is_empty(), "a second space unticks");
        assert!(!t.show_button());

        assert_eq!(
            t.toggle_tick("acme/web"),
            Err("acme/web is already cloned".to_string())
        );
        assert_eq!(
            t.toggle_tick("acme/docs"),
            Err("folder docs/ is taken".to_string())
        );
        assert!(t.ticked.is_empty());
    }

    #[test]
    fn a_failed_clone_can_be_ticked_again() {
        let mut t = tab(&["acme/api"], &[]);
        t.progress.insert(
            "acme/api".to_string(),
            RepoState::Failed("boom".to_string()),
        );
        assert_eq!(t.toggle_tick("acme/api"), Ok(()));
    }

    #[test]
    fn clone_events_walk_the_rows_through_their_states() {
        let mut t = tab(&["acme/api", "acme/web"], &[]);
        t.toggle_tick("acme/web").unwrap();
        t.toggle_tick("acme/api").unwrap();

        // List order, whatever the tick order.
        let batch = t.start_batch();
        assert_eq!(
            batch,
            [
                ("acme/api".to_string(), "api".to_string()),
                ("acme/web".to_string(), "web".to_string()),
            ]
        );
        assert!(t.cloning);
        assert!(t.ticked.is_empty());
        assert!(!t.show_button(), "no second batch while one runs");
        assert_eq!(t.state_of(&t.repos[0]), RepoState::Queued);

        assert_eq!(
            t.apply_event(CloneEvent::Started("acme/api".to_string())),
            "Cloning acme/api…"
        );
        assert_eq!(t.state_of(&t.repos[0]), RepoState::Cloning);
        assert_eq!(
            t.apply_event(CloneEvent::Done("acme/api".to_string())),
            "✓ acme/api cloned"
        );
        assert_eq!(t.state_of(&t.repos[0]), RepoState::Cloned);

        t.apply_event(CloneEvent::Started("acme/web".to_string()));
        assert_eq!(
            t.apply_event(CloneEvent::Failed {
                repo: "acme/web".to_string(),
                message: "denied".to_string(),
            }),
            "✗ acme/web: denied"
        );
        assert_eq!(
            t.state_of(&t.repos[1]),
            RepoState::Failed("denied".to_string())
        );
        assert_eq!(t.failed_count(), 1);

        assert_eq!(
            t.apply_event(CloneEvent::Finished),
            "Clones finished: 1 ok, 1 failed"
        );
        assert!(!t.cloning);
    }

    /// Review focus 4: a repo cloned by hand while ticked would make the
    /// batch fail on an existing folder.
    #[test]
    fn a_reload_drops_the_ticks_that_are_no_longer_clonable() {
        let mut t = tab(&["acme/api", "acme/web"], &[]);
        t.toggle_tick("acme/api").unwrap();
        t.toggle_tick("acme/web").unwrap();

        let repos = t.repos.clone();
        t.apply_load(repos, vec![local("api", Some("acme/api"))]);

        assert!(!t.ticked.contains("acme/api"));
        assert!(t.ticked.contains("acme/web"));
    }

    #[test]
    fn a_reload_keeps_a_failure_and_lets_the_folder_confirm_a_success() {
        let mut t = tab(&["acme/api", "acme/web"], &[]);
        t.progress.insert("acme/api".to_string(), RepoState::Cloned);
        t.progress.insert(
            "acme/web".to_string(),
            RepoState::Failed("boom".to_string()),
        );

        let repos = t.repos.clone();
        t.apply_load(repos, vec![local("api", Some("acme/api"))]);

        assert!(!t.progress.contains_key("acme/api"));
        assert_eq!(t.state_of(&t.repos[0]), RepoState::Cloned);
        assert_eq!(
            t.state_of(&t.repos[1]),
            RepoState::Failed("boom".to_string())
        );
    }

    #[test]
    fn switching_owner_forgets_the_previous_list() {
        let mut t = tab(&["acme/api"], &[]);
        t.toggle_tick("acme/api").unwrap();
        t.progress
            .insert("acme/api".to_string(), RepoState::Failed("x".to_string()));

        t.set_owner("corp".to_string());

        assert_eq!(t.filters.owner.as_deref(), Some("corp"));
        assert!(t.ticked.is_empty());
        assert!(t.progress.is_empty());
        assert!(t.repos.is_empty(), "never show acme's repos under corp");
        assert!(!t.loaded);
    }

    #[test]
    fn hide_cloned_keeps_everything_but_the_cloned_rows() {
        let mut t = tab(
            &["acme/api", "acme/web", "acme/docs"],
            &[("web", Some("acme/web")), ("docs", None)],
        );
        assert_eq!(t.listed().len(), 3);
        assert_eq!(t.cloned_count(), 1);

        t.filters.hide_cloned = true;
        let names: Vec<&str> = t.listed().iter().map(|r| r.name.as_str()).collect();
        // A taken name is not cloned: it stays, it is something to sort out.
        assert_eq!(names, ["api", "docs"]);
    }

    /// Final review I3: a failed row cloned by hand afterwards must read
    /// cloned, and lose its tick, once the folder says so.
    #[test]
    fn a_failed_row_cloned_by_hand_reads_cloned_after_a_reload() {
        let mut t = tab(&["acme/web"], &[]);
        t.progress.insert(
            "acme/web".to_string(),
            RepoState::Failed("auth".to_string()),
        );
        t.toggle_tick("acme/web").unwrap();

        let repos = t.repos.clone();
        t.apply_load(repos, vec![local("web", Some("acme/web"))]);

        assert_eq!(t.state_of(&t.repos[0]), RepoState::Cloned);
        assert!(t.ticked.is_empty());
    }

    /// Final review M9: GitHub's `pushedAt` is nullable; one never-pushed
    /// repo must not fail the parse of the whole list.
    #[test]
    fn a_repo_never_pushed_still_parses() {
        let repo: Repo = serde_json::from_str(
            r#"{
                "nameWithOwner": "acme/empty", "name": "empty", "description": "",
                "visibility": "PRIVATE", "isArchived": false, "isFork": false,
                "pushedAt": null, "url": "https://github.com/acme/empty"
            }"#,
        )
        .unwrap();
        assert_eq!(repo.pushed_at, None);
    }
}
