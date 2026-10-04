//! An issue's detail (`enter` on the Issues tab): what `gh issue view`
//! returns, and the lines of its two sections, Overview and Comments.

use super::{Comment, comment_entry, entries_lines, fact_label, facts_then_body, local_time};
use crate::columns::label_spans;
use crate::issues::{PrRef, pr_ref_label};
use crate::model::{Author, Label};
use chrono::FixedOffset;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use serde::Deserialize;

/// One issue as `gh issue view --json …` returns it: only the fields the
/// view shows (see `gh::ISSUE_VIEW_FIELDS`). Everything but the title,
/// the URL, the state and the author may be missing, and defaults.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueDetail {
    pub title: String,
    pub url: String,
    /// Markdown, as typed; empty when the author wrote nothing.
    #[serde(default)]
    pub body: String,
    /// "OPEN" | "CLOSED".
    pub state: String,
    /// Why it was closed: "COMPLETED" | "NOT_PLANNED" | "DUPLICATE"…;
    /// `None` (`null`) while it is open.
    #[serde(default)]
    pub state_reason: Option<String>,
    pub author: Author,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub labels: Vec<Label>,
    #[serde(default)]
    pub assignees: Vec<Author>,
    /// The PRs that close it, as the Issues tab's PRs column reads them.
    #[serde(default, rename = "closedByPullRequestsReferences")]
    pub linked_prs: Vec<PrRef>,
    #[serde(default)]
    pub comments: Vec<Comment>,
    /// The folder, stamped by the loader as `Issue::repo` is: a linked PR
    /// of the same repo reads `#412`, one of another `owner/name#12`.
    #[serde(skip)]
    pub repo: String,
}

/// The Overview section: the facts, a rule `width` wide, the description.
/// `offset` is the user's time zone, for the opening date.
pub fn overview_lines(d: &IssueDetail, width: u16, offset: FixedOffset) -> Vec<Line<'static>> {
    facts_then_body(
        facts_lines(d, offset),
        &d.body,
        width,
        "No description provided.",
    )
}

/// One line per question: in what state and since when, who is on it,
/// which PRs close it, what it is labelled.
fn facts_lines(d: &IssueDetail, offset: FixedOffset) -> Vec<Line<'static>> {
    let gray = Style::new().fg(Color::DarkGray);
    let (state, color) = state_look(d);
    let headline = Line::from(vec![
        Span::styled("● ", Style::new().fg(color)),
        Span::styled(state, Style::new().fg(color)),
        Span::raw(format!(
            " · @{} · opened {}",
            d.author.login,
            local_time(&d.created_at, offset)
        )),
    ]);

    let assigned = if d.assignees.is_empty() {
        Span::styled("nobody", gray)
    } else {
        let logins: Vec<String> = d
            .assignees
            .iter()
            .map(|a| format!("@{}", a.login))
            .collect();
        Span::raw(logins.join(", "))
    };
    let linked = if d.linked_prs.is_empty() {
        Span::styled("no PR", gray)
    } else {
        let refs: Vec<String> = d
            .linked_prs
            .iter()
            .map(|pr| pr_ref_label(pr, &d.repo))
            .collect();
        Span::styled(refs.join(" · "), Style::new().fg(Color::Green))
    };
    let mut lines = vec![
        headline,
        Line::from(vec![fact_label("assigned"), assigned]),
        Line::from(vec![fact_label("linked"), linked]),
    ];
    if !d.labels.is_empty() {
        let mut spans = vec![fact_label("labels")];
        spans.extend(label_spans(&d.labels));
        lines.push(Line::from(spans));
    }
    lines
}

/// The words for the issue's state, and their color: GitHub's own. A
/// closed issue says why, as GitHub's icon does.
fn state_look(d: &IssueDetail) -> (String, Color) {
    if d.state != "CLOSED" {
        return ("open".to_string(), Color::Green);
    }
    match d.state_reason.as_deref() {
        Some("NOT_PLANNED") => ("closed · not planned".to_string(), Color::DarkGray),
        Some("DUPLICATE") => ("closed · duplicate".to_string(), Color::DarkGray),
        Some("COMPLETED") => ("closed · completed".to_string(), Color::Magenta),
        _ => ("closed".to_string(), Color::Magenta),
    }
}

