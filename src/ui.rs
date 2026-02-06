//! UI rendering functions

use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Table, Wrap,
};

use crate::app::App;
use synh8::types::*;

pub fn ui(frame: &mut Frame, app: &mut App) {
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(10),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

    // Get download size from planned changes if available
    let download_size = app.core.planned_changes()
        .map(|changes| changes.iter().map(|c| c.download_size).sum::<u64>())
        .unwrap_or(0);

    let changes_count = app.total_changes_count();
    let title_text = if changes_count > 0 {
        format!(
            " APT TUI │ {} changes │ {} download ",
            changes_count,
            PackageInfo::size_str(download_size)
        )
    } else if app.core.has_marks() {
        format!(" APT TUI │ {} marked (press 'u' to compute) ",
            app.core.list().iter().filter(|p| app.core.is_user_marked(p.id)).count())
    } else {
        " APT TUI │ No changes pending ".to_string()
    };
    let title = Paragraph::new(title_text)
        .style(Style::default().fg(Color::White).bg(Color::Blue).bold());
    frame.render_widget(title, main_chunks[0]);

    match app.state {
        AppState::Listing | AppState::Searching => {
            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(24),
                    Constraint::Min(40),
                    Constraint::Length(35),
                ])
                .split(main_chunks[1]);

            render_filter_pane(frame, app, panes[0]);
            render_package_table(frame, app, panes[1]);
            render_details_pane(frame, app, panes[2]);
        }
        AppState::ShowingMarkConfirm => {
            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(24),
                    Constraint::Min(40),
                    Constraint::Length(35),
                ])
                .split(main_chunks[1]);

            render_filter_pane(frame, app, panes[0]);
            render_package_table(frame, app, panes[1]);
            render_details_pane(frame, app, panes[2]);

            // Render the mark preview modal on top
            render_mark_preview_modal(frame, app, main_chunks[1]);
        }
        AppState::ShowingChanges => {
            render_changes_modal(frame, app, main_chunks[1]);
        }
        AppState::ShowingChangelog => {
            render_changelog_view(frame, app, main_chunks[1]);
        }
        AppState::ShowingSettings => {
            render_settings_view(frame, app, main_chunks[1]);
        }
        AppState::ConfirmExit => {
            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(24),
                    Constraint::Min(40),
                    Constraint::Length(35),
                ])
                .split(main_chunks[1]);

            render_filter_pane(frame, app, panes[0]);
            render_package_table(frame, app, panes[1]);
            render_details_pane(frame, app, panes[2]);

            render_exit_confirm_modal(frame, app, main_chunks[1]);
        }
        AppState::Upgrading | AppState::Done => {
            let output_text = app.output_lines.join("\n");
            let output = Paragraph::new(output_text)
                .block(Block::default().title(" Output ").borders(Borders::ALL))
                .wrap(Wrap { trim: false });
            frame.render_widget(output, main_chunks[1]);
        }
    }

    let status_style = match app.state {
        AppState::Listing => Style::default().fg(Color::Yellow),
        AppState::Searching => Style::default().fg(Color::White),
        AppState::ShowingMarkConfirm => Style::default().fg(Color::Magenta),
        AppState::ShowingChanges => Style::default().fg(Color::Cyan),
        AppState::ShowingChangelog => Style::default().fg(Color::Cyan),
        AppState::ShowingSettings => Style::default().fg(Color::Yellow),
        AppState::ConfirmExit => Style::default().fg(Color::Red),
        AppState::Upgrading => Style::default().fg(Color::Cyan),
        AppState::Done => Style::default().fg(Color::Green),
    };

    let status_text = match app.state {
        AppState::Searching => format!("/{}_", app.core.search_query()),
        _ => {
            if app.core.search_result_count().is_some() {
                format!("[Search: {}] {}", app.core.search_query(), app.status_message)
            } else {
                app.status_message.clone()
            }
        }
    };
    let status = Paragraph::new(status_text)
        .style(status_style)
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(status, main_chunks[2]);

    let help_text = match app.state {
        AppState::Listing => {
            if app.ui.visual_mode {
                "v/Space:Mark selected │ Esc:Cancel │ ↑↓:Extend selection"
            } else if app.core.search_result_count().is_some() {
                "/:Search │ Esc:Clear │ Space:Mark │ v:Visual │ x:All │ N:None │ u:Apply │ U:Update │ q:Quit"
            } else {
                "/:Search │ Space:Mark │ v:Visual │ x:All │ N:None │ d:Deps │ s:Settings │ u:Apply │ U:Update │ q:Quit"
            }
        }
        AppState::Searching => "Enter:Confirm │ Esc:Cancel │ Type to search...",
        AppState::ShowingMarkConfirm => "y/Space/Enter:Confirm │ n/Esc:Cancel",
        AppState::ShowingChanges => "y/Enter:Apply │ n/Esc:Cancel │ ↑↓:Scroll",
        AppState::ShowingChangelog => "↑↓/PgUp/PgDn:Scroll │ Esc/q:Close",
        AppState::ShowingSettings => "↑↓:Navigate │ Space/Enter:Toggle │ Esc/q:Close",
        AppState::ConfirmExit => "y/Enter:Quit │ n/Esc:Cancel",
        AppState::Upgrading => "Applying changes...",
        AppState::Done => "r:Refresh │ q:Quit",
    };
    let help = Paragraph::new(help_text)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    frame.render_widget(help, main_chunks[3]);

    // Show cursor for text input states
    match app.state {
        AppState::Searching => {
            // Cursor after "/<query>" in the status bar (inside border: +1 x, +1 y)
            let cursor_x = main_chunks[2].x + 1 + 1 + app.core.search_query().len() as u16;
            let cursor_y = main_chunks[2].y + 1;
            frame.set_cursor_position((cursor_x, cursor_y));
        }
        _ => {}
    }
}

