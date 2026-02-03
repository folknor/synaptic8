//! TUI application state and logic
//!
//! This module contains TUI-specific state and acts as an adapter between
//! the core business logic (PackageManager) and the ratatui UI.

use std::collections::HashSet;

use color_eyre::Result;
use ratatui::widgets::{ListState, TableState};
use zeroize::Zeroize;

use crate::core::{PackageManager, ToggleResult};
use crate::types::*;

/// UI widget state for the main views
pub struct UiState {
    pub table_state: TableState,
    pub filter_state: ListState,
    pub focused_pane: FocusedPane,
    pub multi_select: HashSet<usize>,
    pub selection_anchor: Option<usize>,
    pub visual_mode: bool,
}

/// Details pane state and cached data
pub struct DetailsState {
    pub scroll: u16,
    pub tab: DetailsTab,
    pub cached_deps: Vec<(String, String)>,
    pub cached_rdeps: Vec<(String, String)>,
    pub cached_pkg_name: String,
}

impl Default for DetailsState {
    fn default() -> Self {
        Self {
            scroll: 0,
            tab: DetailsTab::Info,
            cached_deps: Vec::new(),
            cached_rdeps: Vec::new(),
            cached_pkg_name: String::new(),
        }
    }
}

/// Modal/popup scroll positions and content
#[derive(Default)]
pub struct ModalState {
    pub mark_confirm_scroll: u16,
    pub changes_scroll: u16,
    pub changelog_scroll: u16,
    pub changelog_content: Vec<String>,
}

/// TUI Application - wraps core PackageManager with UI state
pub struct App {
    /// Core business logic (UI-agnostic)
    pub core: PackageManager,

    /// TUI-specific state
    pub ui: UiState,
    pub details: DetailsState,
    pub modals: ModalState,
    pub state: AppState,
    pub settings: Settings,
    pub settings_selection: usize,
    pub col_widths: ColumnWidths,
    pub status_message: String,
    pub output_lines: Vec<String>,
    pub sudo_password: String,
}

impl App {
    pub fn new() -> Result<Self> {
        let core = PackageManager::new()?;
        let mut filter_state = ListState::default();
        filter_state.select(Some(0));

        let settings = Settings::default();
        let mut app = Self {
            core,
            ui: UiState {
                table_state: TableState::default(),
                filter_state,
                focused_pane: FocusedPane::Packages,
                multi_select: HashSet::new(),
                selection_anchor: None,
                visual_mode: false,
            },
            details: DetailsState::default(),
            modals: ModalState::default(),
            state: AppState::Listing,
            settings,
            settings_selection: 0,
            col_widths: ColumnWidths::new(),
            status_message: String::from("Loading..."),
            output_lines: Vec::new(),
            sudo_password: String::new(),
        };

        // Sync sort settings from UI settings to core
        app.core.set_sort(app.settings.sort_by, app.settings.sort_ascending);
        app.refresh_ui_state();
        app.update_status_message();
        Ok(app)
    }

    /// Refresh UI state after core changes, preserving selection by package name
    fn refresh_ui_state(&mut self) {
        let selected_name = self.selected_package().map(|p| p.name.clone());
        self.col_widths = self.core.rebuild_list();
        self.restore_selection(selected_name);
        self.update_cached_deps();
    }

    /// Restore selection by package name, or reset to 0 if not found
    fn restore_selection(&mut self, package_name: Option<String>) {
        self.ui.multi_select.clear();
        self.ui.selection_anchor = None;
        self.ui.visual_mode = false;

        let new_idx = package_name
            .and_then(|name| self.core.list.iter().position(|p| p.name == name))
            .unwrap_or(0);

        self.ui.table_state.select(if self.core.package_count() > 0 {
            Some(new_idx)
        } else {
            None
        });
    }

    /// Reset UI selection state to beginning
    fn reset_selection(&mut self) {
        self.restore_selection(None);
    }

    // === Accessors ===

    pub fn selected_package(&self) -> Option<&PackageInfo> {
        self.ui.table_state
            .selected()
            .and_then(|i| self.core.get_package(i))
    }

