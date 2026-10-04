//! The Releases tab's logic: a release as `gh release list` returns it, which
//! one is a repo's latest, and where its page lives. Pure: `gh.rs` runs the
//! commands, `fetch.rs` carries the result, this module decides what it
//! means — so every rule here is unit-tested without running `gh`.

use serde::Deserialize;

/// A release, as `gh release list --json ...` returns it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub tag_name: String,
    /// The release's title; GitHub shows the tag when it is empty.
    #[serde(default)]
    pub name: String,
    /// `null` for a draft, which has not been published yet.
    #[serde(default)]
    pub published_at: Option<String>,
    /// GitHub's own "Latest" badge: at most one release of a repo has it.
    #[serde(default)]
    pub is_latest: bool,
    #[serde(default)]
    pub is_draft: bool,
    #[serde(default)]
    pub is_prerelease: bool,
    /// The release's page, as GitHub gives it (`gh release view`; `gh
    /// release list` has no such field). Empty when we have none.
    #[serde(default)]
    pub url: String,
}

/// A repo's latest release: the one GitHub badges "Latest", otherwise the
/// most recently published one that is not a draft — a repo that only ever
/// shipped pre-releases has no "Latest", and still has something to show.
/// `None` when nothing was ever published.
pub fn latest(releases: Vec<Release>) -> Option<Release> {
    // `into_iter`: the list is ours, so the winner is moved out, not cloned.
    let mut shipped: Vec<Release> = releases.into_iter().filter(|r| !r.is_draft).collect();
    if let Some(i) = shipped.iter().position(|r| r.is_latest) {
        return Some(shipped.swap_remove(i));
    }
    // ISO 8601 timestamps sort as text, as the issues' do.
    shipped
        .into_iter()
        .max_by(|a, b| a.published_at.cmp(&b.published_at))
}

/// One row of the tab: a repo of the folder and its latest release. A repo
/// without any release keeps its row: seeing which repos never shipped is
/// half the point of the tab.
#[derive(Debug, Clone, PartialEq)]
pub struct RepoRelease {
    /// The folder, stamped by the loader as `Pr::repo` is.
    pub repo: String,
    /// The repo's releases page, for a row with no release page of its
    /// own (see `ReleaseSource::releases_page`). `None` without a usable
    /// `origin`.
    pub releases_page: Option<String>,
    pub release: Option<Release>,
    /// Commits on the default branch since the release's tag, counted by
    /// the local `git` (see `gh::unreleased_commits`). `None` when there is
    /// no release, or when this clone cannot tell.
    pub unreleased: Option<u64>,
}

impl RepoRelease {
    /// When the latest release went out, if there is one.
    pub fn published_at(&self) -> Option<&str> {
        self.release.as_ref()?.published_at.as_deref()
    }

    /// The page `enter` opens: the release's own, as GitHub gave it, or
    /// else the repo's releases page. Never built from the tag: a tag may
    /// hold characters a URL (or, on Windows, `cmd`) would read otherwise.
    pub fn url(&self) -> Option<String> {
        match &self.release {
            Some(release) if !release.url.is_empty() => Some(release.url.clone()),
            _ => self.releases_page.clone(),
        }
    }
}

/// Where the tab reads a folder's releases: the repo its `origin` points
/// to, on its real host (an ssh alias already resolved).
#[derive(Debug, Clone, PartialEq)]
pub struct ReleaseSource {
    /// `github.com` or a GitHub Enterprise host.
    pub host: String,
    /// `owner/name`.
    pub repo: String,
}

impl ReleaseSource {
    /// What to pass `gh -R`: `owner/name` on github.com, `gh`'s default
    /// host. Elsewhere `None`: `-R owner/name` would name that repo on
    /// github.com — someone else's — and `-R host/owner/name` would send
    /// `gh` to a host it may know nothing of (a GitLab origin). Without
    /// `-R`, `gh` picks among the hosts it is logged into, or fails before
    /// any request.
    pub fn gh_repo(&self) -> Option<String> {
        (self.host == "github.com").then(|| self.repo.clone())
    }

    /// The repo's releases page, for a row without a release.
    pub fn releases_page(&self) -> String {
        format!("https://{}/{}/releases", self.host, self.repo)
    }
}

