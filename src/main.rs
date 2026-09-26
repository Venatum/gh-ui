mod app;
mod columns;
mod config;
mod fetch;
mod filters;
mod gh;
mod model;
mod refresh;
mod repos;
mod runfilters;
mod search;
mod ui;

use anyhow::Result;
use app::{App, Tab};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

fn main() -> Result<()> {
    let arg = std::env::args().nth(1);

    // The two flags every command is expected to answer, handled before we take
    // over the terminal: they print one line and leave.
    match arg.as_deref() {
        Some("-V" | "--version") => {
            println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("-h" | "--help") => {
            print_usage();
            return Ok(());
        }
        _ => {}
    }

    // Anything else is the folder to scan, defaulting to the current one.
    let root: PathBuf = arg.map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));

    let mut app = App::new(root);
    app.start(); // the account, the orgs, and the first load — BEFORE the TUI

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, &mut app); // capture the result...
    ratatui::restore(); // ...so we restore the terminal no matter what
    result
}

/// What `gh-ui --help` prints. `env!` reads the values Cargo bakes in at
/// compile time, so they can never drift from `Cargo.toml`.
fn print_usage() {
    println!(
        "\
{name} {version}
{description}

Usage: {name} [FOLDER]

Arguments:
  [FOLDER]  Folder holding the git repos to scan [default: .]

Options:
  -h, --help     Print this help
  -V, --version  Print the version

Requires `gh` to be installed and authenticated (`gh auth login`).
Press `?` inside the app for the keyboard shortcuts.",
        name = env!("CARGO_PKG_NAME"),
        version = env!("CARGO_PKG_VERSION"),
        description = env!("CARGO_PKG_DESCRIPTION"),
    );
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
                    // Six modes, by priority: input, yes/no prompt, help,
                    // filters, columns, normal.
                    if app.is_input_mode() {
                        handle_input_key(app, key.code);
                    } else if app.confirm.is_some() {
                        handle_confirm_key(app, key.code);
                    } else if app.show_help {
                        app.toggle_help(); // any key closes the help
                    } else if app.filter_panel_open {
                        handle_filter_panel_key(app, key.code);
                    } else if app.column_panel_open {
                        handle_column_panel_key(app, key.code);
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
        KeyCode::Char('q') => app.request_quit(),
        KeyCode::Char('j') | KeyCode::Down => app.next(),
        KeyCode::Char('k') | KeyCode::Up => app.previous(),
        KeyCode::PageDown => app.page_down(),
        KeyCode::PageUp => app.page_up(),
        KeyCode::Home => app.first(),
        KeyCode::End => app.last(),
        KeyCode::Char('r') => app.refresh(),
        KeyCode::Char('a') => app.cycle_auto_refresh(),
        KeyCode::Char('A') => app.disable_auto_refresh(),
        // On the Repos tab's clone button, enter clones; everywhere else it
        // opens the selected item in the browser.
        KeyCode::Enter => {
            if app.on_clone_button() {
                app.ask_clone();
            } else {
                open_selected(app);
            }
        }
        KeyCode::Char(' ') => app.toggle_tick(),
        KeyCode::Char('f') => app.toggle_filter_panel(), // opens the panel
        KeyCode::Char('c') => app.toggle_column_panel(),
        KeyCode::Char('?') => app.toggle_help(),
        KeyCode::Char('/') => app.start_search(),
        // k9s's reflex: esc drops the search without reopening the prompt.
        KeyCode::Esc => app.clear_search(),
        KeyCode::Tab => app.next_tab(),
        KeyCode::Char('1') => app.set_tab(Tab::Prs),
        KeyCode::Char('2') => app.set_tab(Tab::Runs),
        KeyCode::Char('3') => app.set_tab(Tab::Repos),
        // "Mine" on either tab: my PRs, or the runs of my PRs.
        KeyCode::Char('m') => app.toggle_mine(),
        _ => {}
    }
}

/// Keys in the filter panel.
fn handle_filter_panel_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc | KeyCode::Char('f') => app.close_filter_panel(),
        KeyCode::Char('q') => app.request_quit(),
        KeyCode::Down | KeyCode::Char('j') => app.filter_cursor_next(),
        KeyCode::Up | KeyCode::Char('k') => app.filter_cursor_prev(),
        KeyCode::Right | KeyCode::Char('l') => app.filter_change(true),
        KeyCode::Left | KeyCode::Char('h') => app.filter_change(false),
        KeyCode::Char(' ') => app.filter_change(true),
        KeyCode::Enter => app.filter_activate(),
        _ => {}
    }
}

/// Shortcuts of the column panel (key `c`). The list is vertical but the table
/// is horizontal: the top of the list is the leftmost column, so `↑`/`k` moves
/// (the cursor, or the grabbed column) towards the LEFT, `↓`/`j` towards the
/// RIGHT.
///
/// Two modes, tracked by `App::column_grabbed`: browsing (the default), and
/// "grabbed" after `space` — a direct-manipulation gesture that replaces the
/// earlier uppercase `J`/`K` move keys. While grabbed, the same `↑`/`↓` move
/// the focused column instead of the cursor; `space` drops it again.
fn handle_column_panel_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('q') => app.request_quit(),
        // Always closes, grabbed or not: `close_column_panel` clears the
        // grab flag itself, so leaving the panel never leaves it stale.
        KeyCode::Char('c') => app.close_column_panel(),
        // Esc drops first if grabbed (so a second esc is needed to close);
        // otherwise it closes the panel right away.
        KeyCode::Esc => {
            if app.column_grabbed {
                app.column_drop();
            } else {
                app.close_column_panel();
            }
        }
        // Toggling the flag IS grabbing when not grabbed, and dropping when
        // already grabbed — one call covers both directions.
        KeyCode::Char(' ') => app.column_grab_toggle(),
        KeyCode::Enter => {
            if app.column_grabbed {
                app.column_drop();
            } else {
                app.column_toggle();
            }
        }
        KeyCode::Down | KeyCode::Char('j') => {
            if app.column_grabbed {
                app.column_move(false);
            } else {
                app.column_cursor_next();
            }
        }
        KeyCode::Up | KeyCode::Char('k') => {
            if app.column_grabbed {
                app.column_move(true);
            } else {
                app.column_cursor_prev();
            }
        }
        _ => {}
    }
}

/// Keys in text input mode (author / label / search).
fn handle_input_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Esc => app.input_cancel(),
        KeyCode::Enter => app.input_commit(),
        KeyCode::Backspace => app.input_backspace(),
        KeyCode::Char(c) => app.input_push(c),
        _ => {}
    }
}

/// Keys on a yes/no prompt: only an explicit `y` says yes.
fn handle_confirm_key(app: &mut App, code: KeyCode) {
    match code {
        KeyCode::Char('y' | 'Y') => app.confirm_yes(),
        KeyCode::Char('n' | 'N') | KeyCode::Esc => app.confirm_no(),
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