/// The Comments section: the conversation, as a PR's reads.
pub fn comments_lines(d: &IssueDetail, offset: FixedOffset) -> Vec<Line<'static>> {
    let entries: Vec<_> = d.comments.iter().map(comment_entry).collect();
    // Nothing folds in an issue's conversation: no reviews there.
    entries_lines(&entries, offset, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detail::{Detail, Section};

    /// An open issue as `gh issue view --json …` returns it, in the `api`
    /// folder: two assignees, a PR of the same repo and one of another.
    fn sample() -> IssueDetail {
        let mut d: IssueDetail = serde_json::from_str(
            r#"{"title": "Rate limit the public API", "url": "https://github.com/acme/api/issues/57",
                "body": "Every route can be hammered.\r\n\r\n<!-- triage: p1 -->",
                "state": "OPEN", "stateReason": null,
                "author": {"login": "carol"}, "createdAt": "2026-09-28T09:15:00Z",
                "labels": [{"name": "bug", "color": "d73a4a"}],
                "assignees": [{"login": "alice"}, {"login": "Venatum"}],
                "closedByPullRequestsReferences": [
                    {"number": 412, "repository": {"name": "api", "owner": {"login": "acme"}}},
                    {"number": 12, "repository": {"name": "web", "owner": {"login": "acme"}}}
                ],
                "comments": [
                    {"author": {"login": "bob"}, "body": "Per key or per org?",
                     "createdAt": "2026-09-29T10:00:00Z"}
                ]}"#,
        )
        .unwrap();
        d.repo = "api".to_string();
        d
    }

    fn utc() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn the_facts_say_state_author_date_assignees_linked_prs_and_labels() {
        let lines = texts(&overview_lines(&sample(), 20, utc()));
        assert_eq!(lines[0], "● open · @carol · opened 2026-09-28 09:15");
        assert_eq!(lines[1], "assigned @alice, @Venatum");
        // Same repo: `#412`; another one: `acme/web#12`, as the PRs column.
        assert_eq!(lines[2], "linked   #412 · acme/web#12");
        assert_eq!(lines[3], "labels   ● bug");
        assert_eq!(lines[4], "─".repeat(20));
    }

    /// The description goes through the PRs' markdown pass: CRLF and the
    /// template's HTML comment gone.
    #[test]
    fn the_description_follows_the_facts() {
        let lines = texts(&overview_lines(&sample(), 20, utc()));
        assert_eq!(lines[6..], ["Every route can be hammered."]);
    }

    /// Nobody on it and no PR yet are facts too, said in gray words rather
    /// than left blank; no label, no labels line.
    #[test]
    fn nobody_and_no_pr_are_said() {
        let mut d = sample();
        d.assignees.clear();
        d.linked_prs.clear();
        d.labels.clear();
        let lines = texts(&overview_lines(&d, 20, utc()));
        assert_eq!(lines[1], "assigned nobody");
        assert_eq!(lines[2], "linked   no PR");
        assert_eq!(lines[3], "─".repeat(20));
    }

    /// A closed issue says why, in GitHub's words: done, or not planned.
    #[test]
    fn a_closed_issue_says_why() {
        let mut d = sample();
        d.state = "CLOSED".into();
        d.state_reason = Some("NOT_PLANNED".into());
        assert!(
            texts(&overview_lines(&d, 20, utc()))[0].starts_with("● closed · not planned · @carol")
        );
        d.state_reason = Some("COMPLETED".into());
        assert!(
            texts(&overview_lines(&d, 20, utc()))[0].starts_with("● closed · completed · @carol")
        );
    }

    #[test]
    fn the_comments_read_as_the_prs_conversation() {
        assert_eq!(
            texts(&comments_lines(&sample(), utc())),
            ["@bob · 2026-09-29 10:00", "  Per key or per org?"]
        );
        let mut d = sample();
        d.comments.clear();
        assert_eq!(texts(&comments_lines(&d, utc())), ["No comments yet."]);
    }

    /// The section bar counts the comments; `enter` opens the issue.
    #[test]
    fn the_view_counts_the_comments_and_opens_the_issue() {
        let detail = Detail::Issue(sample());
        assert_eq!(detail.count(Section::Comments), Some(1));
        assert_eq!(detail.count(Section::Overview), None);
        assert_eq!(
            detail.url(Section::Comments),
            "https://github.com/acme/api/issues/57"
        );
        assert_eq!(detail.title(), "Rate limit the public API");
    }
}
