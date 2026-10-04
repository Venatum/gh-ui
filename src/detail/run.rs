//! A workflow run's detail (`enter` on the Actions tab): what `gh run
//! view` returns, and the lines of its two sections, Overview and Jobs.

use super::{cut_end, elapsed_label, fact_label, local_time};
use crate::model::null_as_empty;
use chrono::{DateTime, FixedOffset, Utc};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use serde::Deserialize;

/// One run as `gh run view --json …` returns it: only the fields the view
/// shows (see `gh::RUN_VIEW_FIELDS`). Unlike a PR's checks, `gh` spells
/// a run's states in lower case: "completed", "in_progress", "success"…
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDetail {
    pub workflow_name: String,
    /// The commit's or the PR's title: what the view's border shows.
    pub display_title: String,
    pub number: u64,
    /// 1 for the first run, 2 after a re-run…
    #[serde(default)]
    pub attempt: u64,
    /// "queued" | "in_progress" | "completed"…
    pub status: String,
    /// "success" | "failure" | "cancelled"…; empty while it runs.
    #[serde(default, deserialize_with = "null_as_empty")]
    pub conclusion: String,
    #[serde(default)]
    pub event: String,
    #[serde(default)]
    pub head_branch: String,
    #[serde(default)]
    pub head_sha: String,
    pub url: String,
    #[serde(default)]
    pub started_at: Option<String>,
    /// When it last changed: its end, once it is over.
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub jobs: Vec<Job>,
}

/// One job of the run, and its steps.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub name: String,
    pub status: String,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub conclusion: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub completed_at: Option<String>,
    #[serde(default)]
    pub steps: Vec<Step>,
}

/// One step of a job.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    pub name: String,
    pub status: String,
    #[serde(default, deserialize_with = "null_as_empty")]
    pub conclusion: String,
    #[serde(default)]
    pub started_at: Option<String>,
    #[serde(default)]
    pub completed_at: Option<String>,
}

/// Where a run, a job or a step stands, from its status and conclusion.
/// The order is the Jobs section's: failures first, then what still runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Standing {
    Failed,
    Running,
    Queued,
    Passed,
    Cancelled,
    Skipped,
}

fn standing(status: &str, conclusion: &str) -> Standing {
    match (status, conclusion) {
        ("in_progress", _) => Standing::Running,
        ("completed", "success") => Standing::Passed,
        ("completed", "skipped" | "neutral") => Standing::Skipped,
        ("completed", "cancelled") => Standing::Cancelled,
        ("completed", _) => Standing::Failed,
        _ => Standing::Queued,
    }
}

/// The glyph, the word and the color of a standing, as the Actions tab
/// and the PR's Checks section draw them.
fn look(standing: Standing) -> (&'static str, &'static str, Color) {
    match standing {
        Standing::Failed => ("✗", "failure", Color::Red),
        Standing::Running => ("●", "running", Color::Yellow),
        Standing::Queued => ("●", "queued", Color::Yellow),
        Standing::Passed => ("✓", "success", Color::Green),
        Standing::Cancelled => ("-", "cancelled", Color::DarkGray),
        Standing::Skipped => ("·", "skipped", Color::DarkGray),
    }
}