    pub fn get_package_status(&self, name: &str) -> PackageStatus {
        self.core.get_package_status(name)
    }

    #[must_use]
    pub fn has_pending_changes(&self) -> bool {
        self.core.has_pending_changes()
    }

    #[must_use]
    pub fn total_changes_count(&self) -> usize {
        self.core.total_changes_count()
    }

    // === Dependency caching (TUI optimization) ===

    pub fn update_cached_deps(&mut self) {
        let pkg_name = self.selected_package()
            .map(|p| p.name.clone())
            .unwrap_or_default();

        if pkg_name == self.details.cached_pkg_name {
            return;
        }
        self.details.cached_pkg_name = pkg_name.clone();
        self.details.cached_deps = self.core.get_dependencies(&pkg_name);
        self.details.cached_rdeps = self.core.get_reverse_dependencies(&pkg_name);
    }

    // === Search ===

    pub fn start_search(&mut self) {
        match self.core.ensure_search_index() {
            Ok(duration) => {
                if duration.as_millis() > 0 {
                    self.status_message = format!(
                        "Search index built in {:.0}ms",
                        duration.as_secs_f64() * 1000.0
                    );
                }
            }
            Err(e) => {
                self.status_message = format!("Failed to build search index: {}", e);
                return;
            }
        }
        self.state = AppState::Searching;
    }

    pub fn execute_search(&mut self) {
        if let Err(e) = self.core.set_search_query(&self.core.search.query.clone()) {
            self.status_message = format!("Search error: {}", e);
        }
        self.refresh_ui_state();
    }

    pub fn cancel_search(&mut self) {
        self.core.clear_search();
        self.state = AppState::Listing;
        self.refresh_ui_state();
        self.update_status_message();
    }

    pub fn confirm_search(&mut self) {
        self.state = AppState::Listing;
        if let Some(count) = self.core.search_result_count() {
            self.status_message = format!(
                "Found {} packages matching '{}'",
                count,
                self.core.search_query()
            );
        }
    }

    // === Filter ===

    pub fn apply_current_filter(&mut self) {
        self.core.apply_filter(self.core.selected_filter);
        self.col_widths = self.core.rebuild_list();
        self.reset_selection();
    }

    pub fn move_filter_selection(&mut self, delta: i32) {
        let filters = FilterCategory::all();
        let current = self.ui.filter_state.selected().unwrap_or(0) as i32;
        let new_idx = (current + delta).clamp(0, filters.len() as i32 - 1) as usize;
        self.ui.filter_state.select(Some(new_idx));
        self.core.apply_filter(filters[new_idx]);
        self.refresh_ui_state();
    }

    // === Package marking ===

    pub fn toggle_current(&mut self) {
        if let Some(pkg) = self.selected_package() {
            let name = pkg.name.clone();
            match self.core.toggle_package(&name) {
                ToggleResult::Unmarked => {
                    self.refresh_ui_state();
                    self.update_status_message();
                }
                ToggleResult::NeedsPreview => {
                    self.modals.mark_confirm_scroll = 0;
                    self.state = AppState::ShowingMarkConfirm;
                }
                ToggleResult::MarkedDirectly => {
                    self.refresh_ui_state();
                    self.update_status_message();
                }
                ToggleResult::NotMarkable => {}
                ToggleResult::Error(e) => {
                    self.status_message = e;
                }
            }
        }
    }

    pub fn confirm_mark(&mut self) {
        self.core.confirm_mark();
        self.refresh_ui_state();
        self.update_status_message();
        self.state = AppState::Listing;
    }

    pub fn cancel_mark(&mut self) {
        self.core.cancel_mark();
        self.update_status_message();
        self.state = AppState::Listing;
    }

    pub fn mark_all_upgrades(&mut self) {
        if let Err(e) = self.core.mark_all_upgrades() {
            self.status_message = e;
            return;
        }
        self.refresh_ui_state();
        self.update_status_message();
        self.show_changes_preview();
    }

    pub fn unmark_all(&mut self) {
        self.core.unmark_all();
        self.refresh_ui_state();
        self.update_status_message();
    }

    // === Visual mode ===

