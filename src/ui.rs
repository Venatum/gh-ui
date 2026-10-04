//! The rendering: turns the state (`App`) into ratatui widgets. This module
//! decides nothing, it only draws what `App` holds.

use crate::app::{App, Confirm, FilterField, InputKind, Tab, section_of};
use crate::columns::{
    Column, ColumnLayout, IssueColumn, PrColumn, ReleaseColumn, RepoColumn, RunColumn,
    STATUS_LEGEND, release_row_style, repo_row_style,
};
use crate::detail::{
    DetailView, Section, checks_lines, comments_lines, files_lines, overview_lines, section_bar,
};
use crate::filters::{AuthorFilter, Filters};
use crate::issues::IssueFilters;
use crate::refresh::{self, AutoRefresh};
use crate::repos::RepoFilters;
use crate::runfilters::RunFilters;
use chrono::{Local, Offset, Utc};
use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Cell, Clear, Padding, Paragraph, Row, Table, Wrap};
use std::time::Duration;

/// Width (in columns) of the centered overlays.
const FILTER_PANEL_WIDTH: u16 = 52;
const COLUMN_PANEL_WIDTH: u16 = 54;
const HELP_WIDTH: u16 = 64;

/// What an empty text filter reads in the panel.
const EMPTY_TEXT_FILTER: &str = "(empty)  ⏎ edit";

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
    // The detail view takes the table's place; the header and the footer
    // stay, so the spinner and the auto-refresh countdown keep working.
    if let Some(view) = app.detail.as_mut() {
        render_detail(frame, view, areas[1]);
    } else {
        render_table(frame, app, areas[1]);
    }
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

/// The header's auto-refresh chip: the pace, then how long before the next
/// reload. Split out of `render_header` so a test can assert on it without
/// building an `App` (which would read the user's real config).
fn auto_refresh_chip(pace: AutoRefresh, left: Duration) -> String {
    format!(
        "   ⟳ auto {} · {}",
        pace.label(),
        refresh::format_countdown(left)
    )
}

fn render_header(frame: &mut Frame, app: &App, area: Rect) {
    // Line 1: name + spinner (if loading) + status + auto-refresh state.
    let mut top = vec![
        Span::styled("gh-ui", Style::new().bold().fg(Color::Cyan)),
        Span::raw("  —  "),
    ];
    if app.is_busy() {
        const FRAMES: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
        let c = FRAMES[app.spinner_frame % FRAMES.len()];
        top.push(Span::styled(
            format!("{c} "),
            Style::new().fg(Color::Yellow),
        ));
    }
    top.push(Span::raw(app.status.clone()));
    // `time_to_refresh` is `None` exactly when the auto-refresh is off, so
    // the chip and the countdown appear and disappear together.
    if let Some(left) = app.time_to_refresh() {
        top.push(Span::styled(
            auto_refresh_chip(app.auto_refresh, left),
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
        Span::raw(" "),
        tab_span("Issues", app.active_tab == Tab::Issues),
        Span::raw(" "),
        tab_span("Repos", app.active_tab == Tab::Repos),
        Span::raw(" "),
        tab_span("Releases", app.active_tab == Tab::Releases),
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
                "{} · {}   (m to toggle)",
                app.filters.summary_common(),
                app.run_filters.summary()
            ),
            Style::new().fg(Color::DarkGray),
        ),
        Tab::Repos => Span::styled(
            format!(
                "{} · {} repos · {} cloned",
                app.repo_tab.filters.summary(),
                app.repo_tab.repos.len(),
                app.repo_tab.cloned_count()
            ),
            Style::new().fg(Color::DarkGray),
        ),
        Tab::Issues => Span::styled(
            app.issue_tab.filters.summary(),
            Style::new().fg(Color::DarkGray),
        ),
        // No filters to summarize: say what the rows are instead.
        Tab::Releases => Span::styled(
            "the latest release of every repo in the folder",
            Style::new().fg(Color::DarkGray),
        ),
    }];

    // Counted against the tab's own list: the PRs tab compares with everything
    // fetched, the Actions tab with what its branch toggle already kept.
    let (shown, total) = match app.active_tab {
        Tab::Prs => (app.visible_prs().len(), app.prs.len()),
        Tab::Runs => (app.visible_runs().len(), app.branch_runs().len()),
        Tab::Repos => (app.visible_repos().len(), app.repo_tab.listed().len()),
        Tab::Issues => (app.visible_issues().len(), app.issue_tab.issues.len()),
        Tab::Releases => (app.visible_releases().len(), app.release_tab.rows.len()),
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
    // What `PgUp`/`PgDn` jump by: the rows inside the border, header aside.
    app.page_rows = usize::from(area.height.saturating_sub(3)).max(1);
    match app.active_tab {
        Tab::Prs => render_pr_table(frame, app, area),
        Tab::Runs => render_run_table(frame, app, area),
        Tab::Repos => render_repo_table(frame, app, area),
        Tab::Issues => render_issue_table(frame, app, area),
        Tab::Releases => render_release_table(frame, app, area),
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

fn render_issue_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let columns: Vec<IssueColumn> = app.columns.issues.visible().collect();

    let header =
        Row::new(columns.iter().map(|c| c.header()).collect::<Vec<_>>()).style(Style::new().bold());
    let widths: Vec<Constraint> = columns.iter().map(|c| c.width()).collect();
    // Owned cells, as in the other tables: nothing may still borrow `app`
    // when `&mut app.issue_table_state` is handed over below.
    let rows: Vec<Row<'static>> = app
        .visible_issues()
        .into_iter()
        .map(|issue| Row::new(columns.iter().map(|c| c.cell(issue)).collect::<Vec<_>>()))
        .collect();

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::new().reversed())
        .highlight_symbol("▌ ")
        .block(Block::bordered());

    frame.render_stateful_widget(table, area, &mut app.issue_table_state);
}