/// The Overview section: one line per question — how it ended, what
/// started it, when and for how long, how its jobs went. A run has no
/// description: its title is on the view's border.
pub fn overview_lines(
    d: &RunDetail,
    now: DateTime<Utc>,
    offset: FixedOffset,
) -> Vec<Line<'static>> {
    let run = standing(&d.status, &d.conclusion);
    let (glyph, word, color) = look(run);
    let mut headline = format!(" · {} · run #{}", d.workflow_name, d.number);
    if d.attempt > 1 {
        headline.push_str(&format!(" · attempt {}", d.attempt));
    }
    let sha: String = d.head_sha.chars().take(7).collect();

    let running = matches!(run, Standing::Running | Standing::Queued);
    let started = match d.started_at.as_deref() {
        Some(at) => local_time(at, offset),
        None => "-".to_string(),
    };
    let elapsed = elapsed_label(
        d.started_at.as_deref(),
        d.updated_at.as_deref(),
        running,
        now,
    );
    let when = match (running, elapsed.is_empty()) {
        (_, true) => started,
        (true, false) => format!("{started} · running {elapsed}"),
        (false, false) => format!("{started} · took {elapsed}"),
    };

    vec![
        Line::from(vec![
            Span::styled(format!("{glyph} {word}"), Style::new().fg(color)),
            Span::raw(headline),
        ]),
        Line::from(vec![
            fact_label("on"),
            Span::raw(format!("{} · {} @ {sha}", d.event, d.head_branch)),
        ]),
        Line::from(vec![fact_label("started"), Span::raw(when)]),
        jobs_summary(&d.jobs),
    ]
}

/// The Overview's `jobs` fact: how many in each standing, the empty ones
/// left out.
fn jobs_summary(jobs: &[Job]) -> Line<'static> {
    let count = |wanted: &[Standing]| {
        jobs.iter()
            .filter(|job| wanted.contains(&standing(&job.status, &job.conclusion)))
            .count()
    };
    let parts: Vec<Span<'static>> = [
        (count(&[Standing::Failed]), "✗ ", "failed", Color::Red),
        (
            count(&[Standing::Running, Standing::Queued]),
            "● ",
            "running",
            Color::Yellow,
        ),
        (count(&[Standing::Passed]), "✓ ", "passed", Color::Green),
        (
            count(&[Standing::Cancelled]),
            "",
            "cancelled",
            Color::DarkGray,
        ),
        (count(&[Standing::Skipped]), "", "skipped", Color::DarkGray),
    ]
    .into_iter()
    .filter(|(n, ..)| *n > 0)
    .map(|(n, glyph, word, color)| {
        Span::styled(format!("{glyph}{n} {word}"), Style::new().fg(color))
    })
    .collect();

    let mut spans = vec![fact_label("jobs")];
    if parts.is_empty() {
        spans.push(Span::styled("none", Style::new().fg(Color::DarkGray)));
    }
    for (i, part) in parts.into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" · "));
        }
        spans.push(part);
    }
    Line::from(spans)
}

/// One row of the Jobs section before it is laid out: what goes left of
/// the name (a job's `✗ failure`, a step's indented glyph), the name, and
/// the time it took.
struct Row {
    prefix: Span<'static>,
    name: String,
    time: String,
}

