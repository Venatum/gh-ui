//! The Actions tab's own filters. Unlike the PR filters they never reach
//! `gh`: they narrow the runs already in memory, so changing one costs no
//! HTTP request and shows instantly. They are deliberately not persisted —
//! a `status:failed` left over from yesterday would greet the next launch
//! with an empty tab and no explanation.

use crate::model::Run;
use std::collections::{HashMap, HashSet};

/// The outcome a run must have to stay visible.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum RunStatus {
    #[default]
    All,
    Failed,
    Running,
    Success,
}

impl RunStatus {
    /// The label shown in the header and in the filter panel.
    pub fn label(self) -> &'static str {
        match self {
            RunStatus::All => "all",
            RunStatus::Failed => "failed",
            RunStatus::Running => "running",
            RunStatus::Success => "success",
        }
    }

    /// The next value, wrapping back to `All`.
    pub fn next(self) -> Self {
        match self {
            RunStatus::All => RunStatus::Failed,
            RunStatus::Failed => RunStatus::Running,
            RunStatus::Running => RunStatus::Success,
            RunStatus::Success => RunStatus::All,
        }
    }

    /// The previous value, wrapping the other way.
    pub fn prev(self) -> Self {
        match self {
            RunStatus::All => RunStatus::Success,
            RunStatus::Failed => RunStatus::All,
            RunStatus::Running => RunStatus::Failed,
            RunStatus::Success => RunStatus::Running,
        }
    }

    /// Does `run` belong to this status? `Running` reads `status` because an
    /// unfinished run has no conclusion yet; the other two read `conclusion`,
    /// which only a finished run carries.
    pub fn matches(self, run: &Run) -> bool {
        match self {
            RunStatus::All => true,
            RunStatus::Failed => matches!(
                run.conclusion.as_str(),
                "failure" | "timed_out" | "startup_failure"
            ),
            RunStatus::Running => run.status != "completed",
            RunStatus::Success => run.conclusion == "success",
        }
    }
}

/// Everything the Actions tab narrows its list with, except the `/` search.
/// `only_pr_runs` sits here rather than on `App` because it is the same kind
/// of thing as the other three: a view filter, applied in memory, forgotten
/// between runs.
#[derive(Debug, Clone, PartialEq)]
pub struct RunFilters {
    /// Keep only the runs sitting on a branch that carries one of my PRs.
    pub only_pr_runs: bool,
    pub status: RunStatus,
    /// `None` = every event. `Some(e)` = only the runs triggered by `e`.
    pub event: Option<String>,
    /// `None` = every workflow. `Some(w)` = only that workflow's runs.
    pub workflow: Option<String>,
}

impl Default for RunFilters {
    /// The tab opens on "my PRs, whatever happened to them" — the behaviour
    /// gh-ui has always had.
    fn default() -> Self {
        Self {
            only_pr_runs: true,
            status: RunStatus::All,
            event: None,
            workflow: None,
        }
    }
}

impl RunFilters {
    pub fn toggle_only_pr_runs(&mut self) {
        self.only_pr_runs = !self.only_pr_runs;
    }

    pub fn cycle_status(&mut self, forward: bool) {
        self.status = if forward {
            self.status.next()
        } else {
            self.status.prev()
        };
    }

    pub fn cycle_event(&mut self, values: &[String], forward: bool) {
        self.event = cycle_value(&self.event, values, forward);
    }

    pub fn cycle_workflow(&mut self, values: &[String], forward: bool) {
        self.workflow = cycle_value(&self.workflow, values, forward);
    }

    /// The header line for the Actions tab. The branch toggle always shows —
    /// it is the tab's headline and the `m` key acts on it — while the other
    /// three only appear once they narrow something, so the default line stays
    /// as short as it has always been.
    pub fn summary(&self) -> String {
        let mut parts = vec![format!(
            "my PRs: {}",
            if self.only_pr_runs { "[x]" } else { "[ ]" }
        )];
        if self.status != RunStatus::All {
            parts.push(format!("status:{}", self.status.label()));
        }
        if let Some(e) = &self.event {
            parts.push(format!("event:{e}"));
        }
        if let Some(w) = &self.workflow {
            parts.push(format!("workflow:{w}"));
        }
        parts.join(" · ")
    }
}

