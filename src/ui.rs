//! The rendering: turns the state (`App`) into ratatui widgets. This module
//! decides nothing, it only draws what `App` holds.

use crate::app::{App, FilterField, InputKind, Tab, section_of};
use crate::columns::{Column, ColumnLayout, PrColumn, RunColumn};
use crate::filters::Filters;
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph, Row, Table};

/// Width (in columns) of the centered overlays.
const FILTER_PANEL_WIDTH: u16 = 52;
const COLUMN_PANEL_WIDTH: u16 = 54;
const HELP_WIDTH: u16 = 64;

/// The column panel's hint, while browsing (the default mode). Named so a
/// test can build the exact same `Line` the panel renders without needing an
/// `App` (see the tests module).
const COLUMN_PANEL_HINT: &str = " ↑↓ choose · ⏎ show/hide · space grab · esc close";
/// The column panel's hint, once a column has been grabbed (key `space`):
/// `↑`/`↓` now move it instead of the cursor.
const COLUMN_PANEL_HINT_GRABBED: &str = " ↑↓ move it · space drop · esc drop";

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
    if app.column_panel_open {
        render_column_panel(frame, app, frame.area());
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
    if app.auto_refresh.interval().is_some() {
        top.push(Span::styled(
            format!("   ⟳ auto {}", app.auto_refresh.label()),
            Style::new().fg(Color::Green),
        ));
    }

    // The account, flush against the right edge of the header.
    if let Some(label) = app.login.label() {
        let account = Span::styled(label, Style::new().fg(Color::DarkGray));
        let used: usize = top.iter().map(Span::width).sum();
        if let Some(pad) = right_align_padding(area.width, used, account.width()) {
            top.push(Span::raw(" ".repeat(pad)));
            top.push(account);
        }
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

    // Line 3: depending on the tab, PRs filters summary OR the runs toggle
    // state — plus the search chip, which belongs to both.
    let mut subtitle = vec![match app.active_tab {
        Tab::Prs => Span::styled(
            format!(
                "{} · {}",
                app.filters.summary_common(),
                app.filters.summary_prs()
            ),
            Style::new().fg(Color::DarkGray),
        ),
        Tab::Runs => Span::styled(
            format!(
                "{} · my PRs: {}   (m to toggle)",
                app.filters.summary_common(),
                if app.only_pr_runs { "[x]" } else { "[ ]" }
            ),
            Style::new().fg(Color::DarkGray),
        ),
    }];

    // Counted against the tab's own list: the PRs tab compares with everything
    // fetched, the Actions tab with what its branch toggle already kept.
    let (shown, total) = match app.active_tab {
        Tab::Prs => (app.visible_prs().len(), app.prs.len()),
        Tab::Runs => (app.visible_runs().len(), app.branch_runs().len()),
    };
    if let Some(chip) = search_summary(&app.search, shown, total) {
        subtitle.push(Span::raw("  "));
        subtitle.push(Span::styled(chip, Style::new().fg(Color::Yellow)));
    }

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
    // Collected into an owned `Vec` (the columns are `Copy`): nothing keeps
    // borrowing `app` when `&mut app.table_state` is handed over below.
    let columns: Vec<PrColumn> = app.columns.prs.visible().collect();

    let header =
        Row::new(columns.iter().map(|c| c.header()).collect::<Vec<_>>()).style(Style::new().bold());
    let widths: Vec<Constraint> = columns.iter().map(|c| c.width()).collect();
    let rows: Vec<Row<'static>> = app
        .visible_prs()
        .into_iter()
        .map(|pr| Row::new(columns.iter().map(|c| c.cell(pr)).collect::<Vec<_>>()))
        .collect();

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::new().reversed())
        .highlight_symbol("▌ ")
        .block(Block::bordered());

    frame.render_stateful_widget(table, area, &mut app.table_state);
}

fn render_run_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let columns: Vec<RunColumn> = app.columns.runs.visible().collect();

    let header =
        Row::new(columns.iter().map(|c| c.header()).collect::<Vec<_>>()).style(Style::new().bold());
    let widths: Vec<Constraint> = columns.iter().map(|c| c.width()).collect();
    // `rows` must NOT borrow `app` (the cells own their `String`), otherwise the
    // `&mut app.run_table_state` below would conflict with it.
    let rows: Vec<Row<'static>> = app
        .visible_runs()
        .into_iter()
        .map(|run| Row::new(columns.iter().map(|c| c.cell(run)).collect::<Vec<_>>()))
        .collect();

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::new().reversed())
        .highlight_symbol("▌ ")
        .block(Block::bordered());

    frame.render_stateful_widget(table, area, &mut app.run_table_state);
}

