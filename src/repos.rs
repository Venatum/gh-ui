//! The Repos tab's logic: an owner's repositories, which of them already sit
//! in the scanned folder, the ticks and the clone batch. Pure: `gh.rs` runs
//! the commands, `fetch.rs` carries their results, this module decides what
//! they mean — so every rule here is unit-tested without running `gh`.

use serde::Deserialize;

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
    pub pushed_at: String,
    pub url: String,
}

/// A git repo found in the scanned folder: its folder name, and the
/// `owner/name` its `origin` remote points to (`None`: no origin, or not a
/// GitHub URL).
#[derive(Debug, Clone, PartialEq)]
pub struct LocalRepo {
    pub folder: String,
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
    Queued,
    Cloning,
    /// The clone failed; carries `gh`'s message.
    Failed(String),
}

/// The `owner/name` a GitHub remote URL points to, or `None` for anything
/// else. Accepts the three forms `git` writes — `https://github.com/o/n`,
/// `git@github.com:o/n` and `ssh://git@github.com/o/n` — each with or
/// without the `.git` suffix.
pub fn parse_origin(url: &str) -> Option<String> {
    let url = url.trim();
    let path = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("git@github.com:"))
        .or_else(|| url.strip_prefix("ssh://git@github.com/"))?;
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let (owner, name) = path.split_once('/')?;
    if owner.is_empty() || name.is_empty() || name.contains('/') {
        return None;
    }
    Some(format!("{owner}/{name}"))
}

/// The state `repo` has from the folder alone (no clone in progress). The
/// folder is matched on its exact name, as `discover_repos` lists it; the
/// origin case-insensitively, as GitHub treats owners and names.
pub fn local_state(repo: &Repo, locals: &[LocalRepo]) -> RepoState {
    match locals.iter().find(|local| local.folder == repo.name) {
        None => RepoState::Clonable,
        Some(local) => match &local.origin {
            Some(origin) if origin.eq_ignore_ascii_case(&repo.name_with_owner) => RepoState::Cloned,
            other => RepoState::NameTaken(other.clone()),
        },
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
        pushed_at: "2026-09-20T10:00:00Z".to_string(),
        url: format!("https://github.com/{name_with_owner}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(folder: &str, origin: Option<&str>) -> LocalRepo {
        LocalRepo {
            folder: folder.to_string(),
            origin: origin.map(str::to_string),
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
    fn anything_but_a_github_repo_url_is_rejected() {
        assert_eq!(parse_origin("https://gitlab.com/acme/api.git"), None);
        assert_eq!(parse_origin("https://github.com/acme"), None);
        assert_eq!(parse_origin("https://github.com/acme/api/tree/main"), None);
        assert_eq!(parse_origin(""), None);
        assert_eq!(parse_origin("not a url"), None);
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
}
