//! The rendering: turns the state (`App`) into ratatui widgets. This module
//! decides nothing, it only draws what `App` holds.

use crate::app::{App, FILTER_FIELDS, FilterField, InputKind, Tab};
use crate::filters::Filters;
use crate::model::{Pr, Run};
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Clear, Paragraph, Row, Table};

/// Width (in columns) of the centered overlays.
const FILTER_PANEL_WIDTH: u16 = 52;
const HELP_WIDTH: u16 = 60;

pub fn render(frame: &mut Frame, app: &mut App) {
    let areas = Layout::vertical([
        Constraint::Length(5), // header: border + 3 lines (status + tabs + subtitle)
        Constraint::Min(0),
        Constraint::Length(1), // footer
    ])
    .split(frame.area());

    render_header(frame, app, areas[0]);
    render_table(frame, app, areas[1]);
    render_footer(frame, app, areas[2]);

    // Overlays, drawn on top of the rest.
    if app.filter_panel_open {
        render_filter_panel(frame, app, frame.area());
    }
    if app.show_help {
        render_help(frame, frame.area());
    }
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    // Line 1: name + spinner (if loading) + status + auto-refresh state.
    let mut top = vec![
        Span::styled("gh-ui", Style::new().bold().fg(Color::Cyan)),
        Span::raw("  —  "),
    ];
    if app.loading {
        const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let c = FRAMES[app.spinner_frame % FRAMES.len()];
        top.push(Span::styled(
            format!("{c} "),
            Style::new().fg(Color::Yellow),
        ));
    }
    top.push(Span::raw(app.status.clone()));
    if app.auto_refresh {
        top.push(Span::styled("   ⟳ auto 60s", Style::new().fg(Color::Green)));
    }

    // Line 2: the tabs, [ PRs ] [ Actions ], the active one highlighted.
    let tab_span = |label: &str, active: bool| {
        if active {
            Span::styled(
                format!(" {label} "),
                Style::new().fg(Color::Black).bg(Color::Cyan).bold(),
            )
        } else {
            Span::styled(format!(" {label} "), Style::new().fg(Color::DarkGray))
        }
    };
    let tabs = Line::from(vec![
        tab_span("PRs", app.active_tab == Tab::Prs),
        Span::raw(" "),
        tab_span("Actions", app.active_tab == Tab::Runs),
    ]);

    // Line 3: depending on the tab, PRs filters summary OR the runs toggle state.
    let subtitle = match app.active_tab {
        Tab::Prs => Span::styled(app.filters.summary(), Style::new().fg(Color::DarkGray)),
        Tab::Runs => Span::styled(
            format!(
                "my PRs: {}   (m to toggle)",
                if app.only_pr_runs { "[x]" } else { "[ ]" }
            ),
            Style::new().fg(Color::DarkGray),
        ),
    };

    let header =
        Paragraph::new(vec![Line::from(top), tabs, Line::from(subtitle)]).block(Block::bordered());
    frame.render_widget(header, area);
}

fn render_table(frame: &mut Frame, app: &mut App, area: Rect) {
    match app.active_tab {
        Tab::Prs => render_pr_table(frame, app, area),
        Tab::Runs => render_run_table(frame, app, area),
    }
}

fn render_pr_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let header = Row::new([
        "repo", "updated", "#", "title", "author", "review", "+/-", "labels",
    ])
    .style(Style::new().bold());

    let rows: Vec<Row> = app.prs.iter().map(pr_to_row).collect();

    let widths = [
        Constraint::Length(16),
        Constraint::Length(10),
        Constraint::Length(7),
        Constraint::Fill(2), // title: elastic, 2 shares of the rest
        Constraint::Length(20),
        Constraint::Length(9),
        Constraint::Length(11),
        Constraint::Fill(1), // labels: elastic, 1 share of the rest
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::new().reversed())
        .highlight_symbol("▌ ")
        .block(Block::bordered());

    frame.render_stateful_widget(table, area, &mut app.table_state);
}