/// The Jobs section: failures first, then what still runs, then the rest
/// in GitHub's order. A failed, running or queued job shows its steps
/// underneath — the step that broke is the reason to open the view; the
/// others stay one line until `show_folded` (`space`). The times line up
/// whatever the names' lengths, within `width`.
pub fn jobs_lines(
    d: &RunDetail,
    now: DateTime<Utc>,
    width: u16,
    show_folded: bool,
) -> Vec<Line<'static>> {
    if d.jobs.is_empty() {
        return vec![Line::styled(
            "No jobs in this run.",
            Style::new().fg(Color::DarkGray),
        )];
    }
    let mut jobs: Vec<&Job> = d.jobs.iter().collect();
    // Stable: jobs of one standing keep GitHub's order.
    jobs.sort_by_key(|job| standing(&job.status, &job.conclusion));

    let mut rows = Vec::new();
    for job in jobs {
        let job_standing = standing(&job.status, &job.conclusion);
        let (glyph, word, color) = look(job_standing);
        rows.push(Row {
            prefix: Span::styled(format!("{glyph} {word:<9} "), Style::new().fg(color)),
            name: job.name.clone(),
            time: time_of(
                job_standing,
                job.started_at.as_deref(),
                job.completed_at.as_deref(),
                now,
            ),
        });
        let open = matches!(
            job_standing,
            Standing::Failed | Standing::Running | Standing::Queued
        );
        if !(open || show_folded) {
            continue;
        }
        for step in &job.steps {
            let step_standing = standing(&step.status, &step.conclusion);
            let (glyph, _, color) = look(step_standing);
            rows.push(Row {
                prefix: Span::styled(format!("    {glyph} "), Style::new().fg(color)),
                name: step.name.clone(),
                time: time_of(
                    step_standing,
                    step.started_at.as_deref(),
                    step.completed_at.as_deref(),
                    now,
                ),
            });
        }
    }

    // The names end at one column, so the times line up: as far as the
    // longest row reaches, but leaving ` for 99m` room within `width`.
    let reach = rows
        .iter()
        .map(|row| row.prefix.width() + row.name.chars().count())
        .max()
        .unwrap_or(0)
        .min(usize::from(width).saturating_sub(8));
    let mut lines: Vec<Line<'static>> = rows
        .into_iter()
        .map(|row| {
            let room = reach.saturating_sub(row.prefix.width()).max(1);
            let text = if row.time.is_empty() {
                cut_end(&row.name, room)
            } else {
                format!("{:<room$} {:>6}", cut_end(&row.name, room), row.time)
            };
            Line::from(vec![row.prefix, Span::raw(text)])
        })
        .collect();

    let jobs = if d.jobs.len() == 1 { "job" } else { "jobs" };
    lines.push(Line::default());
    lines.push(Line::styled(
        format!(
            "{} {jobs} · space shows every job's steps · enter opens the run on GitHub",
            d.jobs.len()
        ),
        Style::new().fg(Color::DarkGray),
    ));
    lines
}