/// The Releases tab's state, as `IssuesTab` holds the Issues tab's: `app.rs`
/// only wires it. No filters: one row per repo is already a short list.
#[derive(Debug, Default)]
pub struct ReleasesTab {
    pub rows: Vec<RepoRelease>,
    /// Loaded at least once: the auto-refresh keeps it fresh from then on.
    pub loaded: bool,
    /// A load is in flight, apart from `App::loading` as for the issues.
    pub loading: bool,
    /// A reload asked for while one was in flight.
    pub pending: bool,
}

impl ReleasesTab {
    /// Stores a load: the latest release first, the repos that never
    /// released last, by name.
    pub fn apply_load(&mut self, mut rows: Vec<RepoRelease>) {
        // `None` sorts before any `Some`: comparing b to a puts the newest
        // date first and the never-released repos after every dated one.
        rows.sort_by(|a, b| {
            b.published_at()
                .cmp(&a.published_at())
                .then_with(|| a.repo.cmp(&b.repo))
        });
        self.rows = rows;
        self.loaded = true;
        self.loading = false;
    }

    /// How many repos have a release at all, for the status line.
    pub fn released_count(&self) -> usize {
        self.rows.iter().filter(|row| row.release.is_some()).count()
    }
}

/// A release for the tests of every module.
#[cfg(test)]
pub fn sample_release(tag: &str, published_at: &str) -> Release {
    Release {
        tag_name: tag.to_string(),
        name: format!("Release {tag}"),
        published_at: Some(published_at.to_string()),
        is_latest: false,
        is_draft: false,
        is_prerelease: false,
        url: String::new(),
    }
}