fn render_filter_pane(frame: &mut Frame, app: &mut App, area: Rect) {
    let is_focused = app.ui.focused_pane == FocusedPane::Filters;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(7), Constraint::Length(9)])
        .split(area);

    let items: Vec<ListItem> = FilterCategory::all()
        .iter()
        .map(|cat| {
            let count = app.core.filter_count(*cat);
            let label = format!("{} ({})", cat.label(), count);
            let style = if *cat == app.core.selected_filter() {
                Style::default().fg(Color::Yellow).bold()
            } else {
                Style::default()
            };
            ListItem::new(label).style(style)
        })
        .collect();

    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title(" Filters ")
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(list, chunks[0], &mut app.ui.filter_state);

    let legend = vec![
        Line::from(vec![
            Span::styled("↑", Style::default().fg(Color::Yellow)),
            Span::raw(" Upgradable"),
        ]),
        Line::from(vec![
            Span::styled("↑", Style::default().fg(Color::Green)),
            Span::raw(" Upgrade"),
        ]),
        Line::from(vec![
            Span::styled("↑", Style::default().fg(Color::Cyan)),
            Span::raw(" Auto-upg"),
        ]),
        Line::from(vec![
            Span::styled("+", Style::default().fg(Color::Green)),
            Span::raw(" Install"),
        ]),
        Line::from(vec![
            Span::styled("+", Style::default().fg(Color::Cyan)),
            Span::raw(" Auto-inst"),
        ]),
        Line::from(vec![
            Span::styled("-", Style::default().fg(Color::Red)),
            Span::raw(" Remove"),
        ]),
        Line::from(vec![
            Span::styled("·", Style::default().fg(Color::DarkGray)),
            Span::raw(" Installed"),
        ]),
    ];

    let legend_widget = Paragraph::new(legend)
        .block(
            Block::default()
                .title(" Legend ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        );

    frame.render_widget(legend_widget, chunks[1]);
}

fn render_package_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let is_focused = app.ui.focused_pane == FocusedPane::Packages;
    let visible_cols = Column::visible_columns(&app.settings);

    let header_cells: Vec<Cell> = visible_cols
        .iter()
        .map(|col| Cell::from(col.header()).style(Style::default().fg(Color::Cyan).bold()))
        .collect();
    let header = Row::new(header_cells).height(1);

    let list = app.core.list();
    let rows: Vec<Row> = list
        .iter()
        .enumerate()
        .map(|(idx, pkg)| {
            let is_multi_selected = app.ui.multi_select.contains(&idx);
            let is_user_marked = app.core.is_user_marked(pkg.id);

            let cells: Vec<Cell> = visible_cols
                .iter()
                .map(|col| match col {
                    Column::Status => Cell::from(pkg.status.symbol())
                        .style(Style::default().fg(pkg.status.color())),
                    Column::Name => {
                        let style = if is_user_marked {
                            Style::default().fg(Color::White).bold()
                        } else {
                            Style::default()
                        };
                        // Strip native arch suffix for cleaner display
                        let display_name = app.core.cache().display_name(&pkg.name);
                        Cell::from(display_name).style(style)
                    }
                    Column::Section => Cell::from(pkg.section.as_str()),
                    Column::InstalledVersion => {
                        if pkg.installed_version.is_empty() {
                            Cell::from("-")
                        } else {
                            Cell::from(pkg.installed_version.as_str())
                        }
                    }
                    Column::CandidateVersion => Cell::from(pkg.candidate_version.as_str())
                        .style(Style::default().fg(Color::Green)),
                    Column::DownloadSize => Cell::from(pkg.download_size_str()),
                })
                .collect();

            let row = Row::new(cells);
            if is_multi_selected {
                row.style(Style::default().bg(Color::Blue))
            } else {
                row
            }
        })
        .collect();

    let widths: Vec<Constraint> = visible_cols.iter().map(|col| col.width(&app.col_widths)).collect();

    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(format!(" Packages ({}) ", list.len()))
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .row_highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(table, area, &mut app.ui.table_state);

    if !list.is_empty() {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));

        let mut scrollbar_state = ScrollbarState::new(list.len())
            .position(app.ui.table_state.selected().unwrap_or(0));

        let scrollbar_area = Rect {
            x: area.x + area.width - 1,
            y: area.y + 1,
            width: 1,
            height: area.height.saturating_sub(2),
        };
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}