    pub fn start_visual_mode(&mut self) {
        let current_idx = self.ui.table_state.selected().unwrap_or(0);

        if !self.ui.visual_mode {
            self.ui.visual_mode = true;
            self.ui.selection_anchor = Some(current_idx);
            self.ui.multi_select.clear();
            self.ui.multi_select.insert(current_idx);
            self.status_message = "-- VISUAL -- (move to select, v/Space to mark, Esc to cancel)".to_string();
        } else {
            self.mark_selected_packages();
        }
    }

    pub fn update_visual_selection(&mut self) {
        if !self.ui.visual_mode {
            return;
        }

        let current_idx = self.ui.table_state.selected().unwrap_or(0);
        if let Some(anchor) = self.ui.selection_anchor {
            let start = anchor.min(current_idx);
            let end = anchor.max(current_idx);

            self.ui.multi_select.clear();
            for idx in start..=end {
                self.ui.multi_select.insert(idx);
            }
        }
    }

    pub fn cancel_visual_mode(&mut self) {
        self.ui.visual_mode = false;
        self.ui.multi_select.clear();
        self.ui.selection_anchor = None;
        self.update_status_message();
    }

    pub fn toggle_multi_select(&mut self) {
        if !self.ui.visual_mode {
            self.start_visual_mode();
        } else {
            self.mark_selected_packages();
        }
    }

    fn mark_selected_packages(&mut self) {
        let packages_to_mark: Vec<String> = self.ui.multi_select.iter()
            .filter_map(|&idx| self.core.get_package(idx))
            .map(|p| p.name.clone())
            .collect();

        if let Err(e) = self.core.mark_packages(&packages_to_mark) {
            self.status_message = e;
        }

        self.ui.multi_select.clear();
        self.ui.selection_anchor = None;
        self.ui.visual_mode = false;

        self.refresh_ui_state();
        self.update_status_message();

        if self.has_pending_changes() {
            self.show_changes_preview();
        }
    }

    // === Navigation ===

    pub fn move_package_selection(&mut self, delta: i32) {
        if self.core.package_count() == 0 {
            return;
        }
        let current = self.ui.table_state.selected().unwrap_or(0) as i32;
        let new_idx = (current + delta).clamp(0, self.core.package_count() as i32 - 1) as usize;
        self.ui.table_state.select(Some(new_idx));
        self.details.scroll = 0;
        self.update_cached_deps();
    }

    pub fn next_details_tab(&mut self) {
        self.details.tab = match self.details.tab {
            DetailsTab::Info => DetailsTab::Dependencies,
            DetailsTab::Dependencies => DetailsTab::ReverseDeps,
            DetailsTab::ReverseDeps => DetailsTab::Info,
        };
        self.details.scroll = 0;
    }

    pub fn prev_details_tab(&mut self) {
        self.details.tab = match self.details.tab {
            DetailsTab::Info => DetailsTab::ReverseDeps,
            DetailsTab::Dependencies => DetailsTab::Info,
            DetailsTab::ReverseDeps => DetailsTab::Dependencies,
        };
        self.details.scroll = 0;
    }

    pub fn cycle_focus(&mut self) {
        self.ui.focused_pane = match self.ui.focused_pane {
            FocusedPane::Filters => FocusedPane::Packages,
            FocusedPane::Packages => FocusedPane::Details,
            FocusedPane::Details => FocusedPane::Filters,
        };
    }

    pub fn cycle_focus_back(&mut self) {
        self.ui.focused_pane = match self.ui.focused_pane {
            FocusedPane::Filters => FocusedPane::Details,
            FocusedPane::Packages => FocusedPane::Filters,
            FocusedPane::Details => FocusedPane::Packages,
        };
    }

    // === Modals ===

    pub fn show_changelog(&mut self) {
        let pkg_name = match self.selected_package() {
            Some(p) => p.name.clone(),
            None => {
                self.status_message = "No package selected".to_string();
                return;
            }
        };

        self.modals.changelog_content.clear();
        self.modals.changelog_content.push(format!("Loading changelog for {}...", pkg_name));
        self.modals.changelog_scroll = 0;

        match self.core.fetch_changelog(&pkg_name) {
            Ok(lines) => {
                self.modals.changelog_content = lines;
            }
            Err(e) => {
                self.modals.changelog_content.clear();
                self.modals.changelog_content.push(e);
            }
        }

        self.state = AppState::ShowingChangelog;
    }

