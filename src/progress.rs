//! TUI progress display for APT download and install operations.
//!
//! Uses `Rc<RefCell<ProgressState>>` to share terminal access between
//! `TuiAcquireProgress` (download phase) and `TuiInstallProgress` (install phase).
//! Both phases render into the same ratatui terminal.

use std::cell::RefCell;
use std::io::Stdout;
use std::rc::Rc;

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Gauge, Paragraph, Wrap};
use ratatui::Terminal;
use rust_apt::raw::{AcqTextStatus, ItemDesc, PkgAcquire};

use crate::types::PackageInfo;

// ============================================================================
// Types
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressPhase {
    Downloading,
    Installing,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemStatus {
    Hit,
    Fetch,
    Done,
    Fail,
}

#[derive(Debug, Clone)]
pub struct ProgressItem {
    pub label: String,
    pub short_desc: String,
    pub status: ItemStatus,
    pub error: Option<String>,
}

/// Shared progress state, owned by `Rc<RefCell<_>>`.
pub struct ProgressState {
    pub terminal: Terminal<CrosstermBackend<Stdout>>,
    pub phase: ProgressPhase,
    // Download phase
    pub percent: f64,
    pub current_bytes: u64,
    pub total_bytes: u64,
    pub speed_bps: u64,
    pub items: Vec<ProgressItem>,
    // Install phase
    pub install_steps_done: u64,
    pub install_total_steps: u64,
    pub install_action: String,
    // Shared
    pub errors: Vec<String>,
    /// Title shown in the progress box
    pub title: String,
}

const MAX_ITEMS: usize = 50;

impl ProgressState {
    pub fn new(terminal: Terminal<CrosstermBackend<Stdout>>, title: &str) -> Self {
        Self {
            terminal,
            phase: ProgressPhase::Downloading,
            percent: 0.0,
            current_bytes: 0,
            total_bytes: 0,
            speed_bps: 0,
            items: Vec::new(),
            install_steps_done: 0,
            install_total_steps: 0,
            install_action: String::new(),
            errors: Vec::new(),
            title: title.to_string(),
        }
    }

    fn push_item(&mut self, item: ProgressItem) {
        self.items.push(item);
        if self.items.len() > MAX_ITEMS {
            self.items.remove(0);
        }
    }

    fn draw(&mut self) {
        // Borrow fields individually so the closure doesn't capture &mut self
        let phase = self.phase;
        let percent = self.percent;
        let current_bytes = self.current_bytes;
        let total_bytes = self.total_bytes;
        let speed_bps = self.speed_bps;
        let items = &self.items;
        let install_steps_done = self.install_steps_done;
        let install_total_steps = self.install_total_steps;
        let install_action = &self.install_action;
        let errors = &self.errors;
        let title = &self.title;

        // Ignore draw errors — terminal may be in a weird state during dpkg
        drop(self.terminal.draw(|frame| {
            render_progress(
                frame,
                phase,
                percent,
                current_bytes,
                total_bytes,
                speed_bps,
                items,
                install_steps_done,
                install_total_steps,
                install_action,
                errors,
                title,
            );
        }));
    }
}

// ============================================================================
// DynAcquireProgress implementation
// ============================================================================

pub struct TuiAcquireProgress {
    state: Rc<RefCell<ProgressState>>,
}

impl TuiAcquireProgress {
    pub fn new(state: Rc<RefCell<ProgressState>>) -> Self {
        Self { state }
    }
}

impl rust_apt::progress::DynAcquireProgress for TuiAcquireProgress {
    fn pulse_interval(&self) -> usize {
        500_000 // 500ms
    }

    fn hit(&mut self, item: &ItemDesc) {
        let mut state = self.state.borrow_mut();
        state.push_item(ProgressItem {
            label: format!("Hit:{}", item.owner().id()),
            short_desc: item.short_desc(),
            status: ItemStatus::Hit,
            error: None,
        });
    }

    fn fetch(&mut self, item: &ItemDesc) {
        let mut state = self.state.borrow_mut();
        state.push_item(ProgressItem {
            label: format!("Get:{}", item.owner().id()),
            short_desc: item.short_desc(),
            status: ItemStatus::Fetch,
            error: None,
        });
    }

    fn done(&mut self, item: &ItemDesc) {
        let mut state = self.state.borrow_mut();
        state.push_item(ProgressItem {
            label: format!("Done:{}", item.owner().id()),
            short_desc: item.short_desc(),
            status: ItemStatus::Done,
            error: None,
        });
    }

    fn fail(&mut self, item: &ItemDesc) {
        let owner = item.owner();
        let error_text = owner.error_text();
        let mut state = self.state.borrow_mut();
        state.push_item(ProgressItem {
            label: format!("Err:{}", owner.id()),
            short_desc: item.short_desc(),
            status: ItemStatus::Fail,
            error: Some(error_text.clone()),
        });
        if !error_text.is_empty() {
            state.errors.push(format!("{}: {}", item.short_desc(), error_text));
        }
    }

    fn pulse(&mut self, status: &AcqTextStatus, _owner: &PkgAcquire) {
        let mut state = self.state.borrow_mut();
        state.percent = status.percent();
        state.current_bytes = status.current_bytes();
        state.total_bytes = status.total_bytes();
        state.speed_bps = status.current_cps();
        state.draw();
    }

    fn start(&mut self) {
        let mut state = self.state.borrow_mut();
        state.phase = ProgressPhase::Downloading;
        state.draw();
    }

    fn stop(&mut self, _status: &AcqTextStatus) {
        // Phase transition handled externally
    }
}

// ============================================================================
// DynInstallProgress implementation
// ============================================================================

pub struct TuiInstallProgress {
    state: Rc<RefCell<ProgressState>>,
}

impl TuiInstallProgress {
    pub fn new(state: Rc<RefCell<ProgressState>>) -> Self {
        Self { state }
    }
}

impl rust_apt::progress::DynInstallProgress for TuiInstallProgress {
    fn status_changed(
        &mut self,
        pkgname: String,
        steps_done: u64,
        total_steps: u64,
        action: String,
    ) {
        let mut state = self.state.borrow_mut();
        state.phase = ProgressPhase::Installing;
        state.install_steps_done = steps_done;
        state.install_total_steps = total_steps;
        state.install_action = if pkgname.is_empty() {
            action
        } else {
            format!("{action} {pkgname}")
        };
        state.draw();
    }

    fn error(&mut self, pkgname: String, _steps_done: u64, _total_steps: u64, error: String) {
        let mut state = self.state.borrow_mut();
        state.errors.push(format!("{pkgname}: {error}"));
        state.draw();
    }
}

// ============================================================================
// Rendering
// ============================================================================

#[allow(clippy::too_many_arguments)]
fn render_progress(
    frame: &mut Frame,
    phase: ProgressPhase,
    percent: f64,
    current_bytes: u64,
    total_bytes: u64,
    speed_bps: u64,
    items: &[ProgressItem],
    install_steps_done: u64,
    install_total_steps: u64,
    install_action: &str,
    errors: &[String],
    title: &str,
) {
    let area = frame.area();

    // Outer block
    let block = Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Split inner into: header(1), gauge(1), gap(1), log(rest), errors(if any)
    let error_height = if errors.is_empty() { 0 } else { (errors.len() as u16).min(4) + 1 };
    let constraints = if error_height > 0 {
        vec![
            Constraint::Length(1), // phase header
            Constraint::Length(1), // progress bar
            Constraint::Length(1), // byte counter / step counter
            Constraint::Length(1), // gap
            Constraint::Min(3),    // log area
            Constraint::Length(error_height), // errors
        ]
    } else {
        vec![
            Constraint::Length(1), // phase header
            Constraint::Length(1), // progress bar
            Constraint::Length(1), // byte counter / step counter
            Constraint::Length(1), // gap
            Constraint::Min(3),    // log area
        ]
    };
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    match phase {
        ProgressPhase::Downloading => {
            // Header line: "Downloading packages...  45%  2.1 MB/s"
            let speed_str = if speed_bps > 0 {
                format!("  {}/s", PackageInfo::size_str(speed_bps))
            } else {
                String::new()
            };
            let header = Line::from(vec![
                Span::styled("Downloading packages... ", Style::default().fg(Color::Cyan)),
                Span::styled(format!("{percent:.0}%"), Style::default().fg(Color::White).bold()),
                Span::styled(speed_str, Style::default().fg(Color::DarkGray)),
            ]);
            frame.render_widget(Paragraph::new(header), chunks[0]);

            // Progress bar
            let ratio = (percent / 100.0).clamp(0.0, 1.0);
            let gauge = Gauge::default()
                .gauge_style(Style::default().fg(Color::Cyan).bg(Color::DarkGray))
                .ratio(ratio);
            frame.render_widget(gauge, chunks[1]);

            // Byte counter
            let counter = Line::from(Span::styled(
                format!(
                    "{} / {}",
                    PackageInfo::size_str(current_bytes),
                    PackageInfo::size_str(total_bytes),
                ),
                Style::default().fg(Color::DarkGray),
            ));
            frame.render_widget(Paragraph::new(counter), chunks[2]);
        }
        ProgressPhase::Installing => {
            // Header line: "Installing packages...  Step 14 / 38"
            let header = Line::from(vec![
                Span::styled("Installing packages... ", Style::default().fg(Color::Green)),
                Span::styled(
                    format!("Step {install_steps_done} / {install_total_steps}"),
                    Style::default().fg(Color::White).bold(),
                ),
            ]);
            frame.render_widget(Paragraph::new(header), chunks[0]);

            // Progress bar
            let ratio = if install_total_steps > 0 {
                (install_steps_done as f64 / install_total_steps as f64).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let gauge = Gauge::default()
                .gauge_style(Style::default().fg(Color::Green).bg(Color::DarkGray))
                .ratio(ratio);
            frame.render_widget(gauge, chunks[1]);

            // Current action
            let action = Line::from(Span::styled(
                install_action,
                Style::default().fg(Color::DarkGray),
            ));
            frame.render_widget(Paragraph::new(action), chunks[2]);
        }
        ProgressPhase::Done => {
            let header = Line::from(Span::styled(
                "Complete.",
                Style::default().fg(Color::Green).bold(),
            ));
            frame.render_widget(Paragraph::new(header), chunks[0]);

            let gauge = Gauge::default()
                .gauge_style(Style::default().fg(Color::Green).bg(Color::DarkGray))
                .ratio(1.0);
            frame.render_widget(gauge, chunks[1]);
        }
    }

    // Log area — show recent items, bottom-aligned
    let log_height = chunks[4].height as usize;
    let visible_items = if items.len() > log_height {
        &items[items.len() - log_height..]
    } else {
        items
    };
    let log_lines: Vec<Line> = visible_items
        .iter()
        .map(|item| {
            let (prefix_style, desc_style) = match item.status {
                ItemStatus::Hit => (
                    Style::default().fg(Color::Green),
                    Style::default().fg(Color::DarkGray),
                ),
                ItemStatus::Fetch => (
                    Style::default().fg(Color::Cyan),
                    Style::default().fg(Color::White),
                ),
                ItemStatus::Done => (
                    Style::default().fg(Color::Green),
                    Style::default().fg(Color::DarkGray),
                ),
                ItemStatus::Fail => (
                    Style::default().fg(Color::Red),
                    Style::default().fg(Color::Red),
                ),
            };
            Line::from(vec![
                Span::styled(format!("{} ", item.label), prefix_style),
                Span::styled(&item.short_desc, desc_style),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(log_lines), chunks[4]);

    // Errors area (if any)
    if error_height > 0 && chunks.len() > 5 {
        let error_lines: Vec<Line> = errors
            .iter()
            .rev()
            .take(4)
            .rev()
            .map(|e| Line::from(Span::styled(e.as_str(), Style::default().fg(Color::Red))))
            .collect();
        let error_block = Block::default()
            .title(" Errors ")
            .borders(Borders::TOP)
            .border_style(Style::default().fg(Color::Red));
        let error_para = Paragraph::new(error_lines)
            .block(error_block)
            .wrap(Wrap { trim: false });
        frame.render_widget(error_para, chunks[5]);
    }
}
