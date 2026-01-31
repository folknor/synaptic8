//! UI rendering functions

use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Table, Wrap,
};

use crate::app::App;
use crate::types::*;

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

    let changes_count = app.total_changes_count();
    let title_text = if changes_count > 0 {
        format!(
            " APT TUI │ {} changes │ {} download ",
            changes_count,
            PackageInfo::size_str(app.pending_changes.download_size)
        )
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
                    Constraint::Length(19),
                    Constraint::Min(40),
                    Constraint::Length(35),
                ])
                .split(main_chunks[1]);

            render_filter_pane(frame, app, panes[0]);
            render_package_table(frame, app, panes[1]);
            render_details_pane(frame, app, panes[2]);
        }
        AppState::ShowingMarkConfirm => {
            // Render the package list in background
            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(19),
                    Constraint::Min(40),
                    Constraint::Length(35),
                ])
                .split(main_chunks[1]);

            render_filter_pane(frame, app, panes[0]);
            render_package_table(frame, app, panes[1]);
            render_details_pane(frame, app, panes[2]);

            // Overlay the confirmation modal
            render_mark_confirm_modal(frame, app, main_chunks[1]);
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
        AppState::EnteringPassword => {
            render_password_input(frame, app, main_chunks[1]);
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
        AppState::EnteringPassword => Style::default().fg(Color::Yellow),
        AppState::Upgrading => Style::default().fg(Color::Cyan),
        AppState::Done => Style::default().fg(Color::Green),
    };
    let status_text = match app.state {
        AppState::Searching => format!("/{}_", app.search_query),
        AppState::ShowingMarkConfirm => format!(
            "Mark '{}' requires additional changes",
            app.mark_preview.package_name
        ),
        _ => {
            // Show active search filter in status if present
            if app.search_results.is_some() {
                format!("[Search: {}] {}", app.search_query, app.status_message)
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
            if app.visual_mode {
                "v/Space:Mark selected │ Esc:Cancel │ ↑↓:Extend selection"
            } else if app.search_results.is_some() {
                "/:Search │ Esc:Clear │ Space:Mark │ v:Visual │ x:All │ N:None │ u:Apply │ q:Quit"
            } else {
                "/:Search │ Space:Mark │ v:Visual │ x:All │ N:None │ d:Deps │ u:Apply │ q:Quit"
            }
        }
        AppState::Searching => "Enter:Confirm │ Esc:Cancel │ Type to search...",
        AppState::ShowingMarkConfirm => "y/Space/Enter:Confirm │ n/Esc:Cancel",
        AppState::ShowingChanges => "y/Enter:Apply │ n/Esc:Cancel │ ↑↓:Scroll",
        AppState::ShowingChangelog => "↑↓/PgUp/PgDn:Scroll │ Esc/q:Close",
        AppState::ShowingSettings => "↑↓:Navigate │ Space/Enter:Toggle │ Esc/q:Close",
        AppState::EnteringPassword => "Enter:Submit │ Esc:Cancel │ Type password...",
        AppState::Upgrading => "Applying changes...",
        AppState::Done => "r:Refresh │ q:Quit",
    };
    let help = Paragraph::new(help_text)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    frame.render_widget(help, main_chunks[3]);
}

