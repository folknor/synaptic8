//! Application state and logic

use std::collections::HashSet;

use color_eyre::Result;
use ratatui::widgets::{ListState, TableState};
use rust_apt::cache::PackageSort;
use zeroize::Zeroize;

use crate::apt::AptManager;
use crate::search::SearchIndex;
use crate::types::*;

/// Package management state - APT cache and marking state
pub struct PackageState {
    pub apt: AptManager,
    pub list: Vec<PackageInfo>,
    pub pending: PendingChanges,
    pub mark_preview: MarkPreview,
    pub upgradable_count: usize,
}

/// UI widget state for the main views
pub struct UiState {
    pub table_state: TableState,
    pub filter_state: ListState,
    pub focused_pane: FocusedPane,
    pub selected_filter: FilterCategory,
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

/// Full-text search state
#[derive(Default)]
pub struct SearchState {
    pub index: Option<SearchIndex>,
    pub query: String,
    pub results: Option<HashSet<String>>,
    pub build_time: Option<std::time::Duration>,
}

/// Modal/popup scroll positions and content
#[derive(Default)]
pub struct ModalState {
    pub mark_confirm_scroll: u16,
    pub changes_scroll: u16,
    pub changelog_scroll: u16,
    pub changelog_content: Vec<String>,
}

pub struct App {
    // Grouped state
    pub pkg: PackageState,
    pub ui: UiState,
    pub details: DetailsState,
    pub search: SearchState,
    pub modals: ModalState,

    // Global state
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
        let apt = AptManager::new()?;
        let mut filter_state = ListState::default();
        filter_state.select(Some(0));

        let mut app = Self {
            pkg: PackageState {
                apt,
                list: Vec::new(),
                pending: PendingChanges::default(),
                mark_preview: MarkPreview::default(),
                upgradable_count: 0,
            },
            ui: UiState {
                table_state: TableState::default(),
                filter_state,
                focused_pane: FocusedPane::Packages,
                selected_filter: FilterCategory::Upgradable,
                multi_select: HashSet::new(),
                selection_anchor: None,
                visual_mode: false,
            },
            details: DetailsState::default(),
            search: SearchState::default(),
            modals: ModalState::default(),
            state: AppState::Listing,
            settings: Settings::default(),
            settings_selection: 0,
            col_widths: ColumnWidths::new(),
            status_message: String::from("Loading..."),
            output_lines: Vec::new(),
            sudo_password: String::new(),
        };

        app.update_upgradable_count();
        app.reload_packages()?;
        Ok(app)
    }

    pub fn update_cached_deps(&mut self) {
        let pkg_name = self
            .selected_package()
            .map(|p| p.name.clone())
            .unwrap_or_default();

        // Only recalculate if selection changed
        if pkg_name == self.details.cached_pkg_name {
            return;
        }
        self.details.cached_pkg_name = pkg_name.clone();

        // Use AptManager to get dependencies
        self.details.cached_deps = self.pkg.apt.get_dependencies(&pkg_name);
        self.details.cached_rdeps = self.pkg.apt.get_reverse_dependencies(&pkg_name);
    }

    /// Get status for a package by name (for dep views)
    pub fn get_package_status(&self, name: &str) -> PackageStatus {
        self.pkg.apt.get_package_status(name)
    }

    pub fn ensure_search_index(&mut self) -> Result<()> {
        if self.search.index.is_none() {
            let mut index = SearchIndex::new()?;
            let (count, duration) = index.build(&self.pkg.apt)?;
            self.search.build_time = Some(duration);
            self.status_message = format!(
                "Search index built: {} packages in {:.0}ms",
                count,
                duration.as_secs_f64() * 1000.0
            );
            self.search.index = Some(index);
        }
        Ok(())
    }

    pub fn start_search(&mut self) {
        if let Err(e) = self.ensure_search_index() {
            self.status_message = format!("Failed to build search index: {}", e);
            return;
        }
        self.search.query.clear();
        self.state = AppState::Searching;
    }

    pub fn execute_search(&mut self) {
        if self.search.query.is_empty() {
            self.search.results = None;
        } else if let Some(ref index) = self.search.index {
            match index.search(&self.search.query) {
                Ok(results) => {
                    self.search.results = Some(results);
                }
                Err(e) => {
                    self.status_message = format!("Search error: {}", e);
                    self.search.results = None;
                }
            }
        }
        self.apply_current_filter();
        self.ui.table_state.select(if self.pkg.list.is_empty() {
            None
        } else {
            Some(0)
        });
    }

