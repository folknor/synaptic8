//! TUI progress display for APT download and install operations.
//!
//! Uses `Rc<RefCell<ProgressState>>` to share terminal access between
//! `TuiAcquireProgress` (download phase) and `TuiInstallProgress` (install phase).
//! Both phases render into the same ratatui terminal as a centered modal.
//!
//! The progress terminal writes to `/dev/tty` directly, while `StdioRedirect`
//! redirects fd 1/2 to `/dev/null` so dpkg's stdout output is suppressed.

use std::cell::RefCell;
use std::fs::File;
use std::rc::Rc;

use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Gauge, Paragraph, Wrap};
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

/// RAII guard that redirects stdout/stderr to a temp file.
/// Restores the original file descriptors when dropped.
pub struct StdioRedirect {
    saved_stdout: libc::c_int,
    saved_stderr: libc::c_int,
    capture_path: std::path::PathBuf,
}

impl StdioRedirect {
    /// Redirect stdout and stderr to a temp file for later retrieval.
    pub fn capture() -> std::io::Result<Self> {
        use std::os::unix::io::AsRawFd;

        let capture_path = std::env::temp_dir().join("synh8-apt-output.tmp");
        unsafe {
            let saved_stdout = libc::dup(libc::STDOUT_FILENO);
            let saved_stderr = libc::dup(libc::STDERR_FILENO);

            let file = std::fs::File::create(&capture_path)?;
            let capture_fd = file.as_raw_fd();
            libc::dup2(capture_fd, libc::STDOUT_FILENO);
            libc::dup2(capture_fd, libc::STDERR_FILENO);
            // file drops here closing capture_fd, but the dup'd fds keep it open

            Ok(Self { saved_stdout, saved_stderr, capture_path })
        }
    }

    /// Read the captured output.
    pub fn output(&self) -> Vec<String> {
        // Flush C stdio buffers so all libapt output is written to the file
        unsafe { libc::fflush(std::ptr::null_mut()); }
        std::fs::read_to_string(&self.capture_path)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }
}

impl Drop for StdioRedirect {
    fn drop(&mut self) {
        unsafe {
            libc::dup2(self.saved_stdout, libc::STDOUT_FILENO);
            libc::dup2(self.saved_stderr, libc::STDERR_FILENO);
            libc::close(self.saved_stdout);
            libc::close(self.saved_stderr);
        }
        drop(std::fs::remove_file(&self.capture_path));
    }
}

/// Shared progress state, owned by `Rc<RefCell<_>>`.
///
/// The terminal writes to `/dev/tty` directly, bypassing stdout.
/// This allows dpkg output (which goes to fd 1) to be suppressed via
/// `StdioRedirect` without affecting progress rendering.
pub struct ProgressState {
    terminal: Terminal<CrosstermBackend<File>>,
    pub phase: ProgressPhase,
    // Download phase
    pub percent: f64,
    pub current_bytes: u64,
    pub total_bytes: u64,
    pub speed_bps: u64,
    // Install phase
    pub install_steps_done: u64,
    pub install_total_steps: u64,
    pub install_action: String,
    // Shared
    pub errors: Vec<String>,
    /// Title shown in the modal border
    pub title: String,
}

