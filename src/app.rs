//! Application state and logic

use std::collections::{HashMap, HashSet};

use color_eyre::Result;
use ratatui::widgets::{ListState, TableState};
use rust_apt::cache::{Cache, PackageSort};
use rust_apt::progress::{AcquireProgress, InstallProgress};
use rust_apt::{Package, Version};

use crate::search::SearchIndex;
use crate::types::*;

pub struct App {
    pub cache: Cache,
    pub packages: Vec<PackageInfo>,
    pub user_marked: HashMap<String, bool>,
    pub table_state: TableState,
    pub filter_state: ListState,
    pub state: AppState,
    pub status_message: String,
    pub output_lines: Vec<String>,
    pub focused_pane: FocusedPane,
    pub selected_filter: FilterCategory,
    pub detail_scroll: u16,
    pub details_tab: DetailsTab,
    pub pending_changes: PendingChanges,
    pub changes_scroll: u16,
    pub mark_preview: MarkPreview, // Preview of additional changes when marking
    pub mark_confirm_scroll: u16,
    // Search
    pub search_index: Option<SearchIndex>,
    pub search_query: String,
    pub search_results: Option<HashSet<String>>, // None = no active search filter
    pub search_build_time: Option<std::time::Duration>,
    // Settings
    pub settings: Settings,
    pub settings_selection: usize,
    // Changelog
    pub changelog_content: Vec<String>,
    pub changelog_scroll: u16,
    // Cached dependencies for current selection (avoid recalc every frame)
    pub cached_deps: Vec<(String, String)>,
    pub cached_rdeps: Vec<(String, String)>,
    pub cached_pkg_name: String,
    // Column widths (calculated from content)
    pub col_widths: ColumnWidths,
    // Sudo password input
    pub sudo_password: String,
    // Multi-select (visual mode)
    pub multi_select: HashSet<usize>,    // Indices of selected packages in current list
    pub selection_anchor: Option<usize>, // Anchor point for range selection
    pub visual_mode: bool,               // Whether visual selection mode is active
    // Cached counts (avoid iterating all packages repeatedly)
    pub upgradable_count: usize,
}