fn render_filter_pane(frame: &mut Frame, app: &mut App, area: Rect) {
    let is_focused = app.focused_pane == FocusedPane::Filters;

    // Split into filters and legend
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(7), Constraint::Length(7)])
        .split(area);

    let items: Vec<ListItem> = FilterCategory::all()
        .iter()
        .map(|cat| {
            let style = if *cat == app.selected_filter {
                Style::default().fg(Color::Yellow).bold()
            } else {
                Style::default()
            };
            ListItem::new(cat.label()).style(style)
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

    frame.render_stateful_widget(list, chunks[0], &mut app.filter_state.clone());

    // Legend
    let legend = vec![
        Line::from(vec![
            Span::styled("↑", Style::default().fg(Color::Yellow)),
            Span::raw(" Upg avail"),
        ]),
        Line::from(vec![
            Span::styled("↑", Style::default().fg(Color::Green)),
            Span::raw(" Upgrading"),
        ]),
        Line::from(vec![
            Span::styled("+", Style::default().fg(Color::Green)),
            Span::raw(" Install"),
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
    let is_focused = app.focused_pane == FocusedPane::Packages;
    let visible_cols = Column::visible_columns(&app.settings);

    let header_cells: Vec<Cell> = visible_cols
        .iter()
        .map(|col| Cell::from(col.header()).style(Style::default().fg(Color::Cyan).bold()))
        .collect();
    let header = Row::new(header_cells).height(1);

    let rows: Vec<Row> = app
        .packages
        .iter()
        .enumerate()
        .map(|(idx, pkg)| {
            let is_multi_selected = app.multi_select.contains(&idx);

            let cells: Vec<Cell> = visible_cols
                .iter()
                .map(|col| match col {
                    Column::Status => Cell::from(pkg.status.symbol())
                        .style(Style::default().fg(pkg.status.color())),
                    Column::Name => {
                        let style = if pkg.is_user_marked {
                            Style::default().fg(Color::White).bold()
                        } else {
                            Style::default()
                        };
                        Cell::from(pkg.name.clone()).style(style)
                    }
                    Column::Section => Cell::from(pkg.section.clone()),
                    Column::InstalledVersion => {
                        if pkg.installed_version.is_empty() {
                            Cell::from("-")
                        } else {
                            Cell::from(pkg.installed_version.clone())
                        }
                    }
                    Column::CandidateVersion => Cell::from(pkg.candidate_version.clone())
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
                .title(format!(" Packages ({}) ", app.packages.len()))
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .row_highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(table, area, &mut app.table_state);

    if !app.packages.is_empty() {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));

        let mut scrollbar_state = ScrollbarState::new(app.packages.len())
            .position(app.table_state.selected().unwrap_or(0));

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
    let is_focused = app.focused_pane == FocusedPane::Details;

    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    // Tab header
    let info_style = if app.details_tab == DetailsTab::Info {
        Style::default().fg(Color::Yellow).bold()
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let deps_style = if app.details_tab == DetailsTab::Dependencies {
        Style::default().fg(Color::Yellow).bold()
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let rdeps_style = if app.details_tab == DetailsTab::ReverseDeps {
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
        match app.details_tab {
            DetailsTab::Info => {
                content.extend(vec![
                    Line::from(vec![
                        Span::styled("Package: ", Style::default().fg(Color::Cyan).bold()),
                        Span::raw(&pkg.name),
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
                    Line::from(pkg.description.clone()),
                ]);
            }
            DetailsTab::Dependencies => {
                if app.cached_deps.is_empty() {
                    content.push(Line::from(Span::styled(
                        "No dependencies",
                        Style::default().fg(Color::DarkGray),
                    )));
                } else {
                    // Group by dep type
                    let mut current_type = String::new();

                    for (dep_type, target) in &app.cached_deps {
                        if dep_type != &current_type {
                            if !current_type.is_empty() {
                                content.push(Line::from(""));
                            }
                            content.push(Line::from(Span::styled(
                                format!("{}:", dep_type),
                                Style::default().fg(Color::Cyan).bold(),
                            )));
                            current_type = dep_type.clone();
                        }

                        let status = app.get_package_status(target);
                        let symbol = status.symbol();
                        let color = status.color();

                        content.push(Line::from(vec![
                            Span::raw("  "),
                            Span::styled(format!("{:1}", symbol), Style::default().fg(color)),
                            Span::raw(" "),
                            Span::raw(target.clone()),
                        ]));
                    }
                }
            }
            DetailsTab::ReverseDeps => {
                if app.cached_rdeps.is_empty() {
                    content.push(Line::from(Span::styled(
                        "No reverse dependencies",
                        Style::default().fg(Color::DarkGray),
                    )));
                } else {
                    content.push(Line::from(Span::styled(
                        format!("{} packages depend on this:", app.cached_rdeps.len()),
                        Style::default().fg(Color::Cyan).bold(),
                    )));
                    content.push(Line::from(""));

                    // Group by dep type
                    let mut current_type = String::new();

                    for (dep_type, pkg_name) in &app.cached_rdeps {
                        if dep_type != &current_type {
                            if !current_type.is_empty() {
                                content.push(Line::from(""));
                            }
                            content.push(Line::from(Span::styled(
                                format!("{}:", dep_type),
                                Style::default().fg(Color::Cyan).bold(),
                            )));
                            current_type = dep_type.clone();
                        }

                        let status = app.get_package_status(pkg_name);
                        let symbol = status.symbol();
                        let color = status.color();

                        content.push(Line::from(vec![
                            Span::raw("  "),
                            Span::styled(format!("{:1}", symbol), Style::default().fg(color)),
                            Span::raw(" "),
                            Span::raw(pkg_name.clone()),
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

    let title = match app.details_tab {
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
        .scroll((app.detail_scroll, 0));

    frame.render_widget(details, area);
}

fn render_mark_confirm_modal(frame: &mut Frame, app: &App, area: Rect) {
    let preview = &app.mark_preview;

    // Build content first to calculate height
    let mut lines = vec![
        Line::from(Span::styled(
            "Mark this package requires additional changes:",
            Style::default().bold(),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Package: ", Style::default().fg(Color::Cyan)),
            Span::styled(&preview.package_name, Style::default().bold()),
        ]),
        Line::from(""),
    ];

    if !preview.additional_upgrades.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("The following {} packages will be UPGRADED:", preview.additional_upgrades.len()),
            Style::default().fg(Color::Cyan).bold(),
        )));
        for name in &preview.additional_upgrades {
            lines.push(Line::from(Span::styled(
                format!("  ↑ {}", name),
                Style::default().fg(Color::Cyan),
            )));
        }
        lines.push(Line::from(""));
    }

    if !preview.additional_installs.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("The following {} packages will be INSTALLED:", preview.additional_installs.len()),
            Style::default().fg(Color::Green).bold(),
        )));
        for name in &preview.additional_installs {
            lines.push(Line::from(Span::styled(
                format!("  + {}", name),
                Style::default().fg(Color::Green),
            )));
        }
        lines.push(Line::from(""));
    }

    if !preview.additional_removes.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("The following {} packages will be REMOVED:", preview.additional_removes.len()),
            Style::default().fg(Color::Red).bold(),
        )));
        for name in &preview.additional_removes {
            lines.push(Line::from(Span::styled(
                format!("  - {}", name),
                Style::default().fg(Color::Red),
            )));
        }
        lines.push(Line::from(""));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(format!(
        "Total download: {}",
        PackageInfo::size_str(preview.download_size)
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "y/Enter: Accept │ n/Esc: Cancel │ ↑↓: Scroll",
        Style::default().fg(Color::DarkGray),
    )));

    // Calculate max package name width for modal sizing
    let max_name_len = preview.additional_upgrades.iter()
        .chain(preview.additional_installs.iter())
        .chain(preview.additional_removes.iter())
        .map(|s| s.len())
        .max()
        .unwrap_or(0);

    // Calculate modal size based on content
    let content_height = lines.len() as u16 + 2; // +2 for borders
    // Width: at least 50, up to content width + padding, max 80% of screen
    let content_width = (max_name_len + 10).max(45) as u16;
    let modal_width = content_width.min(area.width * 8 / 10);
    let modal_height = content_height.min(area.height.saturating_sub(4));
    let modal_x = area.x + (area.width - modal_width) / 2;
    let modal_y = area.y + (area.height - modal_height) / 2;
    let modal_area = Rect::new(modal_x, modal_y, modal_width, modal_height);

    frame.render_widget(Clear, modal_area);

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Additional Changes Required ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Magenta)),
        )
        .scroll((app.mark_confirm_scroll, 0))
        .wrap(Wrap { trim: false });

    frame.render_widget(modal, modal_area);
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

    if !app.pending_changes.to_upgrade.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("UPGRADE ({}):", app.pending_changes.to_upgrade.len()),
            Style::default().fg(Color::Yellow).bold(),
        )));
        for name in &app.pending_changes.to_upgrade {
            lines.push(Line::from(format!("  ↑ {}", name)));
        }
        lines.push(Line::from(""));
    }

    if !app.pending_changes.to_install.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("INSTALL ({}):", app.pending_changes.to_install.len()),
            Style::default().fg(Color::Green).bold(),
        )));
        for name in &app.pending_changes.to_install {
            lines.push(Line::from(format!("  + {}", name)));
        }
        lines.push(Line::from(""));
    }

    if !app.pending_changes.auto_upgrade.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(
                "AUTO-UPGRADE (dependencies) ({}):",
                app.pending_changes.auto_upgrade.len()
            ),
            Style::default().fg(Color::Cyan).bold(),
        )));
        for name in &app.pending_changes.auto_upgrade {
            lines.push(Line::from(format!("  ↑ {}", name)));
        }
        lines.push(Line::from(""));
    }

    if !app.pending_changes.auto_install.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(
                "AUTO-INSTALL (dependencies) ({}):",
                app.pending_changes.auto_install.len()
            ),
            Style::default().fg(Color::Cyan).bold(),
        )));
        for name in &app.pending_changes.auto_install {
            lines.push(Line::from(format!("  + {}", name)));
        }
        lines.push(Line::from(""));
    }

    if !app.pending_changes.to_remove.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("REMOVE ({}):", app.pending_changes.to_remove.len()),
            Style::default().fg(Color::Red).bold(),
        )));
        for name in &app.pending_changes.to_remove {
            lines.push(Line::from(format!("  - {}", name)));
        }
        lines.push(Line::from(""));
    }

    if !app.pending_changes.auto_remove.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(
                "AUTO-REMOVE (no longer needed) ({}):",
                app.pending_changes.auto_remove.len()
            ),
            Style::default().fg(Color::Magenta).bold(),
        )));
        for name in &app.pending_changes.auto_remove {
            lines.push(Line::from(format!("  X {}", name)));
        }
        lines.push(Line::from(""));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(format!(
        "Download size: {}",
        PackageInfo::size_str(app.pending_changes.download_size)
    )));

    let size_change = if app.pending_changes.install_size_change >= 0 {
        format!(
            "+{}",
            PackageInfo::size_str(app.pending_changes.install_size_change as u64)
        )
    } else {
        format!(
            "-{}",
            PackageInfo::size_str((-app.pending_changes.install_size_change) as u64)
        )
    };
    lines.push(Line::from(format!("Disk space change: {}", size_change)));

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Confirm Changes ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.changes_scroll, 0));

    frame.render_widget(modal, modal_area);
}