/// Walks `all → values[0] → … → all`, or the other way round. A value that
/// has disappeared from `values` — the runs reloaded and no longer carry it —
/// counts as "not found" and drops back to `all` rather than trapping the
/// cycle on something that cannot be reached again.
fn cycle_value(current: &Option<String>, values: &[String], forward: bool) -> Option<String> {
    if values.is_empty() {
        return None;
    }
    match current {
        None if forward => Some(values[0].clone()),
        None => Some(values[values.len() - 1].clone()),
        Some(current) => match values.iter().position(|v| v == current) {
            Some(i) if forward && i + 1 < values.len() => Some(values[i + 1].clone()),
            Some(i) if !forward && i > 0 => Some(values[i - 1].clone()),
            // the end of the cycle, or a value that is no longer offered
            _ => None,
        },
    }
}

/// The distinct events present in `runs`, sorted. Cycling over what is
/// actually there beats a hard-coded list: no `event:schedule` offered in a
/// repo that has no cron.
pub fn events_of(runs: &[Run]) -> Vec<String> {
    distinct(runs.iter().map(|r| r.event.as_str()))
}

/// The distinct workflow names present in `runs`, sorted.
pub fn workflows_of(runs: &[Run]) -> Vec<String> {
    distinct(runs.iter().map(|r| r.workflow_name.as_str()))
}

fn distinct<'a>(values: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut out: Vec<String> = values.map(str::to_string).collect();
    out.sort();
    out.dedup();
    out
}

/// The runs the Actions tab shows, before the `/` search.
///
/// The order matters. `only_pr_runs` narrows first, then the three value
/// filters, and only then does the per-repo cap apply. Capping earlier would
/// hide what the user just asked for: `fetch_runs` pulls a hundred runs per
/// repo but the unfiltered view only ever showed the first `cap` of them, so
/// a failure sitting at position 30 would vanish under `status:failed` while
/// the tab claimed there was nothing to see.
///
/// The cap itself only applies when `only_pr_runs` is off — the PR
/// cross-reference is already narrow enough to show whole.
pub fn keep<'a>(
    runs: &'a [Run],
    pr_branches: &HashSet<&str>,
    filters: &RunFilters,
    cap: usize,
) -> Vec<&'a Run> {
    let narrowed: Vec<&Run> = runs
        .iter()
        .filter(|r| !filters.only_pr_runs || pr_branches.contains(r.head_branch.as_str()))
        .filter(|r| filters.status.matches(r))
        .filter(|r| filters.event.as_deref().is_none_or(|e| r.event == e))
        .filter(|r| {
            filters
                .workflow
                .as_deref()
                .is_none_or(|w| r.workflow_name == w)
        })
        .collect();

    if filters.only_pr_runs {
        narrowed
    } else {
        most_recent_per_repo(narrowed, cap)
    }
}