/// A job's or a step's time: how long it took, or for how long it runs;
/// nothing for one skipped or not started.
fn time_of(
    standing: Standing,
    started: Option<&str>,
    finished: Option<&str>,
    now: DateTime<Utc>,
) -> String {
    match standing {
        Standing::Skipped | Standing::Queued => String::new(),
        Standing::Running => elapsed_label(started, finished, true, now),
        _ => elapsed_label(started, finished, false, now),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detail::{Detail, Section};
    use chrono::TimeZone;

    /// A failed second attempt of CI on a PR's branch: a job that failed
    /// at its test step, one still running, one green, one skipped.
    fn sample() -> RunDetail {
        serde_json::from_str(
            r#"{"workflowName": "CI", "displayTitle": "Add rate limiting", "number": 812,
                "attempt": 2, "status": "completed", "conclusion": "failure",
                "event": "pull_request", "headBranch": "feat/rate-limit",
                "headSha": "1a2b3c4d5e6f", "url": "https://github.com/acme/api/actions/runs/99",
                "createdAt": "2026-10-04T10:12:00Z", "startedAt": "2026-10-04T10:12:00Z",
                "updatedAt": "2026-10-04T10:18:40Z",
                "jobs": [
                    {"name": "build", "status": "completed", "conclusion": "success",
                     "startedAt": "2026-10-04T10:12:05Z", "completedAt": "2026-10-04T10:13:15Z",
                     "steps": [{"name": "Set up job", "number": 1, "status": "completed",
                                "conclusion": "success", "startedAt": "2026-10-04T10:12:05Z",
                                "completedAt": "2026-10-04T10:12:06Z"}]},
                    {"name": "test (ubuntu-latest)", "status": "completed", "conclusion": "failure",
                     "startedAt": "2026-10-04T10:12:05Z", "completedAt": "2026-10-04T10:16:17Z",
                     "steps": [
                        {"name": "Set up job", "number": 1, "status": "completed",
                         "conclusion": "success", "startedAt": "2026-10-04T10:12:05Z",
                         "completedAt": "2026-10-04T10:12:06Z"},
                        {"name": "Run cargo test", "number": 2, "status": "completed",
                         "conclusion": "failure", "startedAt": "2026-10-04T10:12:06Z",
                         "completedAt": "2026-10-04T10:16:04Z"}]},
                    {"name": "deploy", "status": "completed", "conclusion": "skipped",
                     "startedAt": "2026-10-04T10:16:20Z", "completedAt": "2026-10-04T10:16:20Z",
                     "steps": []},
                    {"name": "e2e", "status": "in_progress", "conclusion": "",
                     "startedAt": "2026-10-04T10:14:00Z", "completedAt": "0001-01-01T00:00:00Z",
                     "steps": [{"name": "Run playwright", "number": 1, "status": "in_progress",
                                "conclusion": null, "startedAt": "2026-10-04T10:14:00Z",
                                "completedAt": null}]}
                ]}"#,
        )
        .unwrap()
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 4, 10, 20, 0).unwrap()
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.to_string().trim_end().to_string())
            .collect()
    }

    #[test]
    fn the_facts_say_verdict_trigger_time_and_jobs() {
        let lines = texts(&overview_lines(&sample(), now(), utc()));
        assert_eq!(lines[0], "✗ failure · CI · run #812 · attempt 2");
        assert_eq!(
            lines[1],
            "on       pull_request · feat/rate-limit @ 1a2b3c4"
        );
        assert_eq!(lines[2], "started  2026-10-04 10:12 · took 6m40s");
        assert_eq!(
            lines[3],
            "jobs     ✗ 1 failed · ● 1 running · ✓ 1 passed · 1 skipped"
        );
    }

    /// A first attempt does not say "attempt 1"; a run still going says
    /// for how long.
    #[test]
    fn a_running_first_attempt_says_for_how_long() {
        let mut d = sample();
        d.attempt = 1;
        d.status = "in_progress".into();
        d.conclusion = String::new();
        let lines = texts(&overview_lines(&d, now(), utc()));
        assert_eq!(lines[0], "● running · CI · run #812");
        assert_eq!(lines[2], "started  2026-10-04 10:12 · running for 8m");
    }

    /// Failures first, then what still runs, then the rest in their order.
    /// A failed or running job shows its steps; a finished green or skipped
    /// one stays one line until `space`.
    #[test]
    fn jobs_list_failures_first_with_their_steps() {
        let lines = texts(&jobs_lines(&sample(), now(), 80, false));
        assert_eq!(
            lines,
            [
                "✗ failure   test (ubuntu-latest)  4m12s",
                "    ✓ Set up job                  0m01s",
                "    ✗ Run cargo test              3m58s",
                "● running   e2e                  for 6m",
                "    ● Run playwright             for 6m",
                "✓ success   build                 1m10s",
                "· skipped   deploy",
                "",
                "4 jobs · space shows every job's steps · enter opens the run on GitHub",
            ]
        );
    }

    #[test]
    fn space_shows_the_steps_of_every_job() {
        let lines = texts(&jobs_lines(&sample(), now(), 80, true));
        let build = lines.iter().position(|l| l.contains("build")).unwrap();
        assert_eq!(lines[build + 1], "    ✓ Set up job                  0m01s");
    }

    #[test]
    fn a_run_without_jobs_says_so() {
        let mut d = sample();
        d.jobs.clear();
        assert_eq!(
            texts(&jobs_lines(&d, now(), 80, false)),
            ["No jobs in this run."]
        );
    }

    /// The section bar counts the jobs; `enter` opens the run, from either
    /// section.
    #[test]
    fn the_view_counts_the_jobs_and_opens_the_run() {
        let detail = Detail::Run(sample());
        assert_eq!(detail.count(Section::Jobs), Some(4));
        assert_eq!(detail.count(Section::Overview), None);
        assert_eq!(
            detail.url(Section::Jobs),
            "https://github.com/acme/api/actions/runs/99"
        );
        assert_eq!(detail.title(), "Add rate limiting");
    }
}