fn render_changelog_view(frame: &mut Frame, app: &mut App, area: Rect) {
    let pkg_name = app
        .selected_package()
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "Unknown".to_string());

    let lines: Vec<Line> = app
        .changelog_content
        .iter()
        .map(|s| Line::from(s.as_str()))
        .collect();

    let changelog = Paragraph::new(lines)
        .block(
            Block::default()
                .title(format!(" Changelog: {} ", pkg_name))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.changelog_scroll, 0));

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

    let mut content = vec![
        Line::from(Span::styled(
            "Column Visibility",
            Style::default().fg(Color::Cyan).bold(),
        )),
        Line::from(""),
    ];

    // Column toggles (items 0-5)
    for (i, (name, enabled)) in column_items.iter().enumerate() {
        let checkbox = if *enabled { "[✓]" } else { "[ ]" };
        let style = if i == app.settings_selection {
            Style::default().fg(Color::Yellow).bold()
        } else {
            Style::default()
        };
        content.push(Line::from(vec![
            Span::styled(
                if i == app.settings_selection { "▶ " } else { "  " },
                style,
            ),
            Span::styled(checkbox, if *enabled { Style::default().fg(Color::Green) } else { Style::default().fg(Color::DarkGray) }),
            Span::raw(" "),
            Span::styled(*name, style),
        ]));
    }

    // Sort section
    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "Sorting",
        Style::default().fg(Color::Cyan).bold(),
    )));
    content.push(Line::from(""));

    // Sort by (item 6)
    let sort_style = if app.settings_selection == 6 {
        Style::default().fg(Color::Yellow).bold()
    } else {
        Style::default()
    };
    content.push(Line::from(vec![
        Span::styled(
            if app.settings_selection == 6 { "▶ " } else { "  " },
            sort_style,
        ),
        Span::styled("Sort by: ", sort_style),
        Span::styled(
            app.settings.sort_by.label(),
            Style::default().fg(Color::Green),
        ),
    ]));

    // Sort direction (item 7)
    let dir_style = if app.settings_selection == 7 {
        Style::default().fg(Color::Yellow).bold()
    } else {
        Style::default()
    };
    let dir_label = if app.settings.sort_ascending { "Ascending" } else { "Descending" };
    content.push(Line::from(vec![
        Span::styled(
            if app.settings_selection == 7 { "▶ " } else { "  " },
            dir_style,
        ),
        Span::styled("Direction: ", dir_style),
        Span::styled(dir_label, Style::default().fg(Color::Green)),
    ]));

    let settings = Paragraph::new(content)
        .block(
            Block::default()
                .title(" Settings ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        );

    frame.render_widget(settings, area);
}

fn render_password_input(frame: &mut Frame, app: &App, area: Rect) {
    let modal_width = 50.min(area.width.saturating_sub(4));
    let modal_height = 7;
    let modal_x = area.x + (area.width - modal_width) / 2;
    let modal_y = area.y + (area.height - modal_height) / 2;
    let modal_area = Rect::new(modal_x, modal_y, modal_width, modal_height);

    frame.render_widget(Clear, modal_area);

    // Show asterisks for password
    let password_display = "*".repeat(app.sudo_password.len());

    let content = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Enter sudo password:",
            Style::default().bold(),
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw("> "),
            Span::styled(&password_display, Style::default().fg(Color::Yellow)),
            Span::styled("_", Style::default().fg(Color::Yellow)),
        ]),
    ];

    let modal = Paragraph::new(content)
        .block(
            Block::default()
                .title(" Authentication Required ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        );

    frame.render_widget(modal, modal_area);
}