fn render_details_pane(frame: &mut Frame, app: &App, area: Rect) {
    let is_focused = app.ui.focused_pane == FocusedPane::Details;

    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let info_style = if app.details.tab == DetailsTab::Info {
        Style::default().fg(Color::Yellow).bold()
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let deps_style = if app.details.tab == DetailsTab::Dependencies {
        Style::default().fg(Color::Yellow).bold()
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let rdeps_style = if app.details.tab == DetailsTab::ReverseDeps {
        Style::default().fg(Color::Yellow).bold()
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let mut content = vec![
        Line::from(vec![
            Span::styled("[Info]", info_style),
            Span::raw(" "),
            Span::styled("[Deps]", deps_style),
            Span::raw(" "),
            Span::styled("[RDeps]", rdeps_style),
        ]),
        Line::from(Span::styled("  (d to switch)", Style::default().fg(Color::DarkGray))),
        Line::from(""),
    ];

    if let Some(pkg) = app.selected_package() {
        let display_name = app.core.cache().display_name(&pkg.name);
        match app.details.tab {
            DetailsTab::Info => {
                content.extend(vec![
                    Line::from(vec![
                        Span::styled("Package: ", Style::default().fg(Color::Cyan).bold()),
                        Span::raw(display_name),
                    ]),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("Status: ", Style::default().fg(Color::Cyan)),
                        Span::styled(pkg.status.symbol(), Style::default().fg(pkg.status.color())),
                        Span::raw(format!(" {:?}", pkg.status)),
                    ]),
                    Line::from(vec![
                        Span::styled("Section: ", Style::default().fg(Color::Cyan)),
                        Span::raw(&pkg.section),
                    ]),
                    Line::from(vec![
                        Span::styled("Arch: ", Style::default().fg(Color::Cyan)),
                        Span::raw(&pkg.architecture),
                    ]),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("Installed: ", Style::default().fg(Color::Cyan)),
                        Span::raw(if pkg.installed_version.is_empty() {
                            "(none)"
                        } else {
                            &pkg.installed_version
                        }),
                    ]),
                    Line::from(vec![
                        Span::styled("Candidate: ", Style::default().fg(Color::Green)),
                        Span::raw(&pkg.candidate_version),
                    ]),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("Download: ", Style::default().fg(Color::Cyan)),
                        Span::raw(pkg.download_size_str()),
                    ]),
                    Line::from(vec![
                        Span::styled("Inst Size: ", Style::default().fg(Color::Cyan)),
                        Span::raw(pkg.installed_size_str()),
                    ]),
                    Line::from(""),
                    Line::from(Span::styled(
                        "Description:",
                        Style::default().fg(Color::Cyan).bold(),
                    )),
                    Line::from(pkg.description.as_str()),
                ]);
            }
            DetailsTab::Dependencies => {
                if app.details.cached_deps.is_empty() {
                    content.push(Line::from(Span::styled(
                        "No dependencies",
                        Style::default().fg(Color::DarkGray),
                    )));
                } else {
                    let mut current_type = String::new();

                    for (dep_type, target) in &app.details.cached_deps {
                        if dep_type != &current_type {
                            if !current_type.is_empty() {
                                content.push(Line::from(""));
                            }
                            content.push(Line::from(Span::styled(
                                format!("{dep_type}:"),
                                Style::default().fg(Color::Cyan).bold(),
                            )));
                            current_type = dep_type.clone();
                        }

                        content.push(Line::from(vec![
                            Span::raw("  "),
                            Span::raw(target.as_str()),
                        ]));
                    }
                }
            }
            DetailsTab::ReverseDeps => {
                if app.details.cached_rdeps.is_empty() {
                    content.push(Line::from(Span::styled(
                        "No reverse dependencies",
                        Style::default().fg(Color::DarkGray),
                    )));
                } else {
                    content.push(Line::from(Span::styled(
                        format!("{} packages depend on this:", app.details.cached_rdeps.len()),
                        Style::default().fg(Color::Cyan).bold(),
                    )));
                    content.push(Line::from(""));

                    let mut current_type = String::new();

                    for (dep_type, pkg_name) in &app.details.cached_rdeps {
                        if dep_type != &current_type {
                            if !current_type.is_empty() {
                                content.push(Line::from(""));
                            }
                            content.push(Line::from(Span::styled(
                                format!("{dep_type}:"),
                                Style::default().fg(Color::Cyan).bold(),
                            )));
                            current_type = dep_type.clone();
                        }

                        content.push(Line::from(vec![
                            Span::raw("  "),
                            Span::raw(pkg_name.as_str()),
                        ]));
                    }
                }
            }
        }
    } else {
        content.push(Line::from(Span::styled(
            "No package selected",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let title = match app.details.tab {
        DetailsTab::Info => " Details ",
        DetailsTab::Dependencies => " Dependencies ",
        DetailsTab::ReverseDeps => " Reverse Deps ",
    };

    let details = Paragraph::new(content)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.details.scroll, 0));

    frame.render_widget(details, area);
}

fn render_changes_modal(frame: &mut Frame, app: &mut App, area: Rect) {
    let modal_width = 60.min(area.width.saturating_sub(4));
    let modal_height = 20.min(area.height.saturating_sub(2));
    let modal_x = area.x + (area.width - modal_width) / 2;
    let modal_y = area.y + (area.height - modal_height) / 2;
    let modal_area = Rect::new(modal_x, modal_y, modal_width, modal_height);

    frame.render_widget(Clear, modal_area);

    let mut lines = vec![
        Line::from(Span::styled(
            "The following changes will be made:",
            Style::default().bold(),
        )),
        Line::from(""),
    ];

    // Get planned changes - derive names from PackageId using cache
    if let Some(changes) = app.core.planned_changes() {
        let cache = app.core.cache();

        // Helper to get display name from PackageId (strips native arch suffix)
        let get_name = |c: &PlannedChange| -> String {
            cache.fullname_of(c.package)
                .map(|name| cache.display_name(name).to_string())
                .unwrap_or_else(|| format!("(unknown:{})", c.package.index()))
        };

        // Group by action and reason
        let user_upgrades: Vec<_> = changes.iter()
            .filter(|c| c.action == ChangeAction::Upgrade && c.reason == ChangeReason::UserRequested)
            .collect();
        let user_installs: Vec<_> = changes.iter()
            .filter(|c| c.action == ChangeAction::Install && c.reason == ChangeReason::UserRequested)
            .collect();
        let dep_upgrades: Vec<_> = changes.iter()
            .filter(|c| c.action == ChangeAction::Upgrade && c.reason == ChangeReason::Dependency)
            .collect();
        let dep_installs: Vec<_> = changes.iter()
            .filter(|c| c.action == ChangeAction::Install && c.reason == ChangeReason::Dependency)
            .collect();
        let user_removes: Vec<_> = changes.iter()
            .filter(|c| c.action == ChangeAction::Remove && c.reason == ChangeReason::UserRequested)
            .collect();
        let auto_removes: Vec<_> = changes.iter()
            .filter(|c| c.action == ChangeAction::Remove && c.reason == ChangeReason::AutoRemove)
            .collect();

        if !user_upgrades.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("UPGRADE ({}):", user_upgrades.len()),
                Style::default().fg(Color::Yellow).bold(),
            )));
            for c in &user_upgrades {
                lines.push(Line::from(format!("  ↑ {}", get_name(c))));
            }
            lines.push(Line::from(""));
        }

        if !user_installs.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("INSTALL ({}):", user_installs.len()),
                Style::default().fg(Color::Green).bold(),
            )));
            for c in &user_installs {
                lines.push(Line::from(format!("  + {}", get_name(c))));
            }
            lines.push(Line::from(""));
        }

        if !dep_upgrades.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("AUTO-UPGRADE (dependencies) ({}):", dep_upgrades.len()),
                Style::default().fg(Color::Cyan).bold(),
            )));
            for c in &dep_upgrades {
                lines.push(Line::from(format!("  ↑ {}", get_name(c))));
            }
            lines.push(Line::from(""));
        }

        if !dep_installs.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("AUTO-INSTALL (dependencies) ({}):", dep_installs.len()),
                Style::default().fg(Color::Cyan).bold(),
            )));
            for c in &dep_installs {
                lines.push(Line::from(format!("  + {}", get_name(c))));
            }
            lines.push(Line::from(""));
        }

        if !user_removes.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("REMOVE ({}):", user_removes.len()),
                Style::default().fg(Color::Red).bold(),
            )));
            for c in &user_removes {
                lines.push(Line::from(format!("  - {}", get_name(c))));
            }
            lines.push(Line::from(""));
        }

        if !auto_removes.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("AUTO-REMOVE (no longer needed) ({}):", auto_removes.len()),
                Style::default().fg(Color::Magenta).bold(),
            )));
            for c in &auto_removes {
                lines.push(Line::from(format!("  X {}", get_name(c))));
            }
            lines.push(Line::from(""));
        }

        // Download and size info
        let download_size: u64 = changes.iter().map(|c| c.download_size).sum();
        let size_change: i64 = changes.iter().map(|c| c.size_change).sum();

        lines.push(Line::from(""));
        lines.push(Line::from(format!(
            "Download size: {}",
            PackageInfo::size_str(download_size)
        )));

        let size_change_str = if size_change >= 0 {
            format!("+{}", PackageInfo::size_str(size_change as u64))
        } else {
            format!("-{}", PackageInfo::size_str((-size_change) as u64))
        };
        lines.push(Line::from(format!("Disk space change: {size_change_str}")));
    } else {
        lines.push(Line::from("No changes computed"));
    }

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Confirm Changes ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.modals.changes_scroll, 0));

    frame.render_widget(modal, modal_area);
}