impl ProgressState {
    pub fn new(title: &str) -> std::io::Result<Self> {
        let tty = std::fs::OpenOptions::new()
            .write(true)
            .open("/dev/tty")?;
        let backend = CrosstermBackend::new(tty);
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            phase: ProgressPhase::Downloading,
            percent: 0.0,
            current_bytes: 0,
            total_bytes: 0,
            speed_bps: 0,
            install_steps_done: 0,
            install_total_steps: 0,
            install_action: String::new(),
            errors: Vec::new(),
            title: title.to_string(),
        })
    }

    fn draw(&mut self) {
        // Borrow fields individually so the closure doesn't capture &mut self
        let phase = self.phase;
        let percent = self.percent;
        let current_bytes = self.current_bytes;
        let total_bytes = self.total_bytes;
        let speed_bps = self.speed_bps;
        let install_steps_done = self.install_steps_done;
        let install_total_steps = self.install_total_steps;
        let install_action = &self.install_action;
        let errors = &self.errors;
        let title = &self.title;

        drop(self.terminal.draw(|frame| {
            render_progress_modal(
                frame,
                phase,
                percent,
                current_bytes,
                total_bytes,
                speed_bps,
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

    fn hit(&mut self, _item: &ItemDesc) {}

    fn fetch(&mut self, _item: &ItemDesc) {}

    fn done(&mut self, _item: &ItemDesc) {}

    fn fail(&mut self, item: &ItemDesc) {
        let owner = item.owner();
        let error_text = owner.error_text();
        if !error_text.is_empty() {
            let mut state = self.state.borrow_mut();
            state.errors.push(format!("{}: {error_text}", item.short_desc()));
            state.draw();
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
// Rendering — compact centered modal
// ============================================================================

#[allow(clippy::too_many_arguments)]
fn render_progress_modal(
    frame: &mut Frame,
    phase: ProgressPhase,
    percent: f64,
    current_bytes: u64,
    total_bytes: u64,
    speed_bps: u64,
    install_steps_done: u64,
    install_total_steps: u64,
    install_action: &str,
    errors: &[String],
    title: &str,
) {
    let area = frame.area();

    // Modal size: roomy when no errors, expands to show errors
    let modal_width = 70.min(area.width.saturating_sub(4));
    // border(1) + pad(1) + status(1) + pad(1) + gauge(1) + pad(1) + detail(1) + pad(1) + border(1)
    let base_height: u16 = 9;
    let error_height = if errors.is_empty() {
        0
    } else {
        // 1 for separator + up to 4 error lines
        1 + (errors.len() as u16).min(4)
    };
    let modal_height = (base_height + error_height).min(area.height.saturating_sub(2));

    let modal_x = area.x + (area.width - modal_width) / 2;
    let modal_y = area.y + (area.height - modal_height) / 2;
    let modal_area = Rect::new(modal_x, modal_y, modal_width, modal_height);

    frame.render_widget(Clear, modal_area);

    let border_color = match phase {
        ProgressPhase::Downloading => Color::Cyan,
        ProgressPhase::Installing => Color::Green,
        ProgressPhase::Done => Color::Green,
    };
    let block = Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(modal_area);
    frame.render_widget(block, modal_area);

    // Inner layout: pad, status, pad, gauge, pad, detail, pad, [errors]
    let mut constraints = vec![
        Constraint::Length(1), // top padding
        Constraint::Length(1), // status line
        Constraint::Length(1), // spacing
        Constraint::Length(1), // progress bar
        Constraint::Length(1), // spacing
        Constraint::Length(1), // detail line (bytes or action)
        Constraint::Length(1), // bottom padding
    ];
    if error_height > 0 {
        constraints.push(Constraint::Min(error_height));
    }
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    match phase {
        ProgressPhase::Downloading => {
            // Status: "Downloading...  45%  2.1 MB/s"
            let speed_str = if speed_bps > 0 {
                format!("  {}/s", PackageInfo::size_str(speed_bps))
            } else {
                String::new()
            };
            let status = Line::from(vec![
                Span::styled("Downloading... ", Style::default().fg(Color::Cyan)),
                Span::styled(format!("{percent:.0}%"), Style::default().fg(Color::White).bold()),
                Span::styled(speed_str, Style::default().fg(Color::DarkGray)),
            ]);
            frame.render_widget(Paragraph::new(status), chunks[1]);

            // Gauge
            let ratio = (percent / 100.0).clamp(0.0, 1.0);
            let gauge = Gauge::default()
                .gauge_style(Style::default().fg(Color::Cyan).bg(Color::DarkGray))
                .ratio(ratio);
            frame.render_widget(gauge, chunks[3]);

            // Detail: byte counter
            let detail = Line::from(Span::styled(
                format!(
                    "{} / {}",
                    PackageInfo::size_str(current_bytes),
                    PackageInfo::size_str(total_bytes),
                ),
                Style::default().fg(Color::DarkGray),
            ));
            frame.render_widget(Paragraph::new(detail), chunks[5]);
        }
        ProgressPhase::Installing => {
            // Status: "Installing...  Step 14 / 38"
            let status = Line::from(vec![
                Span::styled("Installing... ", Style::default().fg(Color::Green)),
                Span::styled(
                    format!("Step {install_steps_done} / {install_total_steps}"),
                    Style::default().fg(Color::White).bold(),
                ),
            ]);
            frame.render_widget(Paragraph::new(status), chunks[1]);

            // Gauge
            let ratio = if install_total_steps > 0 {
                (install_steps_done as f64 / install_total_steps as f64).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let gauge = Gauge::default()
                .gauge_style(Style::default().fg(Color::Green).bg(Color::DarkGray))
                .ratio(ratio);
            frame.render_widget(gauge, chunks[3]);

            // Detail: current action
            let detail = Line::from(Span::styled(
                install_action,
                Style::default().fg(Color::DarkGray),
            ));
            frame.render_widget(Paragraph::new(detail), chunks[5]);
        }
        ProgressPhase::Done => {
            let status = Line::from(Span::styled(
                "Complete.",
                Style::default().fg(Color::Green).bold(),
            ));
            frame.render_widget(Paragraph::new(status), chunks[1]);

            let gauge = Gauge::default()
                .gauge_style(Style::default().fg(Color::Green).bg(Color::DarkGray))
                .ratio(1.0);
            frame.render_widget(gauge, chunks[3]);
        }
    }

    // Errors section (only shown when errors exist)
    if error_height > 0 && chunks.len() > 7 {
        let error_lines: Vec<Line> = errors
            .iter()
            .rev()
            .take(4)
            .rev()
            .map(|e| Line::from(Span::styled(e.as_str(), Style::default().fg(Color::Red))))
            .collect();
        let error_para = Paragraph::new(error_lines)
            .wrap(Wrap { trim: false });
        frame.render_widget(error_para, chunks[7]);
    }
}