    pub fn cancel_search(&mut self) {
        self.search.query.clear();
        self.search.results = None;
        self.state = AppState::Listing;
        self.apply_current_filter();
        self.update_status_message();
    }

    pub fn confirm_search(&mut self) {
        self.execute_search();
        self.state = AppState::Listing;
        if let Some(ref results) = self.search.results {
            self.status_message = format!(
                "Found {} packages matching '{}'",
                results.len(),
                self.search.query
            );
        }
    }

    pub fn reload_packages(&mut self) -> Result<()> {
        self.pkg.list.clear();
        self.apply_current_filter();

        if !self.pkg.list.is_empty() {
            self.ui.table_state.select(Some(0));
        }

        self.update_cached_deps();
        self.update_status_message();
        Ok(())
    }

    pub fn apply_current_filter(&mut self) {
        self.pkg.list.clear();
        self.ui.multi_select.clear();
        self.ui.selection_anchor = None;
        self.ui.visual_mode = false;

        // Reset column widths to header minimums
        self.col_widths.reset();

        let sort = if self.ui.selected_filter == FilterCategory::Upgradable {
            PackageSort::default().upgradable()
        } else {
            PackageSort::default()
        };

        for pkg in self.pkg.apt.packages(&sort) {
            // Apply category filter
            let matches_category = match self.ui.selected_filter {
                FilterCategory::Upgradable => pkg.is_upgradable(),
                FilterCategory::MarkedChanges => {
                    pkg.marked_install() || pkg.marked_delete() || pkg.marked_upgrade()
                }
                FilterCategory::Installed => pkg.is_installed(),
                FilterCategory::NotInstalled => !pkg.is_installed(),
                FilterCategory::All => true,
            };

            // Apply search filter if active
            let matches_search = match &self.search.results {
                Some(results) => results.contains(pkg.name()),
                None => true,
            };

            if matches_category && matches_search {
                if let Some(info) = self.pkg.apt.extract_package_info(&pkg) {
                    // Track max column widths
                    self.col_widths.name = self.col_widths.name.max(info.name.len() as u16);
                    self.col_widths.section = self.col_widths.section.max(info.section.len() as u16);
                    self.col_widths.installed = self.col_widths.installed.max(info.installed_version.len() as u16);
                    self.col_widths.candidate = self.col_widths.candidate.max(info.candidate_version.len() as u16);
                    self.pkg.list.push(info);
                }
            }
        }

        // Sort packages
        let cmp = |a: &PackageInfo, b: &PackageInfo| {
            let ord = match self.settings.sort_by {
                SortBy::Name => a.name.cmp(&b.name),
                SortBy::Section => a.section.cmp(&b.section),
                SortBy::InstalledVersion => a.installed_version.cmp(&b.installed_version),
                SortBy::CandidateVersion => a.candidate_version.cmp(&b.candidate_version),
            };
            if self.settings.sort_ascending { ord } else { ord.reverse() }
        };
        self.pkg.list.sort_by(cmp);
    }