    pub fn show_settings(&mut self) {
        self.settings_selection = 0;
        self.state = AppState::ShowingSettings;
    }

    pub fn toggle_setting(&mut self) {
        match self.settings_selection {
            0 => self.settings.show_status_column = !self.settings.show_status_column,
            1 => self.settings.show_name_column = !self.settings.show_name_column,
            2 => self.settings.show_section_column = !self.settings.show_section_column,
            3 => self.settings.show_installed_version_column = !self.settings.show_installed_version_column,
            4 => self.settings.show_candidate_version_column = !self.settings.show_candidate_version_column,
            5 => self.settings.show_download_size_column = !self.settings.show_download_size_column,
            6 => {
                let all = SortBy::all();
                let idx = all.iter().position(|&s| s == self.settings.sort_by).unwrap_or(0);
                self.settings.sort_by = all[(idx + 1) % all.len()];
                self.core.set_sort(self.settings.sort_by, self.settings.sort_ascending);
                self.col_widths = self.core.rebuild_list();
            }
            7 => {
                self.settings.sort_ascending = !self.settings.sort_ascending;
                self.core.set_sort(self.settings.sort_by, self.settings.sort_ascending);
                self.col_widths = self.core.rebuild_list();
            }
            _ => {}
        }
    }

    pub fn settings_item_count() -> usize {
        8
    }

    pub fn show_changes_preview(&mut self) {
        if self.has_pending_changes() {
            self.state = AppState::ShowingChanges;
            self.modals.changes_scroll = 0;
        } else {
            self.status_message = "No changes to apply".to_string();
        }
    }

    // === Scrolling ===

    pub fn scroll_changelog(&mut self, delta: i32) {
        let max_scroll = self.modals.changelog_content.len().saturating_sub(1) as u16;
        let current = self.modals.changelog_scroll as i32;
        self.modals.changelog_scroll = (current + delta).clamp(0, max_scroll as i32) as u16;
    }

    pub fn scroll_changes(&mut self, delta: i32) {
        let max_scroll = self.changes_line_count().saturating_sub(5) as u16;
        let current = self.modals.changes_scroll as i32;
        self.modals.changes_scroll = (current + delta).clamp(0, max_scroll as i32) as u16;
    }

    pub fn scroll_mark_confirm(&mut self, delta: i32) {
        let max_scroll = self.mark_confirm_line_count().saturating_sub(5) as u16;
        let current = self.modals.mark_confirm_scroll as i32;
        self.modals.mark_confirm_scroll = (current + delta).clamp(0, max_scroll as i32) as u16;
    }

    pub fn changes_line_count(&self) -> usize {
        let pending = &self.core.pending;
        let mut count = 2;
        if !pending.to_upgrade.is_empty() { count += 2 + pending.to_upgrade.len(); }
        if !pending.to_install.is_empty() { count += 2 + pending.to_install.len(); }
        if !pending.auto_upgrade.is_empty() { count += 2 + pending.auto_upgrade.len(); }
        if !pending.auto_install.is_empty() { count += 2 + pending.auto_install.len(); }
        if !pending.to_remove.is_empty() { count += 2 + pending.to_remove.len(); }
        if !pending.auto_remove.is_empty() { count += 2 + pending.auto_remove.len(); }
        count + 4
    }

    pub fn mark_confirm_line_count(&self) -> usize {
        let preview = &self.core.mark_preview;
        let mut count = 4;
        count += preview.additional_upgrades.len();
        count += preview.additional_installs.len();
        count += preview.additional_removes.len();
        count + 4
    }

    // === Status message ===

    pub fn update_status_message(&mut self) {
        let changes = self.total_changes_count();

        if changes > 0 {
            self.status_message = format!(
                "{} changes pending ({} download) | {} upgradable | Press 'u' to review",
                changes,
                PackageInfo::size_str(self.core.pending.download_size),
                self.core.upgradable_count
            );
        } else {
            self.status_message = format!("{} packages upgradable", self.core.upgradable_count);
        }
    }

