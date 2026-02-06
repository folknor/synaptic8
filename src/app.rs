//! TUI application state and logic
//!
//! This module contains TUI-specific state and acts as an adapter between
//! the core business logic (ManagerState) and the ratatui UI.

use std::collections::HashSet;

use color_eyre::Result;
use ratatui::widgets::{ListState, TableState};

use synh8::core::{ManagerState, check_apt_lock};
use synh8::progress::{ProgressState, StdioRedirect, TuiAcquireProgress, TuiInstallProgress};
use synh8::types::*;

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

/// TUI Application - wraps ManagerState with UI state
pub struct App {
    /// Core business logic (typestate package manager wrapped in enum)
    pub core: ManagerState,

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

    /// Mark preview state (shown before confirming package mark)
    pub mark_preview: Option<MarkPreview>,
    pub mark_preview_scroll: usize,
}

impl App {
    pub fn new() -> Result<Self> {
        let core = ManagerState::new()?;
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
            mark_preview: None,
            mark_preview_scroll: 0,
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
            .and_then(|name| self.core.list().iter().position(|p| p.name == name))
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

    #[must_use]
    pub fn has_pending_changes(&self) -> bool {
        self.core.has_marks()
    }

    #[must_use]
    pub fn total_changes_count(&self) -> usize {
        match self.core.planned_changes() {
            Some(changes) => changes.len(),
            None => 0,
        }
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
                self.status_message = format!("Failed to build search index: {e}");
                return;
            }
        }
        self.state = AppState::Searching;
    }

    pub fn execute_search(&mut self) {
        let query = self.core.search_query().to_string();
        if let Err(e) = self.core.set_search_query(&query) {
            self.status_message = format!("Search error: {e}");
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
        self.core.apply_filter(self.core.selected_filter());
        self.col_widths = self.core.rebuild_list();
        self.reset_selection();
    }

    pub fn move_filter_selection(&mut self, delta: i32) {
        // Cancel visual mode since the package list is about to change
        if self.ui.visual_mode {
            self.cancel_visual_mode();
        }

        let filters = FilterCategory::all();
        let current = self.ui.filter_state.selected().unwrap_or(0) as i32;
        let new_idx = (current + delta).clamp(0, filters.len() as i32 - 1) as usize;
        self.ui.filter_state.select(Some(new_idx));
        self.core.apply_filter(filters[new_idx]);
        self.refresh_ui_state();
    }

    // === Package marking ===

    pub fn toggle_current(&mut self) {
        let Some(pkg) = self.selected_package() else {
            return;
        };
        let id = pkg.id;
        // Use display name (strips native arch suffix)
        let pkg_name = self.core.cache().display_name(&pkg.name).to_string();
        let was_marked = pkg.status.is_marked();

        // Skip toggle for installed non-upgradable packages that aren't already marked
        if !was_marked && pkg.status == PackageStatus::Installed {
            self.status_message = format!("{pkg_name} is already installed and up to date");
            return;
        }

        // Track if this was a user-marked package (vs dependency) BEFORE toggle
        let was_user_marked = self.core.is_user_marked(id);

        // Snapshot currently planned packages BEFORE the toggle so we can
        // show only the NEW dependencies in the confirmation modal
        let previously_planned: HashSet<PackageId> = self.core.planned_changes()
            .map(|changes| changes.iter().map(|c| c.package).collect())
            .unwrap_or_default();

        // Use the library's toggle() which handles cascade correctly
        let result = self.core.toggle(id);

        match result {
            ToggleResult::Marked { package: _, additional } => {
                // Package was marked - check if there are additional deps
                if additional.is_empty() {
                    // No additional deps, just update UI
                    self.refresh_ui_state();
                    self.update_status_message();
                } else {
                    // Build preview for confirmation modal (only new deps)
                    let preview = self.core.build_mark_preview(id, &previously_planned);
                    self.mark_preview = preview;
                    self.mark_preview_scroll = 0;
                    self.state = AppState::ShowingMarkConfirm;
                }
            }
            ToggleResult::Unmarked { package: _, also_unmarked } => {
                // Package was unmarked - check if cascade happened
                if also_unmarked.is_empty() {
                    // No cascade, just update UI
                    self.refresh_ui_state();
                    self.update_status_message();
                } else {
                    // Build preview showing what was unmarked (using display names)
                    let cache = self.core.cache();
                    let also_names: Vec<String> = also_unmarked.iter()
                        .filter_map(|id| cache.fullname_of(*id).map(|n| cache.display_name(n).to_string()))
                        .collect();

                    let preview = MarkPreview {
                        package_name: pkg_name,
                        is_upgrade: was_marked,
                        is_marking: false, // This is an unmark operation
                        was_user_marked, // Was the original package user-marked (vs dependency)?
                        additional_installs: Vec::new(),
                        additional_upgrades: also_names, // Reuse this field for "also unmarked"
                        additional_removes: Vec::new(),
                        download_size: 0,
                    };
                    self.mark_preview = Some(preview);
                    self.mark_preview_scroll = 0;
                    self.state = AppState::ShowingMarkConfirm;
                }
            }
            ToggleResult::NoChange { package: _ } => {
                // Couldn't unmark - it's a dependency we can't trace
                // Tell user to unmark the original package instead
                self.status_message = format!(
                    "{pkg_name} is a dependency - unmark the package that requires it"
                );
                self.refresh_ui_state();
            }
        }
    }

    pub fn confirm_mark(&mut self) {
        // Package is already marked and planned - just close the modal
        self.mark_preview = None;
        self.refresh_ui_state();
        self.update_status_message();
        self.state = AppState::Listing;
    }

    pub fn cancel_mark(&mut self) {
        if let Some(ref preview) = self.mark_preview {
            if preview.is_marking {
                // Undo a mark operation - unmark the package
                // First, find the ID using display_name comparison (immutable borrow scope)
                let id_to_unmark = {
                    let cache = self.core.cache();
                    self.core.list().iter()
                        .find(|p| cache.display_name(&p.name) == preview.package_name)
                        .map(|p| p.id)
                };
                // Now mutate (borrow dropped)
                if let Some(id) = id_to_unmark {
                    self.core.unmark(id);
                }
            } else {
                // Undo an unmark operation - re-mark the USER-MARKED packages only
                // Dependencies will be restored automatically by compute_plan()
                //
                // If original was user-marked: re-mark it (deps follow)
                // If original was a dependency: re-mark the also_unmarked (they were user-marked)
                let names_to_remark: Vec<String> = if preview.was_user_marked {
                    // Original package was user-marked, re-mark only it
                    vec![preview.package_name.clone()]
                } else {
                    // Original was a dependency, re-mark the user-marked packages (in also_unmarked)
                    preview.additional_upgrades.clone()
                };

                // First, collect all IDs to remark (immutable borrow scope)
                let ids_to_remark: Vec<_> = {
                    let cache = self.core.cache();
                    names_to_remark.iter()
                        .filter_map(|name| {
                            self.core.list().iter()
                                .find(|p| cache.display_name(&p.name) == *name)
                                .map(|p| p.id)
                        })
                        .collect()
                };

                // Now mutate (borrow dropped)
                for id in ids_to_remark {
                    self.core.mark_install(id);
                }
                // Recompute plan after re-marking
                self.core.compute_plan();
            }
        }
        self.mark_preview = None;
        self.refresh_ui_state();
        self.update_status_message();
        self.state = AppState::Listing;
    }

    pub fn mark_all_upgrades(&mut self) {
        // Mark all upgradable packages in the full cache (not just filtered view)
        self.core.mark_all_upgradable();
        // show_changes_preview will compute_plan + rebuild, no need to refresh_ui_state here
        self.update_status_message();
        self.show_changes_preview();
    }

    pub fn unmark_all(&mut self) {
        self.core.reset();
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
        // Get the PackageIds of selected packages
        let ids_to_mark: Vec<PackageId> = self.ui.multi_select.iter()
            .filter_map(|&idx| self.core.get_package(idx))
            .filter(|p| p.status == PackageStatus::Upgradable || p.status == PackageStatus::NotInstalled)
            .map(|p| p.id)
            .collect();

        self.ui.multi_select.clear();
        self.ui.selection_anchor = None;
        self.ui.visual_mode = false;

        // Mark all selected packages
        for id in ids_to_mark {
            self.core.mark_install(id);
        }

        self.refresh_ui_state();
        self.update_status_message();
    }

    // === Navigation ===

    pub fn move_package_selection(&mut self, delta: i32) {
        if self.core.package_count() == 0 {
            return;
        }
        let current = self.ui.table_state.selected().unwrap_or(0) as i64;
        let new_idx = (current + delta as i64).clamp(0, self.core.package_count() as i64 - 1) as usize;
        self.ui.table_state.select(Some(new_idx));
        self.details.scroll = 0;
        self.update_cached_deps();
    }

    pub fn select_first_package(&mut self) {
        if self.core.package_count() == 0 {
            return;
        }
        self.ui.table_state.select(Some(0));
        self.details.scroll = 0;
        self.update_cached_deps();
    }

    pub fn select_last_package(&mut self) {
        if self.core.package_count() == 0 {
            return;
        }
        self.ui.table_state.select(Some(self.core.package_count() - 1));
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
        self.modals.changelog_content.push(format!("Loading changelog for {pkg_name}..."));
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
            // Compute plan to get full changeset
            self.core.compute_plan();
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
        let max_scroll = self.mark_confirm_line_count().saturating_sub(10);
        let current = self.mark_preview_scroll as i32;
        self.mark_preview_scroll = (current + delta).clamp(0, max_scroll as i32) as usize;
    }

    pub fn changes_line_count(&self) -> usize {
        match self.core.planned_changes() {
            Some(changes) => {
                let mut lines = 2; // header + blank line

                // Count changes grouped by action/reason category
                let categories = [
                    changes.iter().filter(|c| c.action == ChangeAction::Upgrade && c.reason == ChangeReason::UserRequested).count(),
                    changes.iter().filter(|c| c.action == ChangeAction::Install && c.reason == ChangeReason::UserRequested).count(),
                    changes.iter().filter(|c| c.action == ChangeAction::Upgrade && c.reason == ChangeReason::Dependency).count(),
                    changes.iter().filter(|c| c.action == ChangeAction::Install && c.reason == ChangeReason::Dependency).count(),
                    changes.iter().filter(|c| c.action == ChangeAction::Remove && c.reason == ChangeReason::UserRequested).count(),
                    changes.iter().filter(|c| c.action == ChangeAction::Remove && c.reason == ChangeReason::AutoRemove).count(),
                ];

                for count in categories {
                    if count > 0 {
                        lines += 1 + count + 1; // header + items + blank
                    }
                }

                lines += 3; // blank + download size + disk change
                lines
            }
            None => 5,
        }
    }

    pub fn mark_confirm_line_count(&self) -> usize {
        if let Some(ref preview) = self.mark_preview {
            let mut count = 2; // Header lines
            if !preview.additional_installs.is_empty() {
                count += 1 + preview.additional_installs.len();
            }
            if !preview.additional_upgrades.is_empty() {
                count += 1 + preview.additional_upgrades.len();
            }
            if !preview.additional_removes.is_empty() {
                count += 1 + preview.additional_removes.len();
            }
            count + 2 // Footer lines
        } else {
            0
        }
    }

    // === Status message ===

    pub fn update_status_message(&mut self) {
        let has_marks = self.core.has_marks();

        if has_marks {
            // Count user marks
            let mark_count = self.core.list().iter()
                .filter(|p| self.core.is_user_marked(p.id))
                .count();

            self.status_message = format!(
                "{} packages marked | {} upgradable | Press 'u' to review",
                mark_count,
                self.core.upgradable_count()
            );
        } else {
            self.status_message = format!("{} packages upgradable", self.core.upgradable_count());
        }
    }

    // === System operations ===

    /// Commit planned changes with live TUI progress display.
    ///
    /// Creates its own `/dev/tty`-backed terminal for the progress modal and
    /// redirects stdout/stderr to `/dev/null` so dpkg output is suppressed.
    pub fn commit_changes_live(&mut self) -> Result<()> {
        use std::cell::RefCell;
        use std::rc::Rc;

        self.state = AppState::Upgrading;

        let progress_state = Rc::new(RefCell::new(
            ProgressState::new("Applying Changes")?,
        ));

        let acq = TuiAcquireProgress::new(Rc::clone(&progress_state));
        let inst = TuiInstallProgress::new(Rc::clone(&progress_state));

        let mut acquire_progress = rust_apt::progress::AcquireProgress::new(acq);
        let mut install_progress = rust_apt::progress::InstallProgress::new(inst);

        // Redirect stdout/stderr to /dev/null so dpkg output is suppressed.
        // The progress terminal writes to /dev/tty directly, bypassing fd 1.
        let _redirect = StdioRedirect::to_devnull();

        match self.core.commit_with_progress(&mut acquire_progress, &mut install_progress) {
            Ok(()) => {
                self.output_lines.push("Changes applied successfully.".to_string());
                self.state = AppState::Done;
                self.status_message = "Done. Press 'q' to quit or 'r' to refresh.".to_string();
            }
            Err(e) => {
                self.output_lines.push(format!("Error: {e}"));
                self.state = AppState::Done;
                self.status_message = format!("Error: {e}. Press 'q' to quit or 'r' to refresh.");
            }
        }

        // _redirect drops here, restoring stdout/stderr
        Ok(())
    }

    /// Run `apt update` with live TUI progress display.
    ///
    /// Creates its own `/dev/tty`-backed terminal for the progress modal.
    pub fn update_packages_live(&mut self) -> Result<()> {
        use std::cell::RefCell;
        use std::rc::Rc;

        if let Some(msg) = check_apt_lock() {
            self.status_message = msg;
            return Ok(());
        }

        let progress_state = Rc::new(RefCell::new(
            ProgressState::new("Updating Package Lists")?,
        ));

        let acq = TuiAcquireProgress::new(Rc::clone(&progress_state));
        let mut acquire_progress = rust_apt::progress::AcquireProgress::new(acq);

        match self.core.update_with_progress(&mut acquire_progress) {
            Ok(()) => {
                // Rebuild package list and counts after update
                self.core.rebuild_list();
                self.core.update_cache_counts();
                self.apply_current_filter();
                self.update_status_message();
            }
            Err(e) => {
                self.status_message = format!("Update failed: {e}");
            }
        }

        Ok(())
    }

    pub fn refresh_cache(&mut self) -> Result<()> {
        if let Some(msg) = check_apt_lock() {
            self.status_message = msg;
            return Ok(());
        }

        if let Err(e) = self.core.refresh() {
            self.status_message = format!("Refresh failed: {e}");
            return Ok(());
        }

        self.refresh_ui_state();
        self.update_status_message();
        Ok(())
    }
}
