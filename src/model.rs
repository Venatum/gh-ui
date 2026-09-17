//! The app's data structures: the shape of a PR as `gh` returns it in JSON,
//! plus the small nested types (author, label).

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

    // This field is NOT in gh's JSON: we fill it ourselves with the name of the
    // directory/repo the PR comes from. `#[serde(skip)]` = serde ignores it at
    // parsing and gives it its default value ("").
    #[serde(skip)]
    pub repo: String,
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
}