fn render_release_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let columns: Vec<ReleaseColumn> = app.columns.releases.visible().collect();

    let header =
        Row::new(columns.iter().map(|c| c.header()).collect::<Vec<_>>()).style(Style::new().bold());
    let widths: Vec<Constraint> = columns.iter().map(|c| c.width()).collect();
    // Owned cells, as in the other tables: nothing may still borrow `app`
    // when `&mut app.release_table_state` is handed over below.
    let rows: Vec<Row<'static>> = app
        .visible_releases()
        .into_iter()
        .map(|row| {
            Row::new(columns.iter().map(|c| c.cell(row)).collect::<Vec<_>>())
                .style(release_row_style(row))
        })
        .collect();

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::new().reversed())
        .highlight_symbol("▌ ")
        .block(Block::bordered());

    frame.render_stateful_widget(table, area, &mut app.release_table_state);
}

fn render_repo_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let columns: Vec<RepoColumn> = app.columns.repos.visible().collect();

    let header =
        Row::new(columns.iter().map(|c| c.header()).collect::<Vec<_>>()).style(Style::new().bold());
    let widths: Vec<Constraint> = columns.iter().map(|c| c.width()).collect();
    // Owned cells, as in the other tables: nothing may still borrow `app`
    // when `&mut app.repo_table_state` is handed over below.
    let mut rows: Vec<Row<'static>> = app
        .visible_repos()
        .into_iter()
        .map(|repo| {
            let state = app.repo_tab.state_of(repo);
            let ticked = app.repo_tab.ticked.contains(&repo.name_with_owner);
            Row::new(
                columns
                    .iter()
                    .map(|c| c.cell(repo, &state, ticked))
                    .collect::<Vec<_>>(),
            )
            .style(repo_row_style(repo))
        })
        .collect();

    // The clone button: one more row, so ↓ reaches it like any other. Its
    // label sits in the first column wide enough for it (not the tick box).
    if app.repo_tab.show_button() {
        let n = app.repo_tab.ticked.len();
        let label_at = columns
            .iter()
            .position(|c| *c != RepoColumn::Tick)
            .unwrap_or(0);
        let cells = (0..columns.len()).map(|i| {
            if i == label_at {
                Cell::from(Span::styled(
                    format!("[ Clone {n} {} ]", repos_word(n)),
                    Style::new().bold().fg(Color::Cyan),
                ))
            } else {
                Cell::from("")
            }
        });
        rows.push(Row::new(cells.collect::<Vec<_>>()));
    }

    let table = Table::new(rows, widths)
        .header(header)
        .row_highlight_style(Style::new().reversed())
        .highlight_symbol("▌ ")
        .block(Block::bordered());

    frame.render_stateful_widget(table, area, &mut app.repo_table_state);
}

/// The detail view (key `v`): a bordered box named after the PR, holding
/// its body once `gh` answered, or what it waits for, or why it failed.
fn render_detail(frame: &mut Frame, view: &mut DetailView, area: Rect) {
    // The loaded title, which may have changed since the list was loaded.
    let title = view
        .detail
        .as_ref()
        .map_or(view.title.as_str(), |d| d.title.as_str());
    let block = Block::bordered()
        .title(format!(
            " {} #{} · {} ",
            view.key.repo, view.key.number, title
        ))
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if view.detail.is_some() {
        render_sections(frame, view, inner);
    } else if let Some(error) = &view.error {
        let lines = vec![
            Line::from(Span::styled(
                format!("✗ {error}"),
                Style::new().fg(Color::Red),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "r  try again  ·  esc  back to the list",
                Style::new().fg(Color::DarkGray),
            )),
        ];
        render_centered(frame, lines, inner);
    } else {
        let waiting = format!("Loading {} #{}…", view.key.repo, view.key.number);
        render_centered(frame, vec![Line::from(waiting)], inner);
    }
}

