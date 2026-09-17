mod app;
mod fetch;
mod filters;
mod gh;
mod model;
mod ui;

use anyhow::Result;
use app::{App, Tab};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

fn main() -> Result<()> {
    let root: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    let mut app = App::new(root);
    app.refresh(); // first load, BEFORE entering the TUI

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app); // capture the result...
    ratatui::restore(); // ...so we restore the terminal no matter what
    result
}

/// Loop cadence: we wake up at most every 100ms.
const TICK: Duration = Duration::from_millis(100);

/// The main loop: draw (if needed), wait, react, keep the state alive.
fn run(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    let mut needs_redraw = true; // draw at least once at startup

    while !app.should_quit {
        if needs_redraw {
            terminal.draw(|frame| ui::render(frame, app))?;
            needs_redraw = false;
        }

        // `poll(TICK)`: wait AT MOST 100ms for an event to arrive (otherwise we
        // fall through below anyway, to keep the state alive).
        if event::poll(TICK)? {
            match event::read()? {
                // A key was pressed (we ignore releases).
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    // Four modes, by priority: input, help, panel, normal.
                    if app.is_input_mode() {
                        handle_input_key(app, key.code);
                    } else if app.show_help {
                        app.toggle_help(); // any key closes the help
                    } else if app.filter_panel_open {
                        handle_filter_panel_key(app, key.code);
                    } else {
                        handle_normal_key(app, key.code);
                    }
                    needs_redraw = true;
                }
                // Terminal resized → we must redraw everything.
                Event::Resize(_, _) => needs_redraw = true,
                _ => {}
            }
        }

        // Collect the background thread's results and animate the spinner. Returns
        // `true` if there is something new to display.
        if app.on_tick() {
            needs_redraw = true;
        }
    }
    Ok(())
}

/// Shortcuts in normal mode (list navigation).
fn handle_normal_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Char('j') | KeyCode::Down => app.next(),
        KeyCode::Char('k') | KeyCode::Up => app.previous(),
        KeyCode::Char('r') => app.refresh(),
        KeyCode::Char('a') => app.toggle_auto_refresh(),
        KeyCode::Enter => open_selected(app),
        KeyCode::Char('f') => app.toggle_filter_panel(), // opens the panel
        KeyCode::Char('?') => app.toggle_help(),
        KeyCode::Tab => app.next_tab(),
        KeyCode::Char('1') => app.set_tab(Tab::Prs),
        KeyCode::Char('2') => app.set_tab(Tab::Runs),
        // Only visible on the Actions tab; harmless on the PRs tab.
        KeyCode::Char('m') => app.toggle_only_pr_runs(),
        _ => {}
    }
}

/// Keys in the filter panel.
fn handle_filter_panel_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc | KeyCode::Char('f') => app.close_filter_panel(),
        KeyCode::Char('q') => app.should_quit = true,
        KeyCode::Down | KeyCode::Char('j') => app.filter_cursor_next(),
        KeyCode::Up | KeyCode::Char('k') => app.filter_cursor_prev(),
        KeyCode::Right | KeyCode::Char('l') => app.filter_change(true),
        KeyCode::Left | KeyCode::Char('h') => app.filter_change(false),
        KeyCode::Char(' ') => app.filter_change(true),
        KeyCode::Enter => app.filter_activate(),
        _ => {}
    }
}

/// Keys in text input mode (author / label).
fn handle_input_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => app.input_cancel(),
        KeyCode::Enter => app.input_commit(),
        KeyCode::Backspace => app.input_backspace(),
        KeyCode::Char(c) => app.input_push(c),
        _ => {}
    }
}

/// Opens the selected item (PR or run) in the browser.
fn open_selected(app: &App) {
    if let Some(url) = app.selected_url() {
        open_url(&url);
    }
}

/// Opens a URL with the system's native opener. `#[cfg(...)]` selects the
/// code compiled per OS: only one of these three lines exists in the binary.
fn open_url(url: &str) {
    // `let _ =`: we ignore the Result (if it fails, we don't break the TUI).
    #[cfg(target_os = "macos")]
    let _ = Command::new("open").arg(url).status();

    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = Command::new("xdg-open").arg(url).status();

    #[cfg(target_os = "windows")]
    let _ = Command::new("cmd").args(["/C", "start", "", url]).status();
}