impl App {
    pub fn new() -> Result<Self> {
        let cache = Cache::new::<&str>(&[])?;
        let mut filter_state = ListState::default();
        filter_state.select(Some(0));

        let mut app = Self {
            cache,
            packages: Vec::new(),
            user_marked: HashMap::new(),
            table_state: TableState::default(),
            filter_state,
            state: AppState::Listing,
            status_message: String::from("Loading..."),
            output_lines: Vec::new(),
            focused_pane: FocusedPane::Packages,
            selected_filter: FilterCategory::Upgradable,
            detail_scroll: 0,
            details_tab: DetailsTab::Info,
            pending_changes: PendingChanges::default(),
            changes_scroll: 0,
            mark_preview: MarkPreview::default(),
            mark_confirm_scroll: 0,
            search_index: None,
            search_query: String::new(),
            search_results: None,
            search_build_time: None,
            settings: Settings::default(),
            settings_selection: 0,
            changelog_content: Vec::new(),
            changelog_scroll: 0,
            cached_deps: Vec::new(),
            cached_rdeps: Vec::new(),
            cached_pkg_name: String::new(),
            col_widths: ColumnWidths::new(),
            sudo_password: String::new(),
            multi_select: HashSet::new(),
            selection_anchor: None,
            visual_mode: false,
            upgradable_count: 0,
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
        if pkg_name == self.cached_pkg_name {
            return;
        }
        self.cached_pkg_name = pkg_name.clone();

        // Clear and recalculate deps
        self.cached_deps.clear();
        self.cached_rdeps.clear();

        let pkg = match self.cache.get(&pkg_name) {
            Some(p) => p,
            None => return,
        };

        // Get forward dependencies
        if let Some(version) = pkg.candidate() {
            if let Some(dependencies) = version.dependencies() {
                for dep in dependencies {
                    let dep_type = dep.dep_type().to_string();
                    for base_dep in dep.iter() {
                        self.cached_deps.push((dep_type.clone(), base_dep.name().to_string()));
                    }
                }
            }
        }

        // Sort deps by type priority, then by name
        fn dep_type_order(t: &str) -> u8 {
            match t {
                "PreDepends" => 0,
                "Depends" => 1,
                "Recommends" => 2,
                "Suggests" => 3,
                "Enhances" => 4,
                _ => 5,
            }
        }
        self.cached_deps.sort_by(|a, b| {
            dep_type_order(&a.0).cmp(&dep_type_order(&b.0))
                .then_with(|| a.1.cmp(&b.1))
        });

        // Get reverse dependencies
        let rdep_map = pkg.rdepends();
        for (dep_type, deps) in rdep_map.iter() {
            let type_str = format!("{:?}", dep_type);
            for dep in deps {
                for base_dep in dep.iter() {
                    self.cached_rdeps.push((type_str.clone(), base_dep.name().to_string()));
                }
            }
        }
        // Sort rdeps by type priority, then by name
        self.cached_rdeps.sort_by(|a, b| {
            dep_type_order(&a.0).cmp(&dep_type_order(&b.0))
                .then_with(|| a.1.cmp(&b.1))
        });
    }

    /// Get status for a package by name (for dep views)
    pub fn get_package_status(&self, name: &str) -> PackageStatus {
        match self.cache.get(name) {
            Some(pkg) => {
                if pkg.marked_upgrade() {
                    PackageStatus::Upgrade
                } else if pkg.marked_install() {
                    if pkg.is_installed() {
                        PackageStatus::Upgrade
                    } else if self.user_marked.get(name).copied().unwrap_or(false) {
                        PackageStatus::Install
                    } else {
                        PackageStatus::AutoInstall
                    }
                } else if pkg.marked_delete() {
                    if self.user_marked.get(name).copied().unwrap_or(false) {
                        PackageStatus::Remove
                    } else {
                        PackageStatus::AutoRemove
                    }
                } else if pkg.is_installed() {
                    if pkg.is_upgradable() {
                        PackageStatus::Upgradable
                    } else {
                        PackageStatus::Installed
                    }
                } else {
                    PackageStatus::NotInstalled
                }
            }
            None => PackageStatus::NotInstalled,
        }
    }

    pub fn ensure_search_index(&mut self) -> Result<()> {
        if self.search_index.is_none() {
            let mut index = SearchIndex::new()?;
            let (count, duration) = index.build(&self.cache)?;
            self.search_build_time = Some(duration);
            self.status_message = format!(
                "Search index built: {} packages in {:.0}ms",
                count,
                duration.as_secs_f64() * 1000.0
            );
            self.search_index = Some(index);
        }
        Ok(())
    }

    pub fn start_search(&mut self) {
        if let Err(e) = self.ensure_search_index() {
            self.status_message = format!("Failed to build search index: {}", e);
            return;
        }
        self.search_query.clear();
        self.state = AppState::Searching;
    }

    pub fn execute_search(&mut self) {
        if self.search_query.is_empty() {
            self.search_results = None;
        } else if let Some(ref index) = self.search_index {
            match index.search(&self.search_query) {
                Ok(results) => {
                    self.search_results = Some(results);
                }
                Err(e) => {
                    self.status_message = format!("Search error: {}", e);
                    self.search_results = None;
                }
            }
        }
        self.apply_current_filter();
        self.table_state.select(if self.packages.is_empty() {
            None
        } else {
            Some(0)
        });
    }

    pub fn cancel_search(&mut self) {
        self.search_query.clear();
        self.search_results = None;
        self.state = AppState::Listing;
        self.apply_current_filter();
        self.update_status_message();
    }

    pub fn confirm_search(&mut self) {
        self.execute_search();
        self.state = AppState::Listing;
        if let Some(ref results) = self.search_results {
            self.status_message = format!(
                "Found {} packages matching '{}'",
                results.len(),
                self.search_query
            );
        }
    }

    pub fn reload_packages(&mut self) -> Result<()> {
        self.packages.clear();
        self.apply_current_filter();

        if !self.packages.is_empty() {
            self.table_state.select(Some(0));
        }

        self.update_cached_deps();
        self.update_status_message();
        Ok(())
    }

    pub fn apply_current_filter(&mut self) {
        self.packages.clear();
        self.multi_select.clear();
        self.selection_anchor = None;
        self.visual_mode = false;

        // Reset column widths to header minimums
        self.col_widths.reset();

        let sort = if self.selected_filter == FilterCategory::Upgradable {
            PackageSort::default().upgradable()
        } else {
            PackageSort::default()
        };

        for pkg in self.cache.packages(&sort) {
            // Apply category filter
            let matches_category = match self.selected_filter {
                FilterCategory::Upgradable => pkg.is_upgradable(),
                FilterCategory::MarkedChanges => {
                    pkg.marked_install() || pkg.marked_delete() || pkg.marked_upgrade()
                }
                FilterCategory::Installed => pkg.is_installed(),
                FilterCategory::NotInstalled => !pkg.is_installed(),
                FilterCategory::All => true,
            };

            // Apply search filter if active
            let matches_search = match &self.search_results {
                Some(results) => results.contains(pkg.name()),
                None => true,
            };

            if matches_category && matches_search {
                if let Some(info) = self.extract_package_info(&pkg) {
                    // Track max column widths
                    self.col_widths.name = self.col_widths.name.max(info.name.len() as u16);
                    self.col_widths.section = self.col_widths.section.max(info.section.len() as u16);
                    self.col_widths.installed = self.col_widths.installed.max(info.installed_version.len() as u16);
                    self.col_widths.candidate = self.col_widths.candidate.max(info.candidate_version.len() as u16);
                    self.packages.push(info);
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
        self.packages.sort_by(cmp);
    }

    fn extract_package_info(&self, pkg: &Package) -> Option<PackageInfo> {
        let candidate = pkg.candidate()?;

        let status = if pkg.marked_upgrade() {
            // Upgrade is always for installed packages
            PackageStatus::Upgrade
        } else if pkg.marked_install() {
            if pkg.is_installed() {
                // Installed package marked for install = upgrade
                PackageStatus::Upgrade
            } else if self.user_marked.get(pkg.name()).copied().unwrap_or(false) {
                PackageStatus::Install
            } else {
                PackageStatus::AutoInstall
            }
        } else if pkg.marked_delete() {
            if self.user_marked.get(pkg.name()).copied().unwrap_or(false) {
                PackageStatus::Remove
            } else {
                PackageStatus::AutoRemove
            }
        } else if pkg.is_installed() {
            if pkg.is_upgradable() {
                PackageStatus::Upgradable
            } else {
                PackageStatus::Installed
            }
        } else {
            PackageStatus::NotInstalled
        };

        let installed_version = pkg
            .installed()
            .map(|v: Version| v.version().to_string())
            .unwrap_or_default();

        let installed_size = candidate.installed_size();
        let download_size = candidate.size();

        let description = candidate.summary().unwrap_or_default().to_string();
        let section = candidate.section().unwrap_or("unknown").to_string();
        let architecture = candidate.arch().to_string();

        Some(PackageInfo {
            name: pkg.name().to_string(),
            status,
            section,
            installed_version,
            candidate_version: candidate.version().to_string(),
            installed_size,
            download_size,
            description,
            architecture,
            is_user_marked: self.user_marked.get(pkg.name()).copied().unwrap_or(false),
        })
    }

    pub fn selected_package(&self) -> Option<&PackageInfo> {
        self.table_state
            .selected()
            .and_then(|i| self.packages.get(i))
    }

    pub fn next_details_tab(&mut self) {
        self.details_tab = match self.details_tab {
            DetailsTab::Info => DetailsTab::Dependencies,
            DetailsTab::Dependencies => DetailsTab::ReverseDeps,
            DetailsTab::ReverseDeps => DetailsTab::Info,
        };
        self.detail_scroll = 0;
    }

    pub fn prev_details_tab(&mut self) {
        self.details_tab = match self.details_tab {
            DetailsTab::Info => DetailsTab::ReverseDeps,
            DetailsTab::Dependencies => DetailsTab::Info,
            DetailsTab::ReverseDeps => DetailsTab::Dependencies,
        };
        self.detail_scroll = 0;
    }

    pub fn show_changelog(&mut self) {
        let pkg_name = match self.selected_package() {
            Some(p) => p.name.clone(),
            None => {
                self.status_message = "No package selected".to_string();
                return;
            }
        };

        self.changelog_content.clear();
        self.changelog_content.push(format!("Loading changelog for {}...", pkg_name));
        self.changelog_scroll = 0;

        // Run apt changelog command
        match std::process::Command::new("apt")
            .args(["changelog", &pkg_name])
            .output()
        {
            Ok(output) => {
                self.changelog_content.clear();
                if output.status.success() {
                    let content = String::from_utf8_lossy(&output.stdout);
                    for line in content.lines() {
                        self.changelog_content.push(line.to_string());
                    }
                    if self.changelog_content.is_empty() {
                        self.changelog_content.push("No changelog available.".to_string());
                    }
                } else {
                    let err = String::from_utf8_lossy(&output.stderr);
                    self.changelog_content.push(format!("Error: {}", err));
                }
            }
            Err(e) => {
                self.changelog_content.clear();
                self.changelog_content.push(format!("Failed to run apt changelog: {}", e));
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
        if let Some(i) = self.table_state.selected() {
            if let Some(pkg_info) = self.packages.get(i) {
                let pkg_name = pkg_info.name.clone();
                self.toggle_package(&pkg_name);
            }
        }
    }

    fn toggle_package(&mut self, name: &str) {
        if let Some(pkg) = self.cache.get(name) {
            let currently_marked = pkg.marked_install() || pkg.marked_upgrade();

            if currently_marked {
                // Unmarking - just do it directly (no confirmation needed)
                pkg.mark_keep();
                self.user_marked.remove(name);

                if let Err(e) = self.cache.resolve(true) {
                    self.status_message = format!("Dependency error: {}", e);
                }

                self.calculate_pending_changes();
                self.apply_current_filter();
                self.update_status_message();
            } else if pkg.is_upgradable() || !pkg.is_installed() {
                // Marking - preview additional changes first
                self.preview_mark(name);
            }
        }
    }

    /// Start or extend visual selection mode
    pub fn start_visual_mode(&mut self) {
        let current_idx = self.table_state.selected().unwrap_or(0);

        if !self.visual_mode {
            // Start visual mode
            self.visual_mode = true;
            self.selection_anchor = Some(current_idx);
            self.multi_select.clear();
            self.multi_select.insert(current_idx);
            self.status_message = "-- VISUAL -- (move to select, v/Space to mark, Esc to cancel)".to_string();
        } else {
            // Already in visual mode - mark selected and exit
            self.mark_selected_packages();
        }
    }

    /// Update visual selection when cursor moves
    pub fn update_visual_selection(&mut self) {
        if !self.visual_mode {
            return;
        }

        let current_idx = self.table_state.selected().unwrap_or(0);
        if let Some(anchor) = self.selection_anchor {
            let start = anchor.min(current_idx);
            let end = anchor.max(current_idx);

            self.multi_select.clear();
            for idx in start..=end {
                self.multi_select.insert(idx);
            }
        }
    }

    /// Cancel visual mode without marking
    pub fn cancel_visual_mode(&mut self) {
        self.visual_mode = false;
        self.multi_select.clear();
        self.selection_anchor = None;
        self.update_status_message();
    }

    /// Handle Shift+Space for multi-select (alternative to 'v')
    pub fn toggle_multi_select(&mut self) {
        if !self.visual_mode {
            // Start visual mode
            self.start_visual_mode();
        } else {
            // Complete selection and mark
            self.mark_selected_packages();
        }
    }

    /// Mark all multi-selected packages for install/upgrade
    fn mark_selected_packages(&mut self) {
        // Collect package names to mark
        let packages_to_mark: Vec<String> = self.multi_select.iter()
            .filter_map(|&idx| self.packages.get(idx))
            .map(|p| p.name.clone())
            .collect();

        // Mark each package
        for name in &packages_to_mark {
            if let Some(pkg) = self.cache.get(name) {
                if pkg.is_upgradable() || !pkg.is_installed() {
                    pkg.mark_install(true, true);
                    pkg.protect();
                    self.user_marked.insert(name.clone(), true);
                }
            }
        }

        // Resolve dependencies
        if let Err(e) = self.cache.resolve(true) {
            self.status_message = format!("Dependency error: {}", e);
        }

        // Clear selection and exit visual mode
        self.multi_select.clear();
        self.selection_anchor = None;
        self.visual_mode = false;

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
            .cache
            .get_changes(false)
            .map(|p| p.name().to_string())
            .collect();

        // Temporarily mark the package to see what would happen
        if let Some(pkg) = self.cache.get(name) {
            pkg.mark_install(true, true);
            pkg.protect();
        }

        // Resolve dependencies
        if let Err(e) = self.cache.resolve(true) {
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

        for pkg in self.cache.get_changes(false) {
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
            self.mark_preview = MarkPreview {
                package_name: name.to_string(),
                is_marking: true,
                additional_installs,
                additional_upgrades,
                additional_removes,
                download_size,
            };
            self.mark_confirm_scroll = 0;
            self.state = AppState::ShowingMarkConfirm;
        } else {
            // No additional changes, just apply directly
            self.mark_preview = MarkPreview {
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
        // Nuclear option: reload cache to guarantee clean state
        if let Ok(new_cache) = Cache::new::<&str>(&[]) {
            self.cache = new_cache;
        }

        // Re-apply only user-marked packages
        for name in self.user_marked.keys() {
            if let Some(pkg) = self.cache.get(name) {
                pkg.mark_install(true, true);
                pkg.protect();
            }
        }

        // Resolve dependencies
        if !self.user_marked.is_empty() {
            if let Err(e) = self.cache.resolve(true) {
                self.status_message = format!("Warning: dependency resolution issue: {}", e);
            }
        }
    }

    /// User confirmed the mark, now actually apply it
    pub fn confirm_mark(&mut self) {
        let name = self.mark_preview.package_name.clone();

        // Now actually apply the mark
        if let Some(pkg) = self.cache.get(&name) {
            pkg.mark_install(true, true);
            pkg.protect();
        }
        if let Err(e) = self.cache.resolve(true) {
            self.status_message = format!("Warning: dependency resolution issue: {}", e);
        }

        // Record in user_marked
        self.user_marked.insert(name, true);

        self.mark_preview = MarkPreview::default();
        self.calculate_pending_changes();
        self.apply_current_filter();
        self.update_status_message();
        self.state = AppState::Listing;
    }

    /// User cancelled the mark - nothing to undo since we didn't apply yet
    pub fn cancel_mark(&mut self) {
        self.mark_preview = MarkPreview::default();
        self.calculate_pending_changes();
        self.update_status_message();
        self.state = AppState::Listing;
    }

    pub fn mark_all_upgrades(&mut self) {
        for pkg in self.cache.packages(&PackageSort::default()) {
            if pkg.is_upgradable() {
                pkg.mark_install(true, true);
                pkg.protect();
                self.user_marked.insert(pkg.name().to_string(), true);
            }
        }

        if let Err(e) = self.cache.resolve(true) {
            self.status_message = format!("Dependency error: {}", e);
        }

        self.calculate_pending_changes();
        self.apply_current_filter();
        self.update_status_message();

        // Show changes preview so user can see all dependencies
        self.show_changes_preview();
    }

    pub fn unmark_all(&mut self) {
        for pkg in self.cache.packages(&PackageSort::default()) {
            pkg.mark_keep();
        }
        self.user_marked.clear();

        self.calculate_pending_changes();
        self.apply_current_filter();
        self.update_status_message();
    }

    pub fn calculate_pending_changes(&mut self) {
        self.pending_changes = PendingChanges::default();

        for pkg in self.cache.get_changes(false) {
            let name = pkg.name().to_string();
            let is_user = self.user_marked.get(&name).copied().unwrap_or(false);

            if pkg.marked_install() {
                if pkg.is_installed() {
                    if is_user {
                        self.pending_changes.to_upgrade.push(name);
                    } else {
                        self.pending_changes.auto_upgrade.push(name);
                    }
                } else {
                    if is_user {
                        self.pending_changes.to_install.push(name);
                    } else {
                        self.pending_changes.auto_install.push(name);
                    }
                }

                if let Some(cand) = pkg.candidate() {
                    self.pending_changes.download_size += cand.size();
                    self.pending_changes.install_size_change += cand.installed_size() as i64;
                }
            } else if pkg.marked_delete() {
                if is_user {
                    self.pending_changes.to_remove.push(name);
                } else {
                    self.pending_changes.auto_remove.push(name);
                }

                if let Some(inst) = pkg.installed() {
                    self.pending_changes.install_size_change -= inst.installed_size() as i64;
                }
            }
        }
    }

    pub fn show_changes_preview(&mut self) {
        self.calculate_pending_changes();
        if self.has_pending_changes() {
            self.state = AppState::ShowingChanges;
            self.changes_scroll = 0;
        } else {
            self.status_message = "No changes to apply".to_string();
        }
    }

    pub fn has_pending_changes(&self) -> bool {
        !self.pending_changes.to_install.is_empty()
            || !self.pending_changes.to_upgrade.is_empty()
            || !self.pending_changes.to_remove.is_empty()
            || !self.pending_changes.auto_install.is_empty()
            || !self.pending_changes.auto_remove.is_empty()
    }

    pub fn total_changes_count(&self) -> usize {
        self.pending_changes.to_install.len()
            + self.pending_changes.to_upgrade.len()
            + self.pending_changes.to_remove.len()
            + self.pending_changes.auto_install.len()
            + self.pending_changes.auto_remove.len()
    }

    pub fn update_upgradable_count(&mut self) {
        self.upgradable_count = self
            .cache
            .packages(&PackageSort::default())
            .filter(|p| p.is_upgradable())
            .count();
    }

    pub fn update_status_message(&mut self) {
        let changes = self.total_changes_count();

        if changes > 0 {
            self.status_message = format!(
                "{} changes pending ({} download) | {} upgradable | Press 'u' to review",
                changes,
                PackageInfo::size_str(self.pending_changes.download_size),
                self.upgradable_count
            );
        } else {
            self.status_message = format!("{} packages upgradable", self.upgradable_count);
        }
    }

    pub fn move_package_selection(&mut self, delta: i32) {
        if self.packages.is_empty() {
            return;
        }
        let current = self.table_state.selected().unwrap_or(0) as i32;
        let new_idx = (current + delta).clamp(0, self.packages.len() as i32 - 1) as usize;
        self.table_state.select(Some(new_idx));
        self.detail_scroll = 0;
        self.update_cached_deps();
    }

    pub fn move_filter_selection(&mut self, delta: i32) {
        let filters = FilterCategory::all();
        let current = self.filter_state.selected().unwrap_or(0) as i32;
        let new_idx = (current + delta).clamp(0, filters.len() as i32 - 1) as usize;
        self.filter_state.select(Some(new_idx));
        self.selected_filter = filters[new_idx];
        self.apply_current_filter();
        self.table_state.select(if self.packages.is_empty() {
            None
        } else {
            Some(0)
        });
    }

    /// Scroll changelog with bounds checking
    pub fn scroll_changelog(&mut self, delta: i32) {
        let max_scroll = self.changelog_content.len().saturating_sub(1) as u16;
        let current = self.changelog_scroll as i32;
        self.changelog_scroll = (current + delta).clamp(0, max_scroll as i32) as u16;
    }

    /// Get the number of lines in the pending changes display
    pub fn changes_line_count(&self) -> usize {
        let mut count = 2; // Header lines
        if !self.pending_changes.to_upgrade.is_empty() {
            count += 2 + self.pending_changes.to_upgrade.len();
        }
        if !self.pending_changes.to_install.is_empty() {
            count += 2 + self.pending_changes.to_install.len();
        }
        if !self.pending_changes.auto_upgrade.is_empty() {
            count += 2 + self.pending_changes.auto_upgrade.len();
        }
        if !self.pending_changes.auto_install.is_empty() {
            count += 2 + self.pending_changes.auto_install.len();
        }
        if !self.pending_changes.to_remove.is_empty() {
            count += 2 + self.pending_changes.to_remove.len();
        }
        if !self.pending_changes.auto_remove.is_empty() {
            count += 2 + self.pending_changes.auto_remove.len();
        }
        count + 4 // Footer lines (size info)
    }

    /// Scroll changes view with bounds checking
    pub fn scroll_changes(&mut self, delta: i32) {
        let max_scroll = self.changes_line_count().saturating_sub(5) as u16; // Assume ~5 visible lines minimum
        let current = self.changes_scroll as i32;
        self.changes_scroll = (current + delta).clamp(0, max_scroll as i32) as u16;
    }

    /// Get the number of lines in the mark confirmation display
    pub fn mark_confirm_line_count(&self) -> usize {
        let mut count = 4; // Header lines
        count += self.mark_preview.additional_upgrades.len();
        count += self.mark_preview.additional_installs.len();
        count += self.mark_preview.additional_removes.len();
        count + 4 // Footer lines
    }

    /// Scroll mark confirmation view with bounds checking
    pub fn scroll_mark_confirm(&mut self, delta: i32) {
        let max_scroll = self.mark_confirm_line_count().saturating_sub(5) as u16;
        let current = self.mark_confirm_scroll as i32;
        self.mark_confirm_scroll = (current + delta).clamp(0, max_scroll as i32) as u16;
    }

    pub fn cycle_focus(&mut self) {
        self.focused_pane = match self.focused_pane {
            FocusedPane::Filters => FocusedPane::Packages,
            FocusedPane::Packages => FocusedPane::Details,
            FocusedPane::Details => FocusedPane::Filters,
        };
    }

    pub fn cycle_focus_back(&mut self) {
        self.focused_pane = match self.focused_pane {
            FocusedPane::Filters => FocusedPane::Details,
            FocusedPane::Packages => FocusedPane::Filters,
            FocusedPane::Details => FocusedPane::Packages,
        };
    }

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
            self.sudo_password.clear();
            self.state = AppState::EnteringPassword;
            return ApplyResult::NeedsPassword;
        }

        self.state = AppState::Upgrading;
        ApplyResult::NeedsCommit
    }

    pub fn commit_changes(&mut self) -> Result<()> {
        // This runs outside the TUI context
        println!("\n=== Applying changes ===\n");

        let mut acquire_progress = AcquireProgress::apt();
        let mut install_progress = InstallProgress::apt();

        // Take ownership of cache for commit (it consumes self)
        let cache = std::mem::replace(&mut self.cache, Cache::new::<&str>(&[])?);

        match cache.commit(&mut acquire_progress, &mut install_progress) {
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

        // Clear state after commit
        self.user_marked.clear();
        self.pending_changes = PendingChanges::default();
        self.search_index = None;

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
        let to_install: Vec<&str> = self.pending_changes.to_install.iter()
            .chain(self.pending_changes.to_upgrade.iter())
            .chain(self.pending_changes.auto_install.iter())
            .chain(self.pending_changes.auto_upgrade.iter())
            .map(|s| s.as_str())
            .collect();

        // Collect packages to remove
        let to_remove: Vec<String> = self.pending_changes.to_remove.iter()
            .chain(self.pending_changes.auto_remove.iter())
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
        let mut child = Command::new("sudo")
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
        self.sudo_password.clear();

        // Capture stderr
        let mut stderr_output = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            let _ = stderr.read_to_string(&mut stderr_output);
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
        self.user_marked.clear();
        self.pending_changes = PendingChanges::default();
        self.search_index = None;

        Ok(())
    }

    pub fn refresh_cache(&mut self) -> Result<()> {
        // Check for APT locks first
        if let Some(msg) = Self::check_apt_lock() {
            self.status_message = msg;
            return Ok(()); // Not a fatal error, just can't refresh
        }

        self.cache = Cache::new::<&str>(&[])?;
        self.user_marked.clear();
        self.pending_changes = PendingChanges::default();
        self.search_index = None; // Force rebuild on next search
        self.search_query.clear();
        self.search_results = None;
        self.update_upgradable_count();
        self.reload_packages()?;
        Ok(())
    }
}