/// A loaded view: the section bar, a blank row, then the section's text,
/// scrolled.
fn render_sections(frame: &mut Frame, view: &mut DetailView, area: Rect) {
    let Some(detail) = view.detail.as_deref() else {
        return;
    };
    let [bar, _, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);
    // Owned lines: `detail` borrows `view`, which is written below.
    let mut bar_line = section_bar(view.section, detail);
    // A reload that failed kept the text: its error goes at the end of the
    // bar, against the right edge (cut there if the bar is too narrow).
    if let Some(error) = &view.error {
        let error = Span::styled(format!("✗ {error}"), Style::new().fg(Color::Red));
        let room = usize::from(bar.width).saturating_sub(bar_line.width() + error.width());
        bar_line.push_span(Span::raw(" ".repeat(room.max(1))));
        bar_line.push_span(error);
    }
    let lines = match view.section {
        Section::Overview => overview_lines(detail, body.width),
        Section::Checks => checks_lines(detail, Utc::now(), body.width),
        Section::Files => files_lines(detail, body.width),
        Section::Comments => comments_lines(detail, Local::now().offset().fix(), view.show_folded),
    };
    frame.render_widget(Paragraph::new(bar_line), bar);

    let paragraph = Paragraph::new(Text::from(lines)).wrap(Wrap { trim: false });
    // Counted once wrapped, so the last screenful is exact however long the
    // lines are. A reload may have shortened the text: the scroll comes
    // back within it.
    let rows = u16::try_from(paragraph.line_count(body.width)).unwrap_or(u16::MAX);
    view.page = body.height.max(1);
    view.max_scroll = rows.saturating_sub(body.height);
    let scroll = view.scroll().min(view.max_scroll);
    *view.scroll_mut() = scroll;
    frame.render_widget(paragraph.scroll((scroll, 0)), body);
}

/// `lines`, each one centered, in the middle of `area`. A line wider than
/// the area wraps rather than losing its tail: an error message is the one
/// thing the user must be able to read in full.
fn render_centered(frame: &mut Frame, lines: Vec<Line<'static>>, area: Rect) {
    let paragraph = Paragraph::new(lines).centered().wrap(Wrap { trim: true });
    let rows = u16::try_from(paragraph.line_count(area.width)).unwrap_or(u16::MAX);
    let height = rows.min(area.height);
    frame.render_widget(paragraph, centered_rect(area.width, height, area));
}

/// The detail view's footer: only the keys that work there, the list's
/// own keys wait until `esc`.
const DETAIL_HINTS: [(&str, &str); 7] = [
    ("esc", "back"),
    ("←→", "section"),
    ("↑↓", "scroll"),
    ("enter", "browser"),
    ("space", "unfold"),
    ("r", "reload"),
    ("q", "quit"),
];

/// The footer shortcuts, in the order they matter. `?` is not in the list: it
/// is appended separately and never dropped, because it is how the user
/// reaches everything the footer had to cut.
const HINTS: [(&str, &str); 11] = [
    ("↑↓", "nav"),
    // Near the front on purpose: hints are dropped from the tail, and `/` is
    // the one key on this row that is not reachable from a panel.
    ("/", "search"),
    ("enter", "open"),
    // Next to `enter`: the other way to look at the selected PR.
    ("v", "view"),
    ("tab/1-5", "tab"),
    ("f", "filters"),
    ("m", "mine"),
    ("c", "columns"),
    ("r", "refresh"),
    ("a", "auto"),
    ("q", "quit"),
];