fn render_changelog_view(frame: &mut Frame, app: &mut App, area: Rect) {
    let pkg_name = app
        .selected_package()
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "Unknown".to_string());

    let lines: Vec<Line> = app
        .modals.changelog_content
        .iter()
        .map(|s| Line::from(s.as_str()))
        .collect();

    let changelog = Paragraph::new(lines)
        .block(
            Block::default()
                .title(format!(" Changelog: {pkg_name} "))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.modals.changelog_scroll, 0));

    frame.render_widget(changelog, area);
}

fn render_settings_view(frame: &mut Frame, app: &mut App, area: Rect) {
    let column_items = [
        ("Status column (S)", app.settings.show_status_column),
        ("Name column", app.settings.show_name_column),
        ("Section column", app.settings.show_section_column),
        ("Installed version column", app.settings.show_installed_version_column),
        ("Candidate version column", app.settings.show_candidate_version_column),
        ("Download size column", app.settings.show_download_size_column),
    ];

    let mut items: Vec<ListItem> = column_items
        .iter()
        .enumerate()
        .map(|(idx, (label, enabled))| {
            let checkbox = if *enabled { "[X]" } else { "[ ]" };
            let text = format!("{checkbox} {label}");
            let style = if idx == app.settings_selection {
                Style::default().bg(Color::DarkGray)
            } else {
                Style::default()
            };
            ListItem::new(text).style(style)
        })
        .collect();

    // Add sort options
    items.push(ListItem::new(""));
    let sort_style = if app.settings_selection == 6 {
        Style::default().bg(Color::DarkGray)
    } else {
        Style::default()
    };
    items.push(ListItem::new(format!("Sort by: {}", app.settings.sort_by.label())).style(sort_style));

    let order_style = if app.settings_selection == 7 {
        Style::default().bg(Color::DarkGray)
    } else {
        Style::default()
    };
    let order = if app.settings.sort_ascending { "Ascending" } else { "Descending" };
    items.push(ListItem::new(format!("Sort order: {order}")).style(order_style));

    let settings_list = List::new(items)
        .block(
            Block::default()
                .title(" Settings ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        );

    frame.render_widget(settings_list, area);
}

fn render_mark_preview_modal(frame: &mut Frame, app: &App, area: Rect) {
    let Some(ref preview) = app.mark_preview else {
        return;
    };

    let modal_width = 60.min(area.width.saturating_sub(4));
    let modal_height = 20.min(area.height.saturating_sub(4));
    let modal_x = area.x + (area.width - modal_width) / 2;
    let modal_y = area.y + (area.height - modal_height) / 2;
    let modal_area = Rect::new(modal_x, modal_y, modal_width, modal_height);

    frame.render_widget(Clear, modal_area);

    let mut lines = Vec::new();

    if preview.is_marking {
        // MARK operation
        let action = if preview.is_upgrade { "upgrade" } else { "install" };
        lines.push(Line::from(Span::styled(
            format!("Mark '{}' for {}?", preview.package_name, action),
            Style::default().bold(),
        )));
        lines.push(Line::from(""));

        if !preview.additional_installs.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("Will install {} additional packages:", preview.additional_installs.len()),
                Style::default().fg(Color::Green),
            )));
            for name in &preview.additional_installs {
                lines.push(Line::from(format!("  + {name}")));
            }
            lines.push(Line::from(""));
        }

        if !preview.additional_upgrades.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("Will upgrade {} packages:", preview.additional_upgrades.len()),
                Style::default().fg(Color::Yellow),
            )));
            for name in &preview.additional_upgrades {
                lines.push(Line::from(format!("  ^ {name}")));
            }
            lines.push(Line::from(""));
        }

        if !preview.additional_removes.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("Will remove {} packages:", preview.additional_removes.len()),
                Style::default().fg(Color::Red),
            )));
            for name in &preview.additional_removes {
                lines.push(Line::from(format!("  - {name}")));
            }
            lines.push(Line::from(""));
        }

        lines.push(Line::from(Span::styled(
            format!("Download size: {}", PackageInfo::size_str(preview.download_size)),
            Style::default().fg(Color::Cyan),
        )));
    } else {
        // UNMARK operation
        lines.push(Line::from(Span::styled(
            format!("Unmark '{}'?", preview.package_name),
            Style::default().bold(),
        )));
        lines.push(Line::from(""));

        // additional_upgrades is repurposed to hold "also unmarked" packages
        if !preview.additional_upgrades.is_empty() {
            lines.push(Line::from(Span::styled(
                format!("This will also unmark {} packages:", preview.additional_upgrades.len()),
                Style::default().fg(Color::Yellow),
            )));
            for name in &preview.additional_upgrades {
                lines.push(Line::from(format!("  {name}")));
            }
        }
    }

    // Apply scroll offset
    let visible_lines: Vec<Line> = lines
        .into_iter()
        .skip(app.mark_preview_scroll)
        .collect();

    let title = if preview.is_marking {
        " Confirm Package Mark "
    } else {
        " Confirm Package Unmark "
    };

    let modal = Paragraph::new(visible_lines)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Magenta)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(modal, modal_area);

    // Hint at bottom
    let hint_area = Rect::new(
        modal_area.x,
        modal_area.y + modal_area.height - 1,
        modal_area.width,
        1,
    );
    let hint = Paragraph::new(Span::styled(
        " y/Enter: Confirm │ n/Esc: Cancel │ j/k: Scroll ",
        Style::default().fg(Color::DarkGray),
    ))
    .alignment(Alignment::Center);
    frame.render_widget(hint, hint_area);
}

fn render_exit_confirm_modal(frame: &mut Frame, _app: &App, area: Rect) {
    let modal_width = 50.min(area.width.saturating_sub(4));
    let modal_height = 7;
    let modal_x = area.x + (area.width - modal_width) / 2;
    let modal_y = area.y + (area.height - modal_height) / 2;
    let modal_area = Rect::new(modal_x, modal_y, modal_width, modal_height);

    frame.render_widget(Clear, modal_area);

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "You have unsaved changes!",
            Style::default().fg(Color::Red).bold(),
        )),
        Line::from(""),
        Line::from("Really quit without applying?"),
        Line::from(""),
        Line::from(Span::styled(
            "y/Enter: Quit │ n/Esc: Cancel",
            Style::default().fg(Color::DarkGray),
        )),
    ];

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Confirm Exit ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Red)),
        )
        .alignment(Alignment::Center);

    frame.render_widget(modal, modal_area);
}