fn pr_to_row(pr: &Pr) -> Row<'_> {
    let date = pr.updated_at.get(..10).unwrap_or("").to_string();
    let (review_label, review_style) = review_look(&pr.review_decision);

    // Title: prefixed with "[D]" and grayed out if it's a draft.
    let title = if pr.is_draft {
        Cell::from(format!("[D] {}", pr.title)).style(Style::new().fg(Color::DarkGray))
    } else {
        Cell::from(pr.title.clone())
    };

    // Joined labels: "#bug #api" (grayed), truncated to the column width.
    let labels = pr
        .labels
        .iter()
        .map(|l| format!("#{}", l.name))
        .collect::<Vec<_>>()
        .join(" ");

    let diff = Line::from(vec![
        Span::styled(format!("+{}", pr.additions), Style::new().fg(Color::Green)),
        Span::raw("/"),
        Span::styled(format!("-{}", pr.deletions), Style::new().fg(Color::Red)),
    ]);

    Row::new(vec![
        Cell::from(pr.repo.clone()),
        Cell::from(date),
        Cell::from(format!("#{}", pr.number)),
        title,
        Cell::from(format!("@{}", pr.author.login)),
        Cell::from(Span::styled(review_label, review_style)),
        Cell::from(diff),
        Cell::from(Span::styled(labels, Style::new().fg(Color::DarkGray))),
    ])
}

fn render_run_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let header = Row::new([
        "repo", "created", "workflow", "branch", "event", "status", "title",
    ])
    .style(Style::new().bold());

    // `rows` must NOT borrow `app` (see run_to_row -> Row<'static>), otherwise the
    // `&mut app.run_table_state` below would conflict with it.
    let rows: Vec<Row<'static>> = app.visible_runs().into_iter().map(run_to_row).collect();

    let widths = [
        Constraint::Length(16),
        Constraint::Length(10),
        Constraint::Length(18),
        Constraint::Length(22),
        Constraint::Length(14),
        Constraint::Length(11),
        Constraint::Fill(1),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::new().reversed())
        .highlight_symbol("▌ ")
        .block(Block::bordered());

    frame.render_stateful_widget(table, area, &mut app.run_table_state);
}

/// Builds a run's row. Returns a `Row<'static>`: every cell owns its `String`,
/// so the row does not keep borrowing the `Run` it was built from.
fn run_to_row(run: &Run) -> Row<'static> {
    let date = run.created_at.get(..10).unwrap_or("").to_string();
    let (status_label, status_style) = run_look(&run.status, &run.conclusion);

    Row::new(vec![
        Cell::from(run.repo.clone()),
        Cell::from(date),
        Cell::from(run.workflow_name.clone()),
        Cell::from(run.head_branch.clone()),
        Cell::from(run.event.clone()),
        Cell::from(Span::styled(status_label, status_style)),
        Cell::from(run.display_title.clone()),
    ])
}

/// Label + color of a run based on (status, conclusion).
fn run_look(status: &str, conclusion: &str) -> (&'static str, Style) {
    match (status, conclusion) {
        ("completed", "success") => ("✓ success", Style::new().fg(Color::Green)),
        ("completed", "failure") => ("✗ failure", Style::new().fg(Color::Red)),
        ("completed", "cancelled") => ("cancelled", Style::new().fg(Color::DarkGray)),
        ("completed", "skipped") => ("skipped", Style::new().fg(Color::DarkGray)),
        ("in_progress", _) => ("● running", Style::new().fg(Color::Yellow)),
        ("queued", _) => ("queued", Style::new().fg(Color::Gray)),
        _ => ("-", Style::new().fg(Color::DarkGray)),
    }
}

fn review_look(decision: &str) -> (&'static str, Style) {
    match decision {
        "APPROVED" => ("approved", Style::new().fg(Color::Green)),
        "CHANGES_REQUESTED" => ("changes", Style::new().fg(Color::Red)),
        "REVIEW_REQUIRED" => ("review", Style::new().fg(Color::Yellow)),
        _ => ("-", Style::new().fg(Color::DarkGray)),
    }
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    // In input mode, the footer becomes a text prompt.
    if let Some(kind) = app.input_kind {
        let prompt = match kind {
            InputKind::Author => "author",
            InputKind::Label => "label(s)",
        };
        let line = Line::from(vec![
            Span::styled(
                format!(" {prompt} > "),
                Style::new().fg(Color::Black).bg(Color::Yellow),
            ),
            Span::raw(format!(" {}", app.input_buffer)),
            Span::styled("▏", Style::new().fg(Color::Yellow)), // cursor
            Span::styled(
                "   (enter: confirm · esc: cancel)",
                Style::new().fg(Color::DarkGray),
            ),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    // Otherwise, a compact footer; the full detail is in the help (?).
    let hints = [
        ("↑↓", "nav"),
        ("f", "filters"),
        ("r", "refresh"),
        ("a", "auto"),
        ("enter", "open"),
        ("?", "help"),
        ("q", "quit"),
    ];
    let mut spans = Vec::new();
    for (key, label) in hints {
        spans.push(Span::styled(
            format!(" {key} "),
            Style::new().fg(Color::Black).bg(Color::Cyan),
        ));
        spans.push(Span::raw(format!(" {label}   ")));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The navigable filter panel, centered over the interface.
fn render_filter_panel(frame: &mut Frame, app: &App, area: Rect) {
    let f = &app.filters;

    // One line per filter; the focused line is highlighted.
    let mut lines: Vec<Line> = FILTER_FIELDS
        .iter()
        .enumerate()
        .map(|(i, &field)| {
            let focused = i == app.filter_cursor;
            let marker = if focused { "▸ " } else { "  " };
            let text = format!(
                "{marker}{:<11} {}",
                field_name(field),
                field_value(field, f)
            );
            if focused {
                Line::from(Span::styled(
                    text,
                    Style::new().fg(Color::Black).bg(Color::Cyan),
                ))
            } else {
                Line::from(Span::raw(text))
            }
        })
        .collect();

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " ↑↓ navigate · ←→ change · ⏎ edit · esc close",
        Style::new().fg(Color::DarkGray),
    )));

    let popup = centered_rect(FILTER_PANEL_WIDTH, lines.len() as u16 + 2, area);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Filters ")),
        popup,
    );
}

/// The displayed name of a filter field.
fn field_name(field: FilterField) -> &'static str {
    match field {
        FilterField::Mode => "Mode",
        FilterField::Since => "Since",
        FilterField::NoDraft => "No draft",
        FilterField::Unreviewed => "Unreviewed",
        FilterField::NotMine => "Not mine",
        FilterField::Repo => "Repo",
        FilterField::Author => "Author",
        FilterField::Label => "Label",
    }
}