/// The footer shortcuts, in the order they matter. `?` is not in the list: it
/// is appended separately and never dropped, because it is how the user
/// reaches everything the footer had to cut.
const HINTS: [(&str, &str); 8] = [
    ("↑↓", "nav"),
    ("enter", "open"),
    ("tab/1/2", "tab"),
    ("f", "filters"),
    ("c", "columns"),
    ("r", "refresh"),
    ("a", "auto"),
    ("q", "quit"),
];

/// Always rendered, always last.
const HELP_HINT: (&str, &str) = ("?", "help");

/// Columns one hint occupies: a `" key "` chip plus `" label  "`.
fn hint_width((key, label): (&str, &str)) -> usize {
    key.chars().count() + label.chars().count() + 5
}

/// The hints that fit in `width` columns, plus whether any were dropped.
///
/// The full row is 117 columns wide, so on an 80-column terminal it used to be
/// silently clipped mid-word — and what fell off the end was `enter`, `?` and
/// `q`, the three a lost user needs most. Now we drop whole hints from the
/// tail, mark the cut with `…`, and always keep `?`.
fn fitting_hints(width: usize) -> (Vec<(&'static str, &'static str)>, bool) {
    let help = hint_width(HELP_HINT);
    if HINTS.iter().copied().map(hint_width).sum::<usize>() + help <= width {
        return (HINTS.to_vec(), false);
    }

    // "… " sits between the kept hints and `?`, so it is part of the budget.
    let budget = width.saturating_sub(help + 2);
    let mut kept = Vec::new();
    let mut used = 0;
    for hint in HINTS {
        let w = hint_width(hint);
        if used + w > budget {
            break;
        }
        used += w;
        kept.push(hint);
    }
    (kept, true)
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    // In input mode, the footer becomes a text prompt.
    if let Some(kind) = app.input_kind {
        let prompt = match kind {
            InputKind::Author => "author",
            InputKind::Label => "label(s)",
            InputKind::Search => "search",
        };
        let line = Line::from(vec![
            Span::styled(
                format!(" {prompt} > "),
                Style::new().fg(Color::Black).bg(Color::Yellow),
            ),
            Span::raw(format!(" {}", app.input_buffer)),
            Span::styled("▏", Style::new().fg(Color::Yellow)), // cursor
            Span::styled(input_hint(kind), Style::new().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

    // Otherwise, a compact footer; the full detail is in the help (?).
    let (hints, cut) = fitting_hints(area.width as usize);
    let mut spans = Vec::new();
    for (key, label) in hints {
        spans.push(Span::styled(
            format!(" {key} "),
            Style::new().fg(Color::Black).bg(Color::Cyan),
        ));
        spans.push(Span::raw(format!(" {label}  ")));
    }
    if cut {
        spans.push(Span::styled("… ", Style::new().fg(Color::DarkGray)));
    }
    spans.push(Span::styled(
        format!(" {} ", HELP_HINT.0),
        Style::new().fg(Color::Black).bg(Color::Cyan),
    ));
    spans.push(Span::raw(format!(" {}", HELP_HINT.1)));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The tail of the prompt line. The search needs its own: it is applied live,
/// and esc CLEARS it rather than merely abandoning an edit.
fn input_hint(kind: InputKind) -> &'static str {
    match kind {
        InputKind::Search => "   (live · enter: keep · esc: clear)",
        InputKind::Author | InputKind::Label => "   (enter: confirm · esc: cancel)",
    }
}

/// The navigable filter panel, centered over the interface.
fn render_filter_panel(frame: &mut Frame, app: &App, area: Rect) {
    let f = &app.filters;

    // One line per filter of the ACTIVE tab, grouped under section titles.
    // The titles are not navigable: `filter_cursor` indexes the fields only.
    let mut lines: Vec<Line> = Vec::new();
    let mut section = "";
    for (i, &field) in app.active_fields().iter().enumerate() {
        if section_of(field) != section {
            section = section_of(field);
            lines.push(Line::from(Span::styled(
                format!(" {section}"),
                Style::new().bold().fg(Color::Cyan),
            )));
        }

        let focused = i == app.filter_cursor;
        let marker = if focused { "▸ " } else { "  " };
        let text = format!(
            "{marker}{:<11} {}",
            field_name(field),
            field_value(field, f, app.only_pr_runs)
        );
        lines.push(if focused {
            Line::from(Span::styled(
                text,
                Style::new().fg(Color::Black).bg(Color::Cyan),
            ))
        } else {
            Line::from(Span::raw(text))
        });
    }

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

/// The column panel: the columns of the active tab, in order, with their
/// checkbox. Top of the list = leftmost column of the table.
fn render_column_panel(frame: &mut Frame, app: &App, area: Rect) {
    let (title, mut lines) = match app.active_tab {
        Tab::Prs => (
            " Columns — PRs ",
            column_lines(&app.columns.prs, app.column_cursor, app.column_grabbed),
        ),
        Tab::Runs => (
            " Columns — Actions ",
            column_lines(&app.columns.runs, app.column_cursor, app.column_grabbed),
        ),
    };

    lines.push(Line::from(""));
    let hint = if app.column_grabbed {
        COLUMN_PANEL_HINT_GRABBED
    } else {
        COLUMN_PANEL_HINT
    };
    lines.push(Line::from(Span::styled(
        hint,
        Style::new().fg(Color::DarkGray),
    )));

    let popup = centered_rect(COLUMN_PANEL_WIDTH, lines.len() as u16 + 2, area);
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(title)),
        popup,
    );
}

/// One line per column: "▸ [x] title". Generic, so both tabs share it.
/// `grabbed`: whether the focused row is currently grabbed (key `space`) —
/// it is then indented 2 extra columns, which is the whole visual cue that
/// tells the user the row is now attached to `↑`/`↓` instead of the cursor.
fn column_lines<C: Column>(
    layout: &ColumnLayout<C>,
    cursor: usize,
    grabbed: bool,
) -> Vec<Line<'static>> {
    layout
        .entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let focused = i == cursor;
            let marker = if focused { "▸ " } else { "  " };
            // The extra indent only ever applies to the focused row: the
            // grabbed column is always the one under the cursor.
            let indent = if focused && grabbed { "  " } else { "" };
            let text = format!(
                "{indent}{marker}{} {}",
                toggle_box(entry.visible),
                entry.column.header()
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
        .collect()
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
        FilterField::OnlyPrRuns => "Only my PRs",
    }
}

/// The displayed value of a field: cycle "◂ x ▸", box "[x]", or text.
fn field_value(field: FilterField, f: &Filters, only_pr_runs: bool) -> String {
    let empty = "(empty)  ⏎ edit";
    match field {
        FilterField::OnlyPrRuns => toggle_box(only_pr_runs),
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
        help_row("a / A", "auto-refresh: off/1mn/5mn/10mn/30mn/1h · A: off"),
        help_row("f", "open the filter panel"),
        help_row("c", "open the columns panel"),
        help_row("tab, 1/2", "switch tab (PRs / Actions)"),
        help_row("m", "Actions tab: filter on my PRs"),
        help_row("q", "quit"),
        Line::from(""),
        Line::from(Span::styled("  In the filter panel", Style::new().bold())),
        help_row("↑/↓", "choose a filter"),
        help_row("←/→", "change its value (cycles, boxes)"),
        help_row("enter", "edit author/label · esc to close"),
        Line::from(""),
        Line::from(Span::styled("  In the columns panel", Style::new().bold())),
        help_row("↑/↓", "choose a column · esc to close"),
        help_row("enter", "show / hide it"),
        help_row("space", "grab it, ↑/↓ moves it · esc/space drops"),
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

/// The "search:…" chip of the header, or `None` when no search is active.
/// `shown`/`total` are deliberate: the search narrows the rows ALREADY
/// fetched (at most `PR_LIMIT` per repo), so the size of the corpus has to
/// stay on screen.
fn search_summary(query: &str, shown: usize, total: usize) -> Option<String> {
    let query = query.trim();
    (!query.is_empty()).then(|| format!("search:\"{query}\" {shown}/{total}"))
}

/// Spaces needed to push a `trailing`-wide span against the right edge of a
/// bordered header `width` columns wide, whose content already occupies
/// `used`. `None` when the two would not fit with at least one space between
/// them: the caller then drops the trailing span rather than wrap the line.
fn right_align_padding(width: u16, used: usize, trailing: usize) -> Option<usize> {
    let inner = (width as usize).checked_sub(2)?; // the block's two borders
    (inner > used + trailing).then(|| inner - used - trailing)
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
    use crate::columns::{ColumnLayout, PrColumn};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    /// Renders into an 80×24 `TestBackend` (a real terminal, no `App` and no
    /// filesystem access) and returns the buffer's content as one string, so
    /// tests can assert on the visible text regardless of styling.
    fn render_to_text(width: u16, height: u16, draw: impl FnOnce(&mut Frame)) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| draw(frame)).unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    }

    /// I2: on an 80×24 terminal (the size named in the finding), the help
    /// popup must still show its closing hint. Before the fix (24 lines, so a
    /// 26-row popup) this fails: the popup does not fit and `render_widget`
    /// silently clips it, so `(any key to close)` never reaches the buffer.
    #[test]
    fn help_popup_fits_an_80x24_terminal() {
        let text = render_to_text(80, 24, |frame| render_help(frame, frame.area()));

        assert!(
            text.contains("(any key to close)"),
            "the help popup must fit an 80x24 terminal and show its closing hint"
        );
    }

    /// Building an `App` reads the user's real `~/.config` (forbidden in
    /// tests), so the header cannot be rendered here. These cover the part
    /// that actually decides the layout instead.
    #[test]
    fn the_account_sits_flush_against_the_right_edge() {
        // 80 columns, 2 borders → 78 usable. 20 used, "@vincent" is 8 wide:
        // 50 spaces leave the last character on column 78.
        let pad = right_align_padding(80, 20, 8).expect("it fits by a mile");
        assert_eq!(pad, 50);
        assert_eq!(20 + pad + 8, 78);
    }

    #[test]
    fn the_account_is_dropped_rather_than_wrapping_the_header() {
        // Exactly one space left between the two: still fine.
        assert_eq!(right_align_padding(22, 11, 8), Some(1));
        // One column tighter and they would touch → we drop the account.
        assert_eq!(right_align_padding(22, 12, 8), None);
        // A header narrower than its own borders must not panic.
        assert_eq!(right_align_padding(1, 0, 8), None);
    }

    #[test]
    fn a_wide_terminal_keeps_every_footer_hint() {
        let (hints, cut) = fitting_hints(200);
        assert_eq!(hints.len(), HINTS.len());
        assert!(!cut);
    }

    #[test]
    fn an_80_column_footer_drops_hints_instead_of_clipping_them() {
        let (hints, cut) = fitting_hints(80);

        assert!(cut, "the full row is 117 columns, it cannot fit 80");
        // Whole hints are dropped, and what is kept is the head of the list.
        assert_eq!(hints, HINTS[..hints.len()].to_vec());

        // Everything rendered fits: the kept hints, "… ", and `?` itself.
        let rendered: usize =
            hints.iter().copied().map(hint_width).sum::<usize>() + 2 + hint_width(HELP_HINT);
        assert!(rendered <= 80, "rendered {rendered} columns in 80");
    }

    #[test]
    fn help_survives_a_terminal_too_narrow_for_anything_else() {
        // `?` is appended unconditionally, so a tiny width keeps no hint at
        // all rather than panicking on the budget subtraction.
        let (hints, cut) = fitting_hints(4);
        assert!(hints.is_empty());
        assert!(cut);
    }

    /// The auto-refresh row is the widest one in the help, and it grew when
    /// the pace became cyclable. Renders it for real so a longer label (or a
    /// narrower `HELP_WIDTH`) can never silently clip it.
    #[test]
    fn the_auto_refresh_help_row_is_not_truncated() {
        let text = render_to_text(80, 24, |frame| render_help(frame, frame.area()));

        assert!(
            text.contains("auto-refresh: off/1mn/5mn/10mn/30mn/1h \u{b7} A: off"),
            "the auto-refresh help row must fit HELP_WIDTH without being cut"
        );
    }

    /// The chip must state BOTH the query and how much of the fetched list it
    /// is hiding: the search is client-side, so `3/57` is what tells the user
    /// the corpus is the 50-per-repo window rather than all of GitHub.
    #[test]
    fn the_search_chip_reports_the_query_and_the_counts() {
        assert_eq!(
            search_summary("api", 3, 57).as_deref(),
            Some("search:\"api\" 3/57")
        );
        // No search -> no chip at all, so the header line stays clean.
        assert_eq!(search_summary("", 57, 57), None);
        assert_eq!(search_summary("   ", 57, 57), None);
    }

    /// The search prompt must not promise "confirm/cancel": it is live, and
    /// its esc clears the query instead of restoring the previous one.
    #[test]
    fn the_search_prompt_announces_its_own_esc() {
        assert_eq!(
            input_hint(InputKind::Search),
            "   (live · enter: keep · esc: clear)"
        );
        assert_eq!(input_hint(InputKind::Author), input_hint(InputKind::Label));
        assert!(input_hint(InputKind::Author).contains("cancel"));
    }

    /// I1: neither of the column panel's two hint lines (browsing / grabbed)
    /// must be truncated. `render_column_panel` itself takes `&App`, and
    /// building an `App` reads the user's real `~/.config` (forbidden in
    /// tests), so this reproduces exactly the panel's own line-building (the
    /// real `column_lines`, `COLUMN_PANEL_HINT[_GRABBED]` and
    /// `COLUMN_PANEL_WIDTH`) instead of calling `render_column_panel` directly.
    /// Before the fix (`COLUMN_PANEL_WIDTH = 40`) this failed: `Paragraph`
    /// truncates instead of wrapping, and "esc close" fell off the line.
    #[test]
    fn column_panel_hint_is_not_truncated() {
        let layout = ColumnLayout::<PrColumn>::default();

        for (grabbed, hint, last_word) in [
            (false, COLUMN_PANEL_HINT, "esc close"),
            (true, COLUMN_PANEL_HINT_GRABBED, "esc drop"),
        ] {
            let text = render_to_text(80, 24, |frame| {
                let mut lines = column_lines(&layout, 0, grabbed);
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    hint,
                    Style::new().fg(Color::DarkGray),
                )));

                let popup = centered_rect(COLUMN_PANEL_WIDTH, lines.len() as u16 + 2, frame.area());
                frame.render_widget(Clear, popup);
                frame.render_widget(
                    Paragraph::new(lines).block(Block::bordered().title(" Columns — PRs ")),
                    popup,
                );
            });

            assert!(
                text.contains(last_word),
                "the column panel's hint (grabbed={grabbed}) must not be truncated"
            );
        }
    }

    /// The grab gesture (I5/new): the grabbed row must render 2 columns
    /// further right than a neighbour, on the SAME render — that shift is
    /// the whole visual cue that the row is now attached to ↑/↓. Compared via
    /// the column of the row's `[` (its toggle box), rather than a raw
    /// leading-whitespace count, because the unfocused marker ("  ") is
    /// itself made of spaces: a plain "count leading spaces" would find 2 on
    /// an ordinary unfocused row too, and miss the extra shift entirely.
    #[test]
    fn the_grabbed_row_is_indented_relative_to_a_neighbour() {
        let layout = ColumnLayout::<PrColumn>::default();

        // Cursor on entry 1: entry 0 is an unfocused neighbour in the very
        // same call, so it is unaffected by `grabbed` and serves as the
        // baseline both times.
        // `.position()` on `chars()`, NOT `str::find` (byte offset): the
        // marker's `▸` is a multi-byte character, so a byte offset would
        // overcount the shift as soon as a grabbed row's `▸` is involved.
        let bracket_col = |line: &Line| -> usize {
            line.spans
                .iter()
                .flat_map(|s| s.content.chars())
                .position(|c| c == '[')
                .expect("every column row has a toggle box")
        };

        let grabbed = column_lines(&layout, 1, true);
        let plain = column_lines(&layout, 1, false);

        let neighbour_col = bracket_col(&grabbed[0]);
        assert_eq!(
            bracket_col(&grabbed[1]),
            neighbour_col + 2,
            "the grabbed row's toggle box must sit 2 columns right of its neighbour's"
        );
        assert_eq!(
            bracket_col(&plain[1]),
            neighbour_col,
            "the same row, not grabbed, must line up with its neighbour"
        );
    }
}
