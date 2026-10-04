//! The app's data structures: the shape of a PR as `gh` returns it in JSON,
//! plus the small nested types (author, label, check).

use serde::Deserialize;

// `#[derive(...)]` asks the compiler to generate code for us.
//   - `Deserialize`: serde will know how to build this type FROM JSON.
//   - `Debug`      : we'll be able to print it with `{:?}` (handy for debugging).
#[derive(Debug, Deserialize)]
pub struct Author {
    pub login: String,
}

#[derive(Debug, Deserialize)]
pub struct Label {
    pub name: String,
    /// Hex without the `#`, e.g. "d73a4a". Defaulted so a label written
    /// without one (in the tests) still parses.
    #[serde(default)]
    pub color: String,
}

/// One entry of a PR's `statusCheckRollup`. GitHub puts two shapes in the
/// same array: a `CheckRun` (GitHub Actions and the other check apps:
/// `status` + `conclusion`) and a `StatusContext` (the older commit-status
/// API: `state` alone). Rather than an untagged enum, all three fields are
/// defaulted and we read whichever the entry carries; the ones we do not
/// need (name, detailsUrl, timestamps...) are simply ignored by serde.
#[derive(Debug, Deserialize)]
pub struct Check {
    /// CheckRun: "QUEUED" | "IN_PROGRESS" | "COMPLETED"... ; "" on a
    /// StatusContext, which has no such field.
    #[serde(default)]
    pub status: String,
    /// CheckRun: "SUCCESS" | "FAILURE" | "SKIPPED"... ; "" while it runs.
    #[serde(default)]
    pub conclusion: String,
    /// StatusContext: "SUCCESS" | "FAILURE" | "ERROR" | "PENDING" |
    /// "EXPECTED" ; "" on a CheckRun.
    #[serde(default)]
    pub state: String,
}

impl Check {
    /// A check that failed for a reason worth reporting. CANCELLED, SKIPPED,
    /// NEUTRAL and STALE are deliberately NOT failures.
    fn is_failing(&self) -> bool {
        matches!(
            self.conclusion.as_str(),
            "FAILURE" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE"
        ) || matches!(self.state.as_str(), "FAILURE" | "ERROR")
    }

    /// A check that has not finished yet.
    fn is_running(&self) -> bool {
        matches!(
            self.status.as_str(),
            "QUEUED" | "IN_PROGRESS" | "WAITING" | "PENDING" | "REQUESTED"
        ) || matches!(self.state.as_str(), "PENDING" | "EXPECTED")
    }
}

/// What a PR's checks add up to, once the whole rollup is reduced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChecksState {
    /// No check at all: a repo without CI, or a head commit with no status.
    None,
    Passing,
    Running,
    Failing,
}

// The `gh` JSON uses camelCase (reviewDecision, isDraft...).
// `rename_all = "camelCase"` tells serde to map it to our snake_case fields
// (review_decision, is_draft...) automatically.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pr {
    pub number: u64,
    pub title: String,
    pub author: Author,

    // `gh` returns "" (empty string) when there is no decision yet.
    // `#[serde(default)]` = if the field is missing from the JSON, use the
    // default value (here an empty String) instead of crashing.
    #[serde(default)]
    pub review_decision: String,

    pub is_draft: bool,
    pub url: String,
    pub updated_at: String,
    pub additions: u64,
    pub deletions: u64,

    // `Vec<Label>` = a list (dynamic array) of labels.
    pub labels: Vec<Label>,

    // The PR's source branch (e.g. "feature/x"). Used to link a PR to its
    // GitHub Actions runs (we cross-reference on this branch in the Actions tab).
    pub head_ref_name: String,

    /// "MERGEABLE" | "CONFLICTING" | "UNKNOWN". GitHub computes it lazily,
    /// so "UNKNOWN" is common right after a push: it must never read as a
    /// conflict.
    #[serde(default)]
    pub mergeable: String,

    /// The PR's checks. An `Option` so that both a missing key and a JSON
    /// `null` (no status on the head commit) degrade to "no checks" instead
    /// of failing the whole parse.
    #[serde(default)]
    pub status_check_rollup: Option<Vec<Check>>,

    // This field is NOT in gh's JSON: we fill it ourselves with the name of the
    // directory/repo the PR comes from. `#[serde(skip)]` = serde ignores it at
    // parsing and gives it its default value ("").
    #[serde(skip)]
    pub repo: String,
}

impl Pr {
    /// Reduces the whole rollup to one verdict. Failing beats running beats
    /// passing, so one failed job among twenty running ones still reads as
    /// a failure.
    pub fn checks_state(&self) -> ChecksState {
        let checks = self.status_check_rollup.as_deref().unwrap_or_default();
        if checks.is_empty() {
            ChecksState::None
        } else if checks.iter().any(Check::is_failing) {
            ChecksState::Failing
        } else if checks.iter().any(Check::is_running) {
            ChecksState::Running
        } else {
            ChecksState::Passing
        }
    }
}