    // === System operations ===

    #[must_use]
    pub fn is_root() -> bool {
        PackageManager::is_root()
    }

    pub fn apply_changes(&mut self) -> ApplyResult {
        if !Self::is_root() {
            self.sudo_password.zeroize();
            self.state = AppState::EnteringPassword;
            return ApplyResult::NeedsPassword;
        }

        self.state = AppState::Upgrading;
        ApplyResult::NeedsCommit
    }

    pub fn commit_changes(&mut self) -> Result<()> {
        println!("\n=== Applying changes ===\n");

        match self.core.commit() {
            Ok(()) => {
                println!("\n=== Changes applied successfully ===");
                self.output_lines.push("Changes applied successfully.".to_string());
                self.state = AppState::Done;
                self.status_message = "Done. Press 'q' to quit or 'r' to refresh.".to_string();
            }
            Err(e) => {
                println!("\n=== Error applying changes ===");
                println!("{}", e);
                self.output_lines.push(format!("Error: {}", e));
                self.state = AppState::Done;
                self.status_message = format!("Error: {}. Press 'q' to quit or 'r' to refresh.", e);
            }
        }

        Ok(())
    }

    pub fn commit_with_sudo(&mut self) -> Result<()> {
        use std::io::{Read, Write};
        use std::process::{Command, Stdio};

        println!("\n=== Applying changes with sudo ===\n");

        let mut args = vec!["apt-get".to_string(), "-y".to_string()];

        let pending = &self.core.pending;
        let to_install: Vec<&str> = pending.to_install.iter()
            .chain(pending.to_upgrade.iter())
            .chain(pending.auto_install.iter())
            .chain(pending.auto_upgrade.iter())
            .map(|s| s.as_str())
            .collect();

        let to_remove: Vec<String> = pending.to_remove.iter()
            .chain(pending.auto_remove.iter())
            .map(|s| format!("{}-", s))
            .collect();

        if !to_install.is_empty() || !to_remove.is_empty() {
            args.push("install".to_string());
            for pkg in &to_install {
                args.push(pkg.to_string());
            }
            for pkg in &to_remove {
                args.push(pkg.clone());
            }
        }

        let mut child = Command::new("/usr/bin/sudo")
            .arg("-S")
            .args(&args)
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            writeln!(stdin, "{}", self.sudo_password)?;
        }

        self.sudo_password.zeroize();

        let mut stderr_output = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut stderr_output);
        }

        let status = child.wait()?;

        if status.success() {
            println!("\n=== Changes applied successfully ===");
            self.output_lines.push("Changes applied successfully.".to_string());
            self.state = AppState::Done;
            self.status_message = "Done. Press 'q' to quit or 'r' to refresh.".to_string();
        } else {
            println!("\n=== Error applying changes ===");
            if !stderr_output.is_empty() {
                let filtered_err: String = stderr_output
                    .lines()
                    .filter(|line| !line.contains("[sudo] password"))
                    .collect::<Vec<_>>()
                    .join("\n");
                if !filtered_err.is_empty() {
                    println!("{}", filtered_err);
                    self.output_lines.push(format!("Error: {}", filtered_err.lines().next().unwrap_or("apt-get failed")));
                } else {
                    self.output_lines.push("Error: apt-get failed (wrong password?)".to_string());
                }
            } else {
                self.output_lines.push("Error: apt-get failed".to_string());
            }
            self.state = AppState::Done;
            self.status_message = "Error: apt-get failed. Press 'q' to quit or 'r' to refresh.".to_string();
        }

        self.core.apt.clear_user_marks();

        Ok(())
    }

    pub fn refresh_cache(&mut self) -> Result<()> {
        if let Some(msg) = PackageManager::check_apt_lock() {
            self.status_message = msg;
            return Ok(());
        }

        if let Err(e) = self.core.refresh() {
            self.status_message = format!("Refresh failed: {}", e);
            return Ok(());
        }

        self.refresh_ui_state();
        self.update_status_message();
        Ok(())
    }
}