    pub fn selected_package(&self) -> Option<&PackageInfo> {
        self.ui.table_state
            .selected()
            .and_then(|i| self.pkg.list.get(i))
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

        // Run apt changelog command
        match std::process::Command::new("apt")
            .args(["changelog", &pkg_name])
            .output()
        {
            Ok(output) => {
                self.modals.changelog_content.clear();
                if output.status.success() {
                    let content = String::from_utf8_lossy(&output.stdout);
                    for line in content.lines() {
                        self.modals.changelog_content.push(line.to_string());
                    }
                    if self.modals.changelog_content.is_empty() {
                        self.modals.changelog_content.push("No changelog available.".to_string());
                    }
                } else {
                    let err = String::from_utf8_lossy(&output.stderr);
                    self.modals.changelog_content.push(format!("Error: {}", err));
                }
            }
            Err(e) => {
                self.modals.changelog_content.clear();
                self.modals.changelog_content.push(format!("Failed to run apt changelog: {}", e));
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
            // Sort options - cycle through
            6 => {
                let all = SortBy::all();
                let idx = all.iter().position(|&s| s == self.settings.sort_by).unwrap_or(0);
                self.settings.sort_by = all[(idx + 1) % all.len()];
                self.apply_current_filter();
            }
            7 => {
                self.settings.sort_ascending = !self.settings.sort_ascending;
                self.apply_current_filter();
            }
            _ => {}
        }
    }

    pub fn settings_item_count() -> usize {
        8 // 6 column toggles + 2 sort options
    }

    pub fn toggle_current(&mut self) {
        if let Some(i) = self.ui.table_state.selected() {
            if let Some(pkg_info) = self.pkg.list.get(i) {
                let pkg_name = pkg_info.name.clone();
                self.toggle_package(&pkg_name);
            }
        }
    }

    fn toggle_package(&mut self, name: &str) {
        if let Some(pkg) = self.pkg.apt.get(name) {
            let currently_marked = pkg.marked_install() || pkg.marked_upgrade();
            let is_upgradable = pkg.is_upgradable();
            let is_installed = pkg.is_installed();
            drop(pkg); // Release borrow before mutating

            if currently_marked {
                // Unmarking - just do it directly (no confirmation needed)
                self.pkg.apt.mark_keep(name);

                if let Err(e) = self.pkg.apt.resolve() {
                    self.status_message = format!("Dependency error: {}", e);
                }

                self.calculate_pending_changes();
                self.apply_current_filter();
                self.update_status_message();
            } else if is_upgradable || !is_installed {
                // Marking - preview additional changes first
                self.preview_mark(name);
            }
        }
    }

    /// Start or extend visual selection mode
    pub fn start_visual_mode(&mut self) {
        let current_idx = self.ui.table_state.selected().unwrap_or(0);

        if !self.ui.visual_mode {
            // Start visual mode
            self.ui.visual_mode = true;
            self.ui.selection_anchor = Some(current_idx);
            self.ui.multi_select.clear();
            self.ui.multi_select.insert(current_idx);
            self.status_message = "-- VISUAL -- (move to select, v/Space to mark, Esc to cancel)".to_string();
        } else {
            // Already in visual mode - mark selected and exit
            self.mark_selected_packages();
        }
    }

    /// Update visual selection when cursor moves
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

    /// Cancel visual mode without marking
    pub fn cancel_visual_mode(&mut self) {
        self.ui.visual_mode = false;
        self.ui.multi_select.clear();
        self.ui.selection_anchor = None;
        self.update_status_message();
    }

    /// Handle Shift+Space for multi-select (alternative to 'v')
    pub fn toggle_multi_select(&mut self) {
        if !self.ui.visual_mode {
            // Start visual mode
            self.start_visual_mode();
        } else {
            // Complete selection and mark
            self.mark_selected_packages();
        }
    }

    /// Mark all multi-selected packages for install/upgrade
    fn mark_selected_packages(&mut self) {
        // Collect package names that can be marked
        let packages_to_mark: Vec<String> = self.ui.multi_select.iter()
            .filter_map(|&idx| self.pkg.list.get(idx))
            .filter_map(|p| {
                // Check if package can be marked
                if let Some(pkg) = self.pkg.apt.get(&p.name) {
                    if pkg.is_upgradable() || !pkg.is_installed() {
                        return Some(p.name.clone());
                    }
                }
                None
            })
            .collect();

        // Mark each package using AptManager
        for name in &packages_to_mark {
            self.pkg.apt.mark_install(name);
        }

        // Resolve dependencies
        if let Err(e) = self.pkg.apt.resolve() {
            self.status_message = format!("Dependency error: {}", e);
        }

        // Clear selection and exit visual mode
        self.ui.multi_select.clear();
        self.ui.selection_anchor = None;
        self.ui.visual_mode = false;

        // Update state
        self.calculate_pending_changes();
        self.apply_current_filter();
        self.update_status_message();

        // Show changes preview if there are pending changes
        if self.has_pending_changes() {
            self.show_changes_preview();
        }
    }

    /// Preview what additional changes would occur if we mark this package
    /// Does NOT apply the changes - just calculates and shows the preview
    fn preview_mark(&mut self, name: &str) {
        // Get current state of all packages before marking
        let before: std::collections::HashSet<String> = self
            .pkg.apt
            .get_changes()
            .map(|p| p.name().to_string())
            .collect();

        // Temporarily mark the package to see what would happen
        // Note: We use direct cache access here for the temporary mark
        // since we'll undo it immediately after
        if let Some(pkg) = self.pkg.apt.get(name) {
            pkg.mark_install(true, true);
            pkg.protect();
        }

        // Resolve dependencies
        if let Err(e) = self.pkg.apt.resolve() {
            self.status_message = format!("Dependency error: {}", e);
            // Undo and return
            self.restore_marks();
            return;
        }

        // Get the diff - what additional changes would occur
        let mut additional_installs = Vec::new();
        let mut additional_upgrades = Vec::new();
        let mut additional_removes = Vec::new();
        let mut download_size: u64 = 0;

        for pkg in self.pkg.apt.get_changes() {
            let pkg_name = pkg.name().to_string();

            // Skip the package we're marking
            if pkg_name == name {
                if let Some(cand) = pkg.candidate() {
                    download_size += cand.size();
                }
                continue;
            }

            // Only show packages that weren't already marked
            if !before.contains(&pkg_name) {
                if pkg.marked_install() || pkg.marked_upgrade() {
                    if let Some(cand) = pkg.candidate() {
                        download_size += cand.size();
                    }
                    // Distinguish between new installs and upgrades
                    if pkg.is_installed() {
                        additional_upgrades.push(pkg_name);
                    } else {
                        additional_installs.push(pkg_name);
                    }
                } else if pkg.marked_delete() {
                    additional_removes.push(pkg_name);
                }
            }
        }

        // UNDO - restore cache to previous state before showing preview
        self.restore_marks();
        self.apply_current_filter();

        // If there are additional changes, show the confirmation popup
        if !additional_installs.is_empty() || !additional_upgrades.is_empty() || !additional_removes.is_empty() {
            self.pkg.mark_preview = MarkPreview {
                package_name: name.to_string(),
                is_marking: true,
                additional_installs,
                additional_upgrades,
                additional_removes,
                download_size,
            };
            self.modals.mark_confirm_scroll = 0;
            self.state = AppState::ShowingMarkConfirm;
        } else {
            // No additional changes, just apply directly
            self.pkg.mark_preview = MarkPreview {
                package_name: name.to_string(),
                is_marking: true,
                additional_installs: Vec::new(),
                additional_upgrades: Vec::new(),
                additional_removes: Vec::new(),
                download_size,
            };
            self.confirm_mark();
        }
    }

    /// Restore cache marks to match user_marked state
    fn restore_marks(&mut self) {
        // Use AptManager's refresh to reload and reapply marks
        if let Err(e) = self.pkg.apt.refresh() {
            self.status_message = format!("Warning: cache refresh issue: {}", e);
        }
    }

    /// User confirmed the mark, now actually apply it
    pub fn confirm_mark(&mut self) {
        let name = self.pkg.mark_preview.package_name.clone();

        // Now actually apply the mark using AptManager
        self.pkg.apt.mark_install(&name);

        if let Err(e) = self.pkg.apt.resolve() {
            self.status_message = format!("Warning: dependency resolution issue: {}", e);
        }

        self.pkg.mark_preview = MarkPreview::default();
        self.calculate_pending_changes();
        self.apply_current_filter();
        self.update_status_message();
        self.state = AppState::Listing;
    }

    /// User cancelled the mark - nothing to undo since we didn't apply yet
    pub fn cancel_mark(&mut self) {
        self.pkg.mark_preview = MarkPreview::default();
        self.calculate_pending_changes();
        self.update_status_message();
        self.state = AppState::Listing;
    }

    pub fn mark_all_upgrades(&mut self) {
        // Collect upgradable package names first
        let upgradable: Vec<String> = self
            .pkg.apt
            .packages(&PackageSort::default())
            .filter(|p| p.is_upgradable())
            .map(|p| p.name().to_string())
            .collect();

        // Mark each package
        for name in upgradable {
            self.pkg.apt.mark_install(&name);
        }

        if let Err(e) = self.pkg.apt.resolve() {
            self.status_message = format!("Dependency error: {}", e);
        }

        self.calculate_pending_changes();
        self.apply_current_filter();
        self.update_status_message();

        // Show changes preview so user can see all dependencies
        self.show_changes_preview();
    }

    pub fn unmark_all(&mut self) {
        // Collect all package names first
        let all_names: Vec<String> = self
            .pkg.apt
            .packages(&PackageSort::default())
            .map(|p| p.name().to_string())
            .collect();

        // Mark each to keep
        for name in all_names {
            self.pkg.apt.mark_keep(&name);
        }
        self.pkg.apt.clear_user_marks();

        self.calculate_pending_changes();
        self.apply_current_filter();
        self.update_status_message();
    }

    pub fn calculate_pending_changes(&mut self) {
        // Delegate to AptManager
        self.pkg.pending = self.pkg.apt.calculate_pending();
    }

    pub fn show_changes_preview(&mut self) {
        self.calculate_pending_changes();
        if self.has_pending_changes() {
            self.state = AppState::ShowingChanges;
            self.modals.changes_scroll = 0;
        } else {
            self.status_message = "No changes to apply".to_string();
        }
    }

    #[must_use]
    pub fn has_pending_changes(&self) -> bool {
        !self.pkg.pending.to_install.is_empty()
            || !self.pkg.pending.to_upgrade.is_empty()
            || !self.pkg.pending.to_remove.is_empty()
            || !self.pkg.pending.auto_install.is_empty()
            || !self.pkg.pending.auto_remove.is_empty()
    }

    #[must_use]
    pub fn total_changes_count(&self) -> usize {
        self.pkg.pending.to_install.len()
            + self.pkg.pending.to_upgrade.len()
            + self.pkg.pending.to_remove.len()
            + self.pkg.pending.auto_install.len()
            + self.pkg.pending.auto_remove.len()
    }

    pub fn update_upgradable_count(&mut self) {
        self.pkg.upgradable_count = self.pkg.apt.count_upgradable();
    }

    pub fn update_status_message(&mut self) {
        let changes = self.total_changes_count();

        if changes > 0 {
            self.status_message = format!(
                "{} changes pending ({} download) | {} upgradable | Press 'u' to review",
                changes,
                PackageInfo::size_str(self.pkg.pending.download_size),
                self.pkg.upgradable_count
            );
        } else {
            self.status_message = format!("{} packages upgradable", self.pkg.upgradable_count);
        }
    }

    pub fn move_package_selection(&mut self, delta: i32) {
        if self.pkg.list.is_empty() {
            return;
        }
        let current = self.ui.table_state.selected().unwrap_or(0) as i32;
        let new_idx = (current + delta).clamp(0, self.pkg.list.len() as i32 - 1) as usize;
        self.ui.table_state.select(Some(new_idx));
        self.details.scroll = 0;
        self.update_cached_deps();
    }

    pub fn move_filter_selection(&mut self, delta: i32) {
        let filters = FilterCategory::all();
        let current = self.ui.filter_state.selected().unwrap_or(0) as i32;
        let new_idx = (current + delta).clamp(0, filters.len() as i32 - 1) as usize;
        self.ui.filter_state.select(Some(new_idx));
        self.ui.selected_filter = filters[new_idx];
        self.apply_current_filter();
        self.ui.table_state.select(if self.pkg.list.is_empty() {
            None
        } else {
            Some(0)
        });
    }

    /// Scroll changelog with bounds checking
    pub fn scroll_changelog(&mut self, delta: i32) {
        let max_scroll = self.modals.changelog_content.len().saturating_sub(1) as u16;
        let current = self.modals.changelog_scroll as i32;
        self.modals.changelog_scroll = (current + delta).clamp(0, max_scroll as i32) as u16;
    }

    /// Get the number of lines in the pending changes display
    pub fn changes_line_count(&self) -> usize {
        let mut count = 2; // Header lines
        if !self.pkg.pending.to_upgrade.is_empty() {
            count += 2 + self.pkg.pending.to_upgrade.len();
        }
        if !self.pkg.pending.to_install.is_empty() {
            count += 2 + self.pkg.pending.to_install.len();
        }
        if !self.pkg.pending.auto_upgrade.is_empty() {
            count += 2 + self.pkg.pending.auto_upgrade.len();
        }
        if !self.pkg.pending.auto_install.is_empty() {
            count += 2 + self.pkg.pending.auto_install.len();
        }
        if !self.pkg.pending.to_remove.is_empty() {
            count += 2 + self.pkg.pending.to_remove.len();
        }
        if !self.pkg.pending.auto_remove.is_empty() {
            count += 2 + self.pkg.pending.auto_remove.len();
        }
        count + 4 // Footer lines (size info)
    }

    /// Scroll changes view with bounds checking
    pub fn scroll_changes(&mut self, delta: i32) {
        let max_scroll = self.changes_line_count().saturating_sub(5) as u16; // Assume ~5 visible lines minimum
        let current = self.modals.changes_scroll as i32;
        self.modals.changes_scroll = (current + delta).clamp(0, max_scroll as i32) as u16;
    }

    /// Get the number of lines in the mark confirmation display
    pub fn mark_confirm_line_count(&self) -> usize {
        let mut count = 4; // Header lines
        count += self.pkg.mark_preview.additional_upgrades.len();
        count += self.pkg.mark_preview.additional_installs.len();
        count += self.pkg.mark_preview.additional_removes.len();
        count + 4 // Footer lines
    }

    /// Scroll mark confirmation view with bounds checking
    pub fn scroll_mark_confirm(&mut self, delta: i32) {
        let max_scroll = self.mark_confirm_line_count().saturating_sub(5) as u16;
        let current = self.modals.mark_confirm_scroll as i32;
        self.modals.mark_confirm_scroll = (current + delta).clamp(0, max_scroll as i32) as u16;
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

    #[must_use]
    pub fn is_root() -> bool {
        unsafe { libc::geteuid() == 0 }
    }

    /// Check if APT lock files are held by another process
    pub fn check_apt_lock() -> Option<String> {
        use std::fs::File;
        use std::os::unix::io::AsRawFd;

        let lock_paths = [
            "/var/lib/dpkg/lock-frontend",
            "/var/lib/dpkg/lock",
            "/var/lib/apt/lists/lock",
        ];

        for path in &lock_paths {
            if let Ok(file) = File::open(path) {
                // Try to get an exclusive lock (non-blocking)
                let fd = file.as_raw_fd();
                let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
                if ret != 0 {
                    return Some(format!(
                        "Another package manager is running ({}). Close it and try again.",
                        path
                    ));
                }
                // Release the lock immediately
                unsafe { libc::flock(fd, libc::LOCK_UN) };
            }
        }
        None
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
        // This runs outside the TUI context
        println!("\n=== Applying changes ===\n");

        match self.pkg.apt.commit() {
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

        // Clear state after commit (user_marks cleared by apt.commit())
        self.pkg.pending = PendingChanges::default();
        self.search.index = None;

        Ok(())
    }

    /// Commit changes using sudo -S (password piped to stdin)
    pub fn commit_with_sudo(&mut self) -> Result<()> {
        use std::io::{Read, Write};
        use std::process::{Command, Stdio};

        println!("\n=== Applying changes with sudo ===\n");

        // Build the apt-get command
        let mut args = vec!["apt-get".to_string(), "-y".to_string()];

        // Collect packages to install/upgrade
        let to_install: Vec<&str> = self.pkg.pending.to_install.iter()
            .chain(self.pkg.pending.to_upgrade.iter())
            .chain(self.pkg.pending.auto_install.iter())
            .chain(self.pkg.pending.auto_upgrade.iter())
            .map(|s| s.as_str())
            .collect();

        // Collect packages to remove
        let to_remove: Vec<String> = self.pkg.pending.to_remove.iter()
            .chain(self.pkg.pending.auto_remove.iter())
            .map(|s| format!("{}-", s))  // apt-get syntax for remove
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

        // Run sudo -S apt-get ... (capture stderr for error reporting)
        // Use absolute path to prevent PATH hijacking attacks
        let mut child = Command::new("/usr/bin/sudo")
            .arg("-S")
            .args(&args)
            .stdin(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        // Write password to stdin
        if let Some(mut stdin) = child.stdin.take() {
            writeln!(stdin, "{}", self.sudo_password)?;
        }

        // Clear password from memory
        self.sudo_password.zeroize();

        // Capture stderr (ignore read errors - we'll just have empty output)
        let mut stderr_output = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            if let Err(e) = stderr.read_to_string(&mut stderr_output) {
                stderr_output = format!("(failed to read stderr: {})", e);
            }
        }

        // Wait for completion
        let status = child.wait()?;

        if status.success() {
            println!("\n=== Changes applied successfully ===");
            self.output_lines.push("Changes applied successfully.".to_string());
            self.state = AppState::Done;
            self.status_message = "Done. Press 'q' to quit or 'r' to refresh.".to_string();
        } else {
            println!("\n=== Error applying changes ===");
            // Show captured stderr for debugging
            if !stderr_output.is_empty() {
                // Filter out sudo password prompt from stderr
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

        // Clear state after commit
        self.pkg.apt.clear_user_marks();
        self.pkg.pending = PendingChanges::default();
        self.search.index = None;

        Ok(())
    }

    pub fn refresh_cache(&mut self) -> Result<()> {
        // Check for APT locks first
        if let Some(msg) = Self::check_apt_lock() {
            self.status_message = msg;
            return Ok(()); // Not a fatal error, just can't refresh
        }

        self.pkg.apt.full_refresh()?;
        self.pkg.pending = PendingChanges::default();
        self.search.index = None; // Force rebuild on next search
        self.search.query.clear();
        self.search.results = None;
        self.update_upgradable_count();
        self.reload_packages()?;
        Ok(())
    }
}