/// A GitHub Actions run, as `gh run list --json ...` returns it.
/// Same style as `Pr`: `repo` is filled in by us (not in the JSON).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub workflow_name: String,
    pub display_title: String,
    pub head_branch: String,
    /// "queued" | "in_progress" | "completed".
    pub status: String,
    /// "success" | "failure" | "cancelled" | "skipped"... ; "" if not finished.
    #[serde(default)]
    pub conclusion: String,
    /// "push" | "pull_request" | "schedule"...
    pub event: String,
    pub created_at: String,
    pub number: u64,
    pub url: String,
    #[serde(skip)]
    pub repo: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pr_deserializes_head_ref_name() {
        let json = r#"{
            "number": 1, "title": "t", "author": {"login": "moi"},
            "isDraft": false, "url": "u", "updatedAt": "2026-01-01T00:00:00Z",
            "additions": 0, "deletions": 0, "labels": [],
            "headRefName": "feature/x"
        }"#;
        let pr: Pr = serde_json::from_str(json).unwrap();
        assert_eq!(pr.head_ref_name, "feature/x");
    }

    #[test]
    fn run_deserializes_and_defaults_conclusion() {
        // "conclusion" is absent as long as the run is not finished.
        let json = r#"{
            "workflowName": "CI", "displayTitle": "fixes a bug",
            "headBranch": "feature/x", "status": "in_progress",
            "event": "push", "createdAt": "2026-01-01T00:00:00Z",
            "number": 42, "url": "u"
        }"#;
        let run: Run = serde_json::from_str(json).unwrap();
        assert_eq!(run.workflow_name, "CI");
        assert_eq!(run.head_branch, "feature/x");
        assert_eq!(run.conclusion, ""); // default because absent
    }

    /// A `Pr` built from JSON with `extra` spliced in as its last field, so
    /// each test only spells out the part it is about.
    fn pr_with(extra: &str) -> Pr {
        let json = format!(
            r#"{{
                "number": 1, "title": "t", "author": {{"login": "moi"}},
                "isDraft": false, "url": "u", "updatedAt": "2026-01-01T00:00:00Z",
                "additions": 0, "deletions": 0, "labels": [],
                "headRefName": "feature/x", {extra}
            }}"#
        );
        serde_json::from_str(&json).unwrap()
    }

    /// Every JSON literal of the other tests omits the two fields: they
    /// must default rather than fail the whole parse.
    #[test]
    fn a_pr_without_merge_nor_check_fields_still_parses() {
        let json = r#"{
            "number": 1, "title": "t", "author": {"login": "moi"},
            "isDraft": false, "url": "u", "updatedAt": "2026-01-01T00:00:00Z",
            "additions": 0, "deletions": 0, "labels": [],
            "headRefName": "feature/x"
        }"#;
        let pr: Pr = serde_json::from_str(json).unwrap();

        assert_eq!(pr.mergeable, "");
        assert_eq!(pr.checks_state(), ChecksState::None);
    }

    /// `gh` sends null when the head commit carries no status at all.
    #[test]
    fn a_null_rollup_is_read_as_no_checks() {
        let pr = pr_with(r#""statusCheckRollup": null"#);

        assert_eq!(pr.checks_state(), ChecksState::None);
    }

    #[test]
    fn one_failed_check_outweighs_the_ones_still_running() {
        let pr = pr_with(
            r#""statusCheckRollup": [
                {"status": "IN_PROGRESS", "conclusion": ""},
                {"status": "COMPLETED", "conclusion": "FAILURE"}
            ]"#,
        );

        assert_eq!(pr.checks_state(), ChecksState::Failing);
    }

    #[test]
    fn a_check_still_running_outweighs_the_ones_that_passed() {
        let pr = pr_with(
            r#""statusCheckRollup": [
                {"status": "COMPLETED", "conclusion": "SUCCESS"},
                {"status": "QUEUED", "conclusion": ""}
            ]"#,
        );

        assert_eq!(pr.checks_state(), ChecksState::Running);
    }

    /// A job skipped by a path filter is a normal outcome: painting it red
    /// would light up the whole table.
    #[test]
    fn skipped_cancelled_and_neutral_checks_are_not_failures() {
        let pr = pr_with(
            r#""statusCheckRollup": [
                {"status": "COMPLETED", "conclusion": "SKIPPED"},
                {"status": "COMPLETED", "conclusion": "CANCELLED"},
                {"status": "COMPLETED", "conclusion": "NEUTRAL"},
                {"status": "COMPLETED", "conclusion": "SUCCESS"}
            ]"#,
        );

        assert_eq!(pr.checks_state(), ChecksState::Passing);
    }

    /// The same array mixes CheckRun entries (status + conclusion) with
    /// StatusContext entries from the older commit-status API, which carry
    /// only `state`.
    #[test]
    fn a_legacy_status_context_is_read_from_its_state() {
        let failing = pr_with(r#""statusCheckRollup": [{"state": "ERROR"}]"#);
        let pending = pr_with(r#""statusCheckRollup": [{"state": "PENDING"}]"#);
        let green = pr_with(r#""statusCheckRollup": [{"state": "SUCCESS"}]"#);

        assert_eq!(failing.checks_state(), ChecksState::Failing);
        assert_eq!(pending.checks_state(), ChecksState::Running);
        assert_eq!(green.checks_state(), ChecksState::Passing);
    }

    #[test]
    fn mergeable_is_read_verbatim() {
        let pr = pr_with(r#""mergeable": "CONFLICTING""#);

        assert_eq!(pr.mergeable, "CONFLICTING");
    }
}