/// The first `cap` runs of each repo. `runs` arrives grouped by repo and
/// ordered most recent first inside a repo (see `fetch::load_runs`), so taking
/// the first ones is taking the most recent ones.
fn most_recent_per_repo(runs: Vec<&Run>, cap: usize) -> Vec<&Run> {
    let mut kept: HashMap<&str, usize> = HashMap::new();
    runs.into_iter()
        .filter(|r| {
            let n = kept.entry(r.repo.as_str()).or_insert(0);
            *n += 1;
            *n <= cap
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A run with the given `status`/`conclusion`; the rest is filler.
    fn run(status: &str, conclusion: &str) -> Run {
        Run {
            workflow_name: "CI".into(),
            display_title: "t".into(),
            head_branch: "b".into(),
            status: status.into(),
            conclusion: conclusion.into(),
            event: "push".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            number: 1,
            url: "u".into(),
            repo: "r".into(),
        }
    }

    /// A successful run carrying the given workflow and event.
    fn run_of(workflow: &str, event: &str) -> Run {
        Run {
            workflow_name: workflow.into(),
            event: event.into(),
            ..run("completed", "success")
        }
    }

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    /// A successful run of `repo`, on `branch`.
    fn run_in(repo: &str, branch: &str) -> Run {
        Run {
            repo: repo.into(),
            head_branch: branch.into(),
            ..run("completed", "success")
        }
    }

    fn branches<'a>(values: &[&'a str]) -> HashSet<&'a str> {
        values.iter().copied().collect()
    }

    /// The whole tab, wide open.
    fn everything() -> RunFilters {
        RunFilters {
            only_pr_runs: false,
            ..RunFilters::default()
        }
    }

    #[test]
    fn all_keeps_every_run() {
        for r in [
            run("completed", "success"),
            run("completed", "failure"),
            run("in_progress", ""),
            run("completed", "cancelled"),
        ] {
            assert!(RunStatus::All.matches(&r));
        }
    }

    /// The three conclusions GitHub uses for "this run did not work" — a
    /// cancelled or skipped run is neither a failure nor a success.
    #[test]
    fn failed_covers_every_broken_conclusion() {
        for conclusion in ["failure", "timed_out", "startup_failure"] {
            assert!(
                RunStatus::Failed.matches(&run("completed", conclusion)),
                "{conclusion} should count as failed"
            );
        }
        for conclusion in ["success", "cancelled", "skipped", "neutral"] {
            assert!(
                !RunStatus::Failed.matches(&run("completed", conclusion)),
                "{conclusion} should not count as failed"
            );
        }
    }

    /// "Running" is read off `status`, not `conclusion`: an unfinished run has
    /// no conclusion at all, so the conclusion field cannot answer this.
    #[test]
    fn running_is_anything_not_completed() {
        for status in ["queued", "in_progress", "waiting", "requested"] {
            assert!(
                RunStatus::Running.matches(&run(status, "")),
                "{status} should count as running"
            );
        }
        assert!(!RunStatus::Running.matches(&run("completed", "success")));
        assert!(!RunStatus::Running.matches(&run("completed", "failure")));
    }

    #[test]
    fn success_is_only_the_successful_conclusion() {
        assert!(RunStatus::Success.matches(&run("completed", "success")));
        assert!(!RunStatus::Success.matches(&run("completed", "skipped")));
        assert!(!RunStatus::Success.matches(&run("in_progress", "")));
    }

    #[test]
    fn next_walks_every_status_then_wraps_to_all() {
        let mut status = RunStatus::All;
        let mut seen = Vec::new();
        for _ in 0..4 {
            status = status.next();
            seen.push(status);
        }
        assert_eq!(
            seen,
            vec![
                RunStatus::Failed,
                RunStatus::Running,
                RunStatus::Success,
                RunStatus::All, // a full tour brings us back to the start
            ]
        );
    }

    #[test]
    fn prev_undoes_next() {
        let mut status = RunStatus::All;
        for _ in 0..4 {
            assert_eq!(status.next().prev(), status);
            status = status.next();
        }
    }

    #[test]
    fn the_tab_opens_on_my_prs_with_nothing_else_narrowed() {
        let f = RunFilters::default();
        assert!(f.only_pr_runs);
        assert_eq!(f.status, RunStatus::All);
        assert_eq!(f.event, None);
        assert_eq!(f.workflow, None);
    }

    #[test]
    fn values_are_deduplicated_and_sorted() {
        let runs = vec![
            run_of("Deploy", "schedule"),
            run_of("CI", "push"),
            run_of("CI", "pull_request"),
            run_of("CI", "push"),
        ];
        assert_eq!(workflows_of(&runs), strings(&["CI", "Deploy"]));
        assert_eq!(
            events_of(&runs),
            strings(&["pull_request", "push", "schedule"])
        );
    }

    #[test]
    fn cycling_an_event_walks_the_values_then_returns_to_all() {
        let values = strings(&["pull_request", "push"]);
        let mut f = RunFilters::default();

        f.cycle_event(&values, true);
        assert_eq!(f.event.as_deref(), Some("pull_request"));
        f.cycle_event(&values, true);
        assert_eq!(f.event.as_deref(), Some("push"));
        f.cycle_event(&values, true);
        assert_eq!(f.event, None, "the last value should wrap back to all");
    }

    #[test]
    fn cycling_backwards_starts_from_the_last_value() {
        let values = strings(&["pull_request", "push"]);
        let mut f = RunFilters::default();

        f.cycle_workflow(&values, false);
        assert_eq!(f.workflow.as_deref(), Some("push"));
        f.cycle_workflow(&values, false);
        assert_eq!(f.workflow.as_deref(), Some("pull_request"));
        f.cycle_workflow(&values, false);
        assert_eq!(f.workflow, None);
    }

    /// The values come from the runs in memory, so a refresh can retire the
    /// one we are on. Cycling must then lead back out, not stay stuck.
    #[test]
    fn a_value_that_vanished_falls_back_to_all() {
        let mut f = RunFilters {
            workflow: Some("Deploy".into()),
            ..RunFilters::default()
        };
        f.cycle_workflow(&strings(&["CI"]), true);
        assert_eq!(f.workflow, None);
    }

    /// Nothing loaded yet: cycling must be a no-op rather than a panic on an
    /// empty slice.
    #[test]
    fn cycling_with_no_values_stays_on_all() {
        let mut f = RunFilters::default();
        f.cycle_event(&[], true);
        assert_eq!(f.event, None);
        f.cycle_event(&[], false);
        assert_eq!(f.event, None);
    }

    #[test]
    fn the_default_summary_is_just_the_branch_toggle() {
        assert_eq!(RunFilters::default().summary(), "my PRs: [x]");
        assert_eq!(everything().summary(), "my PRs: [ ]");
    }

    #[test]
    fn the_summary_only_names_the_filters_that_narrow_something() {
        let f = RunFilters {
            status: RunStatus::Failed,
            workflow: Some("CI".into()),
            ..everything()
        };
        // No `event:` chip: that one is still on "all".
        assert_eq!(f.summary(), "my PRs: [ ] · status:failed · workflow:CI");
    }

    // --- the pipeline ---

    #[test]
    fn only_pr_runs_keeps_the_branches_that_carry_a_pr() {
        let runs = vec![
            run_in("r", "feature/x"),
            run_in("r", "main"),
            run_in("r", "feature/y"),
        ];
        let prs = branches(&["feature/x", "feature/y"]);

        let kept = keep(&runs, &prs, &RunFilters::default(), 20);
        assert_eq!(kept.len(), 2);
        assert!(kept.iter().all(|r| r.head_branch != "main"));

        // Off: every branch comes back.
        assert_eq!(keep(&runs, &prs, &everything(), 20).len(), 3);
    }

    #[test]
    fn the_unfiltered_view_keeps_the_most_recent_runs_of_each_repo() {
        // `fetch_runs` pulls a wide window so the "my PRs" cross-reference has
        // something to work with. With the filter off the user still wants the
        // short list he has always had: the most recent runs of each repo.
        let mut runs: Vec<Run> = (0..30).map(|i| run_in("alpha", &format!("b{i}"))).collect();
        runs.extend((0..5).map(|i| run_in("beta", &format!("c{i}"))));

        let kept = keep(&runs, &branches(&[]), &everything(), 20);

        assert_eq!(kept.iter().filter(|r| r.repo == "alpha").count(), 20);
        // A repo with fewer runs than the limit keeps all of them.
        assert_eq!(kept.iter().filter(|r| r.repo == "beta").count(), 5);
        // And the ones kept are the most recent, i.e. the first `gh` returned.
        assert_eq!(kept[0].head_branch, "b0");
        assert_eq!(kept[19].head_branch, "b19");
    }

    #[test]
    fn pr_runs_survive_a_base_branch_that_floods_the_window() {
        // The bug: a busy `develop` used to fill the whole fetch window, so the
        // cross-reference found nothing and the tab looked empty. The wider
        // window must reach the PR runs sitting behind that flood — and the
        // display limit must not cut them off again.
        let mut runs: Vec<Run> = (0..40).map(|_| run_in("alpha", "develop")).collect();
        runs.push(run_in("alpha", "feature/x"));

        let kept = keep(&runs, &branches(&["feature/x"]), &RunFilters::default(), 20);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].head_branch, "feature/x");
    }

    /// The reason the pipeline has the order it has. The only failure sits
    /// past the per-repo cap: filtering after the cap would report an empty
    /// tab while the run the user asked for was right there in memory.
    #[test]
    fn a_failure_past_the_cap_still_shows_under_status_failed() {
        let mut runs: Vec<Run> = (0..5).map(|i| run_in("alpha", &format!("ok{i}"))).collect();
        runs.push(Run {
            conclusion: "failure".into(),
            ..run_in("alpha", "broken")
        });

        let filters = RunFilters {
            status: RunStatus::Failed,
            ..everything()
        };
        let kept = keep(&runs, &branches(&[]), &filters, 3);

        assert_eq!(kept.len(), 1, "the cap must apply after the status filter");
        assert_eq!(kept[0].head_branch, "broken");
    }

    /// The cap still does its job once the filters have had their say.
    #[test]
    fn the_cap_applies_to_what_the_filters_left() {
        let runs: Vec<Run> = (0..10).map(|i| run_in("alpha", &format!("b{i}"))).collect();
        assert_eq!(keep(&runs, &branches(&[]), &everything(), 3).len(), 3);
    }

    #[test]
    fn event_and_workflow_narrow_independently() {
        let runs = vec![
            run_of("CI", "push"),
            run_of("CI", "schedule"),
            run_of("Deploy", "push"),
        ];
        let no_prs = branches(&[]);

        let by_event = RunFilters {
            event: Some("push".into()),
            ..everything()
        };
        assert_eq!(keep(&runs, &no_prs, &by_event, 20).len(), 2);

        let by_workflow = RunFilters {
            workflow: Some("CI".into()),
            ..everything()
        };
        assert_eq!(keep(&runs, &no_prs, &by_workflow, 20).len(), 2);

        // Together they are an AND, not an OR.
        let both = RunFilters {
            event: Some("push".into()),
            workflow: Some("CI".into()),
            ..everything()
        };
        assert_eq!(keep(&runs, &no_prs, &both, 20).len(), 1);
    }
}