/// A row of the `acme/<repo>` repo, released at `published_at` (as tag
/// `v1.0.0`) or never, for the tests of every module.
#[cfg(test)]
pub fn sample_row(repo: &str, published_at: Option<&str>) -> RepoRelease {
    RepoRelease {
        repo: repo.to_string(),
        releases_page: Some(format!("https://github.com/acme/{repo}/releases")),
        release: published_at.map(|at| Release {
            url: format!("https://github.com/acme/{repo}/releases/tag/v1.0.0"),
            ..sample_release("v1.0.0", at)
        }),
        unreleased: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_parses_from_gh_json() {
        let json = r#"{
            "tagName": "v1.4.0", "name": "Faster sync",
            "publishedAt": "2026-09-20T10:00:00Z", "createdAt": "2026-09-20T09:00:00Z",
            "isLatest": true, "isDraft": false, "isPrerelease": false, "isImmutable": false
        }"#;
        let release: Release = serde_json::from_str(json).unwrap();
        assert_eq!(release.tag_name, "v1.4.0");
        assert_eq!(release.name, "Faster sync");
        assert_eq!(
            release.published_at.as_deref(),
            Some("2026-09-20T10:00:00Z")
        );
        assert!(release.is_latest);
    }

    #[test]
    fn a_draft_without_a_publish_date_still_parses() {
        let json = r#"{"tagName": "v2.0.0", "name": "", "publishedAt": null, "isDraft": true}"#;
        let release: Release = serde_json::from_str(json).unwrap();
        assert_eq!(release.published_at, None);
        assert!(release.is_draft);
    }

    #[test]
    fn the_latest_badge_wins_over_a_newer_pre_release() {
        let mut stable = sample_release("v1.4.0", "2026-09-01T00:00:00Z");
        stable.is_latest = true;
        let mut rc = sample_release("v2.0.0-rc.1", "2026-09-20T00:00:00Z");
        rc.is_prerelease = true;

        let picked = latest(vec![rc, stable.clone()]);
        assert_eq!(picked, Some(stable));
    }

    #[test]
    fn without_a_badge_the_most_recent_publication_wins() {
        let old = sample_release("v0.1.0", "2026-01-01T00:00:00Z");
        let new = sample_release("v0.2.0-beta", "2026-03-01T00:00:00Z");

        let picked = latest(vec![old, new.clone()]);
        assert_eq!(picked, Some(new));
    }

    /// A draft is not shipped, even when it is the newest entry.
    #[test]
    fn a_draft_is_never_the_latest() {
        let shipped = sample_release("v1.0.0", "2026-01-01T00:00:00Z");
        let mut draft = sample_release("v1.1.0", "2026-09-01T00:00:00Z");
        draft.is_draft = true;
        draft.is_latest = true; // whatever GitHub says

        assert_eq!(latest(vec![draft.clone(), shipped.clone()]), Some(shipped));
        assert_eq!(latest(vec![draft]), None);
    }

    #[test]
    fn a_repo_without_releases_has_no_latest() {
        assert_eq!(latest(Vec::new()), None);
    }

    fn row(releases_page: Option<&str>, release: Option<Release>) -> RepoRelease {
        RepoRelease {
            repo: "api".to_string(),
            releases_page: releases_page.map(str::to_string),
            release,
            unreleased: None,
        }
    }

    /// The page GitHub gives for the release: nothing built from the tag,
    /// which may hold characters a URL (or a shell) would read otherwise.
    #[test]
    fn enter_opens_the_page_github_gives_for_the_release() {
        let mut release = sample_release("v1&calc", "2026-09-01T00:00:00Z");
        release.url = "https://github.com/acme/api/releases/tag/v1%26calc".to_string();
        let r = row(Some("https://github.com/acme/api/releases"), Some(release));
        assert_eq!(
            r.url().as_deref(),
            Some("https://github.com/acme/api/releases/tag/v1%26calc")
        );
    }

    #[test]
    fn enter_on_a_repo_without_release_opens_its_releases_page() {
        assert_eq!(
            row(Some("https://github.com/acme/api/releases"), None)
                .url()
                .as_deref(),
            Some("https://github.com/acme/api/releases")
        );
    }

    /// A release whose page GitHub did not give (its second `gh release
    /// view` failed) opens the releases page rather than nothing.
    #[test]
    fn a_release_without_its_page_opens_the_releases_page() {
        let r = row(
            Some("https://github.com/acme/api/releases"),
            Some(sample_release("v1.4.0", "2026-09-01T00:00:00Z")),
        );
        assert_eq!(
            r.url().as_deref(),
            Some("https://github.com/acme/api/releases")
        );
    }

    /// `-R owner/name` on github.com only: anywhere else, `gh` would look
    /// for that `owner/name` on github.com — someone else's repo — or ask
    /// a host it knows nothing of (a GitLab origin). Without `-R`, `gh`
    /// picks among the hosts it is logged into, or fails without a request.
    #[test]
    fn only_a_github_com_source_is_named_to_gh() {
        let github = ReleaseSource {
            host: "github.com".to_string(),
            repo: "acme/api".to_string(),
        };
        let elsewhere = ReleaseSource {
            host: "gitlab.com".to_string(),
            repo: "acme/api".to_string(),
        };
        assert_eq!(github.gh_repo().as_deref(), Some("acme/api"));
        assert_eq!(elsewhere.gh_repo(), None);
        assert_eq!(
            github.releases_page(),
            "https://github.com/acme/api/releases"
        );
    }

    #[test]
    fn a_load_lists_the_newest_release_first_and_the_unreleased_repos_last() {
        let mut tab = ReleasesTab {
            loading: true,
            ..Default::default()
        };
        tab.apply_load(vec![
            sample_row("web", None),
            sample_row("api", Some("2026-01-01T00:00:00Z")),
            sample_row("cli", Some("2026-09-01T00:00:00Z")),
            sample_row("docs", None),
        ]);

        let repos: Vec<&str> = tab.rows.iter().map(|r| r.repo.as_str()).collect();
        assert_eq!(repos, ["cli", "api", "docs", "web"]);
        assert!(tab.loaded);
        assert!(!tab.loading);
    }

    #[test]
    fn the_count_is_of_the_repos_that_have_a_release() {
        let mut tab = ReleasesTab::default();
        tab.apply_load(vec![
            sample_row("api", Some("2026-01-01T00:00:00Z")),
            sample_row("web", None),
        ]);
        assert_eq!(tab.released_count(), 1);
    }

    #[test]
    fn without_a_releases_page_nor_a_release_page_there_is_nothing_to_open() {
        assert_eq!(row(None, None).url(), None);
        let r = row(None, Some(sample_release("v1", "2026-01-01T00:00:00Z")));
        assert_eq!(r.url(), None);
    }
}