/// The displayed value of a field: cycle "◂ x ▸", box "[x]", or text.
fn field_value(field: FilterField, f: &Filters) -> String {
    let empty = "(empty)  ⏎ edit";
    match field {
        FilterField::Mode => format!("◂ {} ▸", f.filter.label()),
        FilterField::Since => format!("◂ {} ▸", f.since.label()),
        FilterField::NoDraft => toggle_box(f.no_draft),
        FilterField::Unreviewed => toggle_box(f.unreviewed),
        FilterField::NotMine => toggle_box(f.not_mine),
        FilterField::Repo => format!("◂ {} ▸", f.repo.as_deref().unwrap_or("all")),
        FilterField::Author => match f.author.as_deref() {
            Some(a) if !a.is_empty() => a.to_string(),
            _ => empty.to_string(),
        },
        FilterField::Label => {
            if f.labels.is_empty() {
                empty.to_string()
            } else {
                f.labels.join(" ")
            }
        }
    }
}

fn toggle_box(on: bool) -> String {
    if on {
        "[x]".to_string()
    } else {
        "[ ]".to_string()
    }
}

/// The help screen, centered over the interface.
fn render_help(frame: &mut Frame, area: Rect) {
    let lines = vec![
        Line::from(Span::styled(
            "gh-ui — shortcuts",
            Style::new().bold().fg(Color::Cyan),
        )),
        Line::from(""),
        help_row("↑/↓, j/k", "navigate the list"),
        help_row("enter", "open the PR in the browser"),
        help_row("r", "reload now"),
        help_row("a", "toggle auto-refresh (60s)"),
        help_row("f", "open the filter panel"),
        help_row("q", "quit"),
        Line::from(""),
        Line::from(Span::styled("  In the filter panel", Style::new().bold())),
        help_row("↑/↓", "choose a filter"),
        help_row("←/→", "change its value (cycles, boxes)"),
        help_row("enter", "edit author/label · esc to close"),
        Line::from(""),
        Line::from(Span::styled(
            "  Filters are remembered between runs.",
            Style::new().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "  (any key to close)",
            Style::new().fg(Color::DarkGray),
        )),
    ];

    let popup = centered_rect(HELP_WIDTH, lines.len() as u16 + 2, area);
    frame.render_widget(Clear, popup); // clears the area under the popup
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Help ")),
        popup,
    );
}

/// A help row: "  key   description".
fn help_row(key: &'static str, desc: &'static str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("  {key:<10}"), Style::new().fg(Color::Yellow)),
        Span::raw(desc),
    ])
}

/// Computes a centered rectangle of `width` columns and `height` rows.
fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let [area] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    let [area] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(area);
    area
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_look_colors() {
        assert_eq!(run_look("completed", "success").0, "✓ success");
        assert_eq!(run_look("completed", "failure").0, "✗ failure");
        assert_eq!(run_look("completed", "cancelled").0, "cancelled");
        assert_eq!(run_look("in_progress", "").0, "● running");
        assert_eq!(run_look("queued", "").0, "queued");
    }
}