/// The footer of `tab`. `v` opens a PR's detail: the PRs tab only. `m`
/// does nothing on the Repos tab, and `space` ticks there: same slot, so
/// both drop at the same width. The Releases tab has nothing to filter nor
/// to call mine: both hints go.
fn hints_for(tab: Tab) -> Vec<(&'static str, &'static str)> {
    let mut hints = HINTS.to_vec();
    if tab != Tab::Prs {
        hints.retain(|hint| hint.0 != "v");
    }
    match tab {
        Tab::Repos => {
            for hint in &mut hints {
                if *hint == ("m", "mine") {
                    *hint = ("space", "tick");
                }
            }
        }
        Tab::Releases => hints.retain(|hint| !matches!(hint.0, "m" | "f")),
        _ => {}
    }
    hints
}

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
fn fitting_hints(
    hints: &[(&'static str, &'static str)],
    width: usize,
) -> (Vec<(&'static str, &'static str)>, bool) {
    let help = hint_width(HELP_HINT);
    if hints.iter().copied().map(hint_width).sum::<usize>() + help <= width {
        return (hints.to_vec(), false);
    }

    // "… " sits between the kept hints and `?`, so it is part of the budget.
    let budget = width.saturating_sub(help + 2);
    let mut kept = Vec::new();
    let mut used = 0;
    for &hint in hints {
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
    // A yes/no prompt takes the footer over, like a text prompt does.
    if let Some(confirm) = &app.confirm {
        let line = Line::from(vec![
            Span::styled(
                format!(" {} ", confirm_question(confirm)),
                Style::new().fg(Color::Black).bg(Color::Yellow),
            ),
            Span::styled("  (y/n)", Style::new().fg(Color::DarkGray)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
        return;
    }

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
    // Bound first, so the slice below does not borrow a temporary.
    let tab_hints = hints_for(app.active_tab);
    let hints: &[(&str, &str)] = if app.detail.is_some() {
        &DETAIL_HINTS
    } else {
        &tab_hints
    };
    let (hints, cut) = fitting_hints(hints, area.width as usize);
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

/// The question a yes/no prompt asks.
fn confirm_question(confirm: &Confirm) -> String {
    match confirm {
        Confirm::Clone { count, into } => {
            format!("Clone {count} {} into {into}?", repos_word(*count))
        }
        Confirm::Quit => "Clones in progress, quit anyway?".to_string(),
    }
}

/// "repo" or "repos", for the button and the prompt.
fn repos_word(n: usize) -> &'static str {
    if n == 1 { "repo" } else { "repos" }
}

/// The navigable filter panel, centered over the interface.
fn render_filter_panel(frame: &mut Frame, app: &App, area: Rect) {
    let f = &app.filters;

    // One line per filter of the ACTIVE tab, grouped under section titles.
    // The titles are not navigable: `filter_cursor` indexes the fields only.
    let mut lines: Vec<Line> = Vec::new();
    let mut section = "";
    for (i, &field) in app.active_fields().iter().enumerate() {
        if section_of(app.active_tab, field) != section {
            section = section_of(app.active_tab, field);
            lines.push(Line::from(Span::styled(
                format!(" {section}"),
                Style::new().bold().fg(Color::Cyan),
            )));
        }

        let focused = i == app.filter_cursor;
        let marker = if focused { "▸ " } else { "  " };
        let value = if app.active_tab == Tab::Issues {
            issue_field_value(field, &app.issue_tab.filters)
        } else if field == FilterField::Owner && app.repo_tab.cloning {
            locked_owner_value(app.repo_tab.filters.owner.as_deref())
        } else {
            field_value(field, f, &app.run_filters, &app.repo_tab.filters)
        };
        let text = format!("{marker}{:<12} {}", field_name(field), value);
        lines.push(if focused {
            Line::from(Span::styled(
                text,
                Style::new().fg(Color::Black).bg(Color::Cyan),
            ))
        } else {
            Line::from(Span::raw(text))
        });
    }

    // A tab without filters (Releases) still answers `f`, with a reason
    // rather than an empty box.
    let hint = if lines.is_empty() {
        lines.push(Line::from(" No filters on this tab: / searches it."));
        " esc close"
    } else {
        " ↑↓ navigate · ←→ change · ⏎ edit · esc close"
    };
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        hint,
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
        Tab::Repos => (
            " Columns — Repos ",
            column_lines(&app.columns.repos, app.column_cursor, app.column_grabbed),
        ),
        Tab::Issues => (
            " Columns — Issues ",
            column_lines(&app.columns.issues, app.column_cursor, app.column_grabbed),
        ),
        Tab::Releases => (
            " Columns — Releases ",
            column_lines(&app.columns.releases, app.column_cursor, app.column_grabbed),
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
        FilterField::Since => "Since",
        FilterField::NoDraft => "No draft",
        FilterField::Unreviewed => "Unreviewed",
        FilterField::Repo => "Repo",
        FilterField::Author => "Author",
        FilterField::ReviewAsked => "Review asked",
        FilterField::Label => "Label",
        FilterField::OnlyPrRuns => "Only my PRs",
        FilterField::RunStatus => "Status",
        FilterField::RunEvent => "Event",
        FilterField::RunWorkflow => "Workflow",
        FilterField::Owner => "Owner",
        FilterField::Archived => "Archived",
        FilterField::Forks => "Forks",
        FilterField::HideCloned => "Hide cloned",
        FilterField::Assignee => "Assignee",
    }
}

/// The displayed value of a field: cycle "◂ x ▸", box "[x]", or text.
/// The Owner row while a clone batch runs: the owner cannot change until
/// it ends (see `App::filter_change`), so no arrows, and the reason.
fn locked_owner_value(owner: Option<&str>) -> String {
    format!("{}  (locked while cloning)", owner.unwrap_or("…"))
}

fn field_value(field: FilterField, f: &Filters, rf: &RunFilters, rpf: &RepoFilters) -> String {
    match field {
        FilterField::OnlyPrRuns => toggle_box(rf.only_pr_runs),
        FilterField::RunStatus => format!("◂ {} ▸", rf.status.label()),
        FilterField::RunEvent => format!("◂ {} ▸", rf.event.as_deref().unwrap_or("all")),
        FilterField::RunWorkflow => format!("◂ {} ▸", rf.workflow.as_deref().unwrap_or("all")),
        FilterField::Owner => format!("◂ {} ▸", rpf.owner.as_deref().unwrap_or("…")),
        FilterField::Archived => toggle_box(rpf.archived),
        FilterField::Forks => toggle_box(rpf.forks),
        FilterField::HideCloned => toggle_box(rpf.hide_cloned),
        FilterField::Since => format!("◂ {} ▸", f.since.label()),
        FilterField::NoDraft => toggle_box(f.no_draft),
        FilterField::Unreviewed => toggle_box(f.unreviewed),
        FilterField::ReviewAsked => toggle_box(f.review_requested),
        FilterField::Repo => format!("◂ {} ▸", f.repo.as_deref().unwrap_or("all")),
        FilterField::Author => format!("◂ {} ▸", author_label(&f.author)),
        FilterField::Label => {
            if f.labels.is_empty() {
                EMPTY_TEXT_FILTER.to_string()
            } else {
                f.labels.join(" ")
            }
        }
        // Issues tab only: drawn by `issue_field_value`.
        FilterField::Assignee => String::new(),
    }
}

fn toggle_box(on: bool) -> String {
    if on {
        "[x]".to_string()
    } else {
        "[ ]".to_string()
    }
}

/// The displayed value of a field on the Issues tab, read from its own
/// filters: the rows are the PRs tab's, the values are not.
fn issue_field_value(field: FilterField, f: &IssueFilters) -> String {
    match field {
        FilterField::Repo => format!("◂ {} ▸", f.repo.as_deref().unwrap_or("all")),
        FilterField::Author => format!("◂ {} ▸", author_label(&f.author)),
        FilterField::Assignee => format!("◂ {} ▸", f.assignee.label()),
        FilterField::Since => format!("◂ {} ▸", f.since.label()),
        FilterField::Label if f.labels.is_empty() => EMPTY_TEXT_FILTER.to_string(),
        FilterField::Label => f.labels.join(" "),
        // Not an Issues row.
        _ => String::new(),
    }
}

/// The `Author` row's value: `any`, `@me`, `not @me`, `octocat`, `not octocat`.
/// Words rather than the query's `-`, which is easy to miss inside `◂ ▸`.
fn author_label(author: &AuthorFilter) -> String {
    match author {
        AuthorFilter::Any => "any".to_string(),
        AuthorFilter::Is(login) => login.clone(),
        AuthorFilter::IsNot(login) => format!("not {login}"),
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
        help_row("↑/↓, j/k", "navigate · PgUp/PgDn: a page · Home/End: ends"),
        // Two rows rewritten rather than one added for `v`: the popup
        // already fills an 80x24 terminal.
        help_row(
            "enter / v",
            "open in the browser / in the detail view (PRs)",
        ),
        help_row("space", "Repos: tick · enter on the clone button: clone"),
        help_row("/", "search the visible list · esc clears it"),
        help_row("r / q", "reload now / quit"),
        help_row("a / A", "auto-refresh: off/1mn/5mn/10mn/30mn/1h · A: off"),
        help_row("f / c", "open the filters / columns panel"),
        help_row("tab/1-5", "switch tab (PRs/Actions/Issues/Repos/Releases)"),
        help_row("m", "mine: PRs / their runs / issues assigned to me"),
        help_row("State", STATUS_LEGEND),
        Line::from(""),
        Line::from(Span::styled("  In the filter panel", Style::new().bold())),
        // One row for both arrow pairs: it pays for the `State` legend's line,
        // the popup already filling an 80x24 terminal.
        help_row(
            "↑/↓ ←/→",
            "choose a filter, change its value (cycles, boxes)",
        ),
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
    use crate::app::Confirm;
    use crate::columns::{ColumnLayout, PrColumn};
    use crate::detail::{DetailKey, DetailView};
    use crate::repos::RepoFilters;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::path::PathBuf;

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
        let (hints, cut) = fitting_hints(&HINTS, 200);
        assert_eq!(hints.len(), HINTS.len());
        assert!(!cut);
    }

    #[test]
    fn an_80_column_footer_drops_hints_instead_of_clipping_them() {
        let (hints, cut) = fitting_hints(&HINTS, 80);

        assert!(cut, "the full row is 127 columns, it cannot fit 80");
        // Whole hints are dropped, and what is kept is the head of the list.
        assert_eq!(hints, HINTS[..hints.len()].to_vec());

        // Everything rendered fits: the kept hints, "… ", and `?` itself.
        let rendered: usize =
            hints.iter().copied().map(hint_width).sum::<usize>() + 2 + hint_width(HELP_HINT);
        assert!(rendered <= 80, "rendered {rendered} columns in 80");
    }

    #[test]
    fn the_repos_footer_offers_tick_where_the_others_offer_mine() {
        let repos = hints_for(Tab::Repos);
        assert!(repos.contains(&("space", "tick")));
        assert!(!repos.contains(&("m", "mine")), "m does nothing there");

        for tab in [Tab::Prs, Tab::Runs, Tab::Issues] {
            assert!(hints_for(tab).contains(&("m", "mine")), "{tab:?}");
        }
    }

    /// `v` opens the detail of a PR: the other tabs have no detail view.
    #[test]
    fn only_the_prs_footer_offers_v() {
        assert_eq!(hints_for(Tab::Prs), HINTS.to_vec());
        assert!(hints_for(Tab::Prs).contains(&("v", "view")));
        for tab in [Tab::Runs, Tab::Issues, Tab::Repos, Tab::Releases] {
            assert!(!hints_for(tab).contains(&("v", "view")), "{tab:?}");
        }
    }

    #[test]
    fn help_survives_a_terminal_too_narrow_for_anything_else() {
        // `?` is appended unconditionally, so a tiny width keeps no hint at
        // all rather than panicking on the budget subtraction.
        let (hints, cut) = fitting_hints(&HINTS, 4);
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

    /// An unset `event`/`workflow` reads `all`, not `(empty)`: they are
    /// cycles like `repo`, not text fields like `author`, and "all" is a real
    /// value of the cycle rather than a prompt to go and type something.
    #[test]
    fn an_unset_run_cycle_shows_all() {
        let f = Filters::default();
        let rf = RunFilters::default();

        assert_eq!(
            field_value(FilterField::RunEvent, &f, &rf, &RepoFilters::default()),
            "◂ all ▸"
        );
        assert_eq!(
            field_value(FilterField::RunWorkflow, &f, &rf, &RepoFilters::default()),
            "◂ all ▸"
        );
        assert_eq!(
            field_value(FilterField::RunStatus, &f, &rf, &RepoFilters::default()),
            "◂ all ▸"
        );

        let rf = RunFilters {
            event: Some("push".into()),
            ..rf
        };
        assert_eq!(
            field_value(FilterField::RunEvent, &f, &rf, &RepoFilters::default()),
            "◂ push ▸"
        );
    }

    /// The `m` row names all three of its meanings (my PRs on the PRs tab,
    /// their runs on Actions, the issues assigned to me on Issues). Rendered
    /// for real, since `Paragraph` would clip the tail — the part that
    /// carries the information — without a word.
    #[test]
    fn the_m_help_row_names_all_three_tabs() {
        let text = render_to_text(80, 24, |frame| render_help(frame, frame.area()));

        assert!(
            text.contains("mine: PRs / their runs / issues assigned to me"),
            "the `m` help row must fit HELP_WIDTH without being cut"
        );
    }

    /// The `/` row must be in the help AND fit `HELP_WIDTH` — `Paragraph`
    /// truncates instead of wrapping, so an over-long row loses its tail in
    /// silence. Same guard as `the_auto_refresh_help_row_is_not_truncated`.
    /// The `State` column's legend is in the help, whole: `Paragraph` would
    /// clip a longer legend (or a narrower `HELP_WIDTH`) without a word.
    #[test]
    fn the_status_legend_row_is_not_truncated() {
        let text = render_to_text(80, 24, |frame| render_help(frame, frame.area()));

        assert!(
            text.contains(STATUS_LEGEND),
            "the status legend must fit HELP_WIDTH without being cut"
        );
    }

    /// The legend's line is paid for by the filter panel's two arrow rows,
    /// merged into one: the popup stays 24 rows high.
    #[test]
    fn the_merged_filter_panel_arrows_row_is_not_truncated() {
        let text = render_to_text(80, 24, |frame| render_help(frame, frame.area()));

        assert!(
            text.contains("choose a filter, change its value (cycles, boxes)"),
            "the merged arrows row must fit HELP_WIDTH without being cut"
        );
    }

    #[test]
    fn the_search_help_row_is_not_truncated() {
        let text = render_to_text(80, 24, |frame| render_help(frame, frame.area()));

        assert!(
            text.contains("search the visible list \u{b7} esc clears it"),
            "the search help row must fit HELP_WIDTH without being cut"
        );
        // Its cost: the two panel rows merged into one.
        assert!(
            text.contains("open the filters / columns panel"),
            "the merged f/c row must fit too"
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

    /// The header cannot be rendered here (building an `App` reads the user's
    /// real `~/.config`), so the chip is asserted on its own. It must show
    /// BOTH the pace and the time left: the pace alone never moves, and a
    /// frozen `auto 5mn` says nothing about when the next reload lands.
    #[test]
    fn the_auto_refresh_chip_shows_the_pace_and_the_countdown() {
        assert_eq!(
            auto_refresh_chip(AutoRefresh::M5, Duration::from_secs(252)),
            "   ⟳ auto 5mn · 4:12"
        );
        // Overdue (a prompt is open, or a load is in flight): it sits at zero.
        assert_eq!(
            auto_refresh_chip(AutoRefresh::M1, Duration::ZERO),
            "   ⟳ auto 1mn · 0:00"
        );
    }

    #[test]
    fn review_asked_is_a_checkbox() {
        let rf = RunFilters::default();
        let off = Filters::default();
        let on = Filters {
            review_requested: true,
            ..Default::default()
        };
        assert_eq!(
            field_value(FilterField::ReviewAsked, &off, &rf, &RepoFilters::default()),
            "[ ]"
        );
        assert_eq!(
            field_value(FilterField::ReviewAsked, &on, &rf, &RepoFilters::default()),
            "[x]"
        );
    }

    /// "Review asked" is 12 characters, one more than the name column held.
    /// Rendered for real, so a narrower column or panel cannot clip it.
    #[test]
    fn the_review_asked_row_is_not_truncated() {
        let app = App::new(PathBuf::from("."));
        let text = render_to_text(80, 24, |frame| {
            render_filter_panel(frame, &app, frame.area())
        });
        assert!(
            text.contains("Review asked [ ]"),
            "the Review asked row must fit the name column"
        );
    }

    #[test]
    fn the_author_row_names_its_value() {
        let rf = RunFilters::default();
        let row = |author| {
            let f = Filters {
                author,
                ..Default::default()
            };
            field_value(FilterField::Author, &f, &rf, &RepoFilters::default())
        };
        assert_eq!(row(AuthorFilter::Any), "◂ any ▸");
        assert_eq!(row(AuthorFilter::me()), "◂ @me ▸");
        assert_eq!(row(AuthorFilter::not_me()), "◂ not @me ▸");
        assert_eq!(row(AuthorFilter::Is("octocat".to_string())), "◂ octocat ▸");
        assert_eq!(
            row(AuthorFilter::IsNot("octocat".to_string())),
            "◂ not octocat ▸"
        );
    }

    #[test]
    fn the_repos_rows_show_the_owner_and_the_boxes() {
        let f = Filters::default();
        let rf = RunFilters::default();
        let rpf = RepoFilters {
            owner: Some("acme".to_string()),
            hide_cloned: true,
            ..Default::default()
        };
        assert_eq!(field_value(FilterField::Owner, &f, &rf, &rpf), "◂ acme ▸");
        assert_eq!(field_value(FilterField::Archived, &f, &rf, &rpf), "[ ]");
        assert_eq!(field_value(FilterField::HideCloned, &f, &rf, &rpf), "[x]");
        assert_eq!(
            field_value(FilterField::Owner, &f, &rf, &RepoFilters::default()),
            "◂ … ▸"
        );
    }

    /// No arrows while a batch runs: `←/→` would not switch it, and the
    /// status line saying so is overwritten by the batch's next step.
    #[test]
    fn the_owner_reads_locked_during_a_clone_batch() {
        assert_eq!(
            locked_owner_value(Some("acme")),
            "acme  (locked while cloning)"
        );
    }

    #[test]
    fn the_hide_cloned_row_is_not_truncated() {
        let mut app = App::new(PathBuf::from("."));
        app.active_tab = Tab::Repos;
        let text = render_to_text(80, 24, |frame| {
            render_filter_panel(frame, &app, frame.area())
        });
        assert!(text.contains("Hide cloned  [ ]"), "got {text}");
    }

    #[test]
    fn the_tab_help_row_names_the_five_tabs() {
        let text = render_to_text(80, 24, |frame| render_help(frame, frame.area()));
        assert!(
            text.contains("switch tab (PRs/Actions/Issues/Repos/Releases)"),
            "the row must fit HELP_WIDTH"
        );
        assert!(text.contains("tab/1-5"));
    }

    /// Neither `m` nor `f` has anything to do on the Releases tab: the
    /// footer leaves them out rather than promise them.
    #[test]
    fn the_releases_footer_offers_neither_mine_nor_filters() {
        let hints = hints_for(Tab::Releases);
        assert!(!hints.contains(&("m", "mine")));
        assert!(!hints.contains(&("f", "filters")));
        assert!(hints.contains(&("tab/1-5", "tab")));
        assert!(hints.contains(&("/", "search")));
    }

    /// `f` still opens the panel there (a key that does nothing at all
    /// reads as broken), and the panel says why it is empty.
    #[test]
    fn the_releases_filter_panel_says_there_is_none() {
        let mut app = App::new(PathBuf::from("."));
        app.active_tab = Tab::Releases;
        let text = render_to_text(80, 24, |frame| {
            render_filter_panel(frame, &app, frame.area())
        });
        assert!(text.contains("No filters on this tab"), "got {text}");
    }

    #[test]
    fn the_prompts_ask_a_yes_no_question() {
        assert_eq!(
            confirm_question(&Confirm::Clone {
                count: 3,
                into: "/ws".to_string()
            }),
            "Clone 3 repos into /ws?"
        );
        assert_eq!(
            confirm_question(&Confirm::Clone {
                count: 1,
                into: "/ws".to_string()
            }),
            "Clone 1 repo into /ws?"
        );
        assert_eq!(
            confirm_question(&Confirm::Quit),
            "Clones in progress, quit anyway?"
        );
    }

    #[test]
    fn the_help_explains_ticking_and_the_clone_button() {
        let text = render_to_text(80, 24, |frame| render_help(frame, frame.area()));
        assert!(text.contains("Repos: tick · enter on the clone button: clone"));
        assert!(text.contains("reload now / quit"));
        assert!(text.contains("(any key to close)"), "still fits 80x24");
    }

    /// The `v` row is paid for by rewriting the `enter` and `space` rows,
    /// not by a new one: the popup already fills an 80x24 terminal.
    #[test]
    fn the_help_names_the_detail_view_and_still_fits_80x24() {
        let text = render_to_text(80, 24, |frame| render_help(frame, frame.area()));
        assert!(text.contains("enter / v"));
        assert!(text.contains("open in the browser / in the detail view (PRs)"));
        assert!(text.contains("(any key to close)"));
    }

    fn loading_view() -> DetailView {
        DetailView::new(
            DetailKey {
                repo: "api".into(),
                number: 412,
            },
            "Add rate limiting".into(),
        )
    }

    #[test]
    fn a_loading_detail_names_its_pr_and_says_so() {
        let mut view = loading_view();
        let text = render_to_text(80, 24, |f| render_detail(f, &mut view, f.area()));
        assert!(text.contains("api #412 · Add rate limiting"));
        assert!(text.contains("Loading api #412…"));
    }

    #[test]
    fn a_failed_detail_says_why() {
        let mut view = loading_view();
        view.apply(Err("`gh pr view` failed: not found".into()));
        let text = render_to_text(80, 24, |f| render_detail(f, &mut view, f.area()));
        assert!(text.contains("`gh pr view` failed: not found"));
        assert!(text.contains("r  try again  ·  esc  back to the list"));
    }

    /// A reload that fails keeps the text, and says so on the section bar.
    #[test]
    fn a_failed_reload_shows_its_error_next_to_the_sections() {
        let mut view = long_view();
        view.loading = true;
        view.apply(Err("HTTP 502".into()));
        let text = render_to_text(80, 24, |f| render_detail(f, &mut view, f.area()));
        assert!(text.contains("line 1 "), "the text stays");
        // The buffer is one string: cut it back into its 80-column rows.
        let cells: Vec<char> = text.chars().collect();
        let bar: String = cells
            .chunks(80)
            .map(String::from_iter)
            .find(|row| row.contains("Overview"))
            .unwrap();
        assert!(bar.contains("✗ HTTP 502"), "{bar}");
    }

    /// At 80 columns the footer drops hints from its tail: the way back
    /// must be the first one, never dropped.
    #[test]
    fn the_detail_footer_keeps_back_and_help_at_80_columns() {
        let (hints, _) = fitting_hints(&DETAIL_HINTS, 80);
        assert_eq!(hints.first(), Some(&("esc", "back")));
    }

    /// A body of 60 numbered lines, loaded: far more than 24 rows hold.
    fn long_view() -> DetailView {
        let mut view = loading_view();
        let mut d = crate::detail::sample_detail();
        d.body = (1..=60)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        view.apply(Ok(Box::new(d)));
        view
    }

    #[test]
    fn end_shows_the_last_line_and_home_the_first() {
        let mut view = long_view();
        let text = render_to_text(80, 24, |f| render_detail(f, &mut view, f.area()));
        assert!(text.contains("line 1 ") && !text.contains("line 60"));
        assert!(view.max_scroll > 0, "the render says how far it can go");

        view.end();
        let text = render_to_text(80, 24, |f| render_detail(f, &mut view, f.area()));
        assert!(text.contains("line 60"));
        assert!(!text.contains("line 1 "));

        view.home();
        let text = render_to_text(80, 24, |f| render_detail(f, &mut view, f.area()));
        assert!(text.contains("line 1 "));
    }

    #[test]
    fn a_loaded_view_shows_the_section_bar_then_the_section() {
        let mut view = long_view();
        let text = render_to_text(80, 24, |f| render_detail(f, &mut view, f.area()));
        assert!(text.contains(" Overview   Checks 0 "));
        assert!(text.contains("line 1 "));

        view.next_section();
        let text = render_to_text(80, 24, |f| render_detail(f, &mut view, f.area()));
        assert!(text.contains("No checks on this PR."));
        assert!(!text.contains("line 1 "));
    }

    /// The end of the scroll counts the rows the text takes once wrapped,
    /// not its source lines: a long paragraph must still be readable to its
    /// last word.
    #[test]
    fn the_scroll_reaches_the_end_of_wrapped_text() {
        let mut view = loading_view();
        let mut d = crate::detail::sample_detail();
        d.body = format!("{} THE-END", "word ".repeat(400));
        view.apply(Ok(Box::new(d)));
        render_to_text(80, 24, |f| render_detail(f, &mut view, f.area()));
        view.end();
        let text = render_to_text(80, 24, |f| render_detail(f, &mut view, f.area()));
        assert!(text.contains("THE-END"));
    }

    #[test]
    fn the_issues_panel_reads_the_issue_filters() {
        let f = IssueFilters {
            assignee: crate::issues::AssigneeFilter::Nobody,
            repo: Some("api".to_string()),
            ..Default::default()
        };
        assert_eq!(issue_field_value(FilterField::Assignee, &f), "◂ nobody ▸");
        assert_eq!(issue_field_value(FilterField::Repo, &f), "◂ api ▸");
        assert_eq!(issue_field_value(FilterField::Author, &f), "◂ any ▸");
        assert_eq!(issue_field_value(FilterField::Label, &f), EMPTY_TEXT_FILTER);
    }
}
