mod app;
mod ui;

use std::io;

use color_eyre::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;

use zeroize::Zeroize;

use app::App;
use synh8::types::*;
use ui::ui;

fn main() -> Result<()> {
    color_eyre::install()?;

    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let mut app = App::new()?;

    loop {
        terminal.draw(|f| ui(f, &mut app))?;

        if event::poll(std::time::Duration::from_millis(100))?
            && let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                match app.state {
                    AppState::Listing => match key.code {
                        KeyCode::Char('q') => {
                            if app.has_pending_changes() {
                                app.state = AppState::ConfirmExit;
                            } else {
                                break;
                            }
                        }
                        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                            app.cycle_focus_back();
                        }
                        KeyCode::Tab => app.cycle_focus(),
                        KeyCode::BackTab => app.cycle_focus_back(),
                        KeyCode::Char('/') => app.start_search(),
                        KeyCode::Esc => {
                            if app.ui.visual_mode {
                                // Cancel visual mode
                                app.cancel_visual_mode();
                            } else if app.core.has_search_results() {
                                // Clear search filter
                                app.core.clear_search();
                                app.apply_current_filter();
                                app.update_status_message();
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => match app.ui.focused_pane {
                            FocusedPane::Filters => app.move_filter_selection(-1),
                            FocusedPane::Packages => {
                                app.move_package_selection(-1);
                                app.update_visual_selection();
                            }
                            FocusedPane::Details => {
                                app.details.scroll = app.details.scroll.saturating_sub(1);
                            }
                        },
                        KeyCode::Down | KeyCode::Char('j') => match app.ui.focused_pane {
                            FocusedPane::Filters => app.move_filter_selection(1),
                            FocusedPane::Packages => {
                                app.move_package_selection(1);
                                app.update_visual_selection();
                            }
                            FocusedPane::Details => {
                                app.details.scroll = app.details.scroll.saturating_add(1);
                            }
                        },
                        KeyCode::PageDown => {
                            app.move_package_selection(10);
                            app.update_visual_selection();
                        }
                        KeyCode::PageUp => {
                            app.move_package_selection(-10);
                            app.update_visual_selection();
                        }
                        KeyCode::Home => {
                            app.move_package_selection(-10000);
                            app.update_visual_selection();
                        }
                        KeyCode::End => {
                            app.move_package_selection(10000);
                            app.update_visual_selection();
                        }
                        KeyCode::Char('g') => {
                            app.move_package_selection(-10000);
                            app.update_visual_selection();
                        }
                        KeyCode::Char('G') => {
                            app.move_package_selection(10000);
                            app.update_visual_selection();
                        }
                        KeyCode::Char(' ') => {
                            if app.ui.visual_mode {
                                app.toggle_multi_select();
                            } else {
                                app.toggle_current();
                            }
                        }
                        KeyCode::Char('v') => app.start_visual_mode(),
                        KeyCode::Char('d') | KeyCode::Left | KeyCode::Char('h') => {
                            app.prev_details_tab();
                        }
                        KeyCode::Right | KeyCode::Char('l') => app.next_details_tab(),
                        KeyCode::Char('c') => app.show_changelog(),
                        KeyCode::Char('s') => app.show_settings(),
                        KeyCode::Char('u') => app.show_changes_preview(),
                        KeyCode::Char('x') => app.mark_all_upgrades(),
                        KeyCode::Char('N') => app.unmark_all(),
                        KeyCode::Char('r') => {
                            if let Err(e) = app.refresh_cache() {
                                app.status_message = format!("Refresh failed: {e}");
                            }
                        }
                        _ => {}
                    },
                    AppState::Searching => match key.code {
                        KeyCode::Esc => app.cancel_search(),
                        KeyCode::Enter => app.confirm_search(),
                        KeyCode::Backspace => {
                            app.core.search_query_pop();
                            app.execute_search();
                        }
                        KeyCode::Char(c) => {
                            app.core.search_query_push(c);
                            app.execute_search();
                        }
                        _ => {}
                    },
                    AppState::ShowingMarkConfirm => match key.code {
                        KeyCode::Char('y') | KeyCode::Enter | KeyCode::Char(' ') => {
                            app.confirm_mark();
                        }
                        KeyCode::Char('n') | KeyCode::Esc => app.cancel_mark(),
                        KeyCode::Up | KeyCode::Char('k') => app.scroll_mark_confirm(-1),
                        KeyCode::Down | KeyCode::Char('j') => app.scroll_mark_confirm(1),
                        KeyCode::PageUp => app.scroll_mark_confirm(-10),
                        KeyCode::PageDown => app.scroll_mark_confirm(10),
                        _ => {}
                    },
                    AppState::ShowingChanges => match key.code {
                        KeyCode::Char('y') | KeyCode::Enter => {
                            match app.apply_changes() {
                                ApplyResult::NeedsPassword => {
                                    // Will show password dialog
                                }
                                ApplyResult::NeedsCommit => {
                                    // Exit TUI, run commit, return
                                    disable_raw_mode()?;
                                    io::stdout().execute(LeaveAlternateScreen)?;

                                    if let Err(e) = app.commit_changes() {
                                        eprintln!("Error: {e}");
                                    }

                                    // Return to TUI
                                    enable_raw_mode()?;
                                    io::stdout().execute(EnterAlternateScreen)?;
                                    terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
                                }
                            }
                        }
                        KeyCode::Char('n') | KeyCode::Esc => {
                            app.state = AppState::Listing;
                        }
                        KeyCode::Up | KeyCode::Char('k') => app.scroll_changes(-1),
                        KeyCode::Down | KeyCode::Char('j') => app.scroll_changes(1),
                        KeyCode::PageUp => app.scroll_changes(-10),
                        KeyCode::PageDown => app.scroll_changes(10),
                        _ => {}
                    },
                    AppState::ShowingChangelog => match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => {
                            app.state = AppState::Listing;
                        }
                        KeyCode::Up | KeyCode::Char('k') => app.scroll_changelog(-1),
                        KeyCode::Down | KeyCode::Char('j') => app.scroll_changelog(1),
                        KeyCode::PageUp => app.scroll_changelog(-10),
                        KeyCode::PageDown => app.scroll_changelog(10),
                        _ => {}
                    },
                    AppState::ConfirmExit => match key.code {
                        KeyCode::Char('y') | KeyCode::Enter => break,
                        KeyCode::Char('n') | KeyCode::Esc => {
                            app.state = AppState::Listing;
                        }
                        _ => {}
                    },
                    AppState::EnteringPassword => match key.code {
                        KeyCode::Esc => {
                            app.sudo_password.zeroize();
                            app.state = AppState::ShowingChanges;
                        }
                        KeyCode::Enter => {
                            // Exit TUI, run sudo commit, return
                            disable_raw_mode()?;
                            io::stdout().execute(LeaveAlternateScreen)?;

                            if let Err(e) = app.commit_with_sudo() {
                                eprintln!("Error: {e}");
                            }

                            // Return to TUI
                            enable_raw_mode()?;
                            io::stdout().execute(EnterAlternateScreen)?;
                            terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
                        }
                        KeyCode::Backspace => {
                            app.sudo_password.pop();
                        }
                        KeyCode::Char(c) => {
                            app.sudo_password.push(c);
                        }
                        _ => {}
                    },
                    AppState::ShowingSettings => match key.code {
                        KeyCode::Esc | KeyCode::Char('q') => {
                            app.state = AppState::Listing;
                            app.apply_current_filter();
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            if app.settings_selection > 0 {
                                app.settings_selection -= 1;
                            }
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if app.settings_selection < App::settings_item_count() - 1 {
                                app.settings_selection += 1;
                            }
                        }
                        KeyCode::Enter | KeyCode::Char(' ') => {
                            app.toggle_setting();
                        }
                        _ => {}
                    },
                    AppState::Upgrading => {}
                    AppState::Done => match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char('r') => {
                            app.state = AppState::Listing;
                            if let Err(e) = app.refresh_cache() {
                                app.status_message = format!("Refresh failed: {e}");
                            }
                        }
                        _ => {}
                    },
                }
            }
    }

    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;

    Ok(())
}
