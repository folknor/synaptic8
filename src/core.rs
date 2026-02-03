//! Core business logic - UI agnostic
//!
//! This module contains all package management logic without any TUI dependencies.
//! It can be used by different UI implementations (TUI, GTK, web, etc.)

use std::collections::HashSet;
use std::fs::File;
use std::os::unix::io::AsRawFd;

use color_eyre::Result;
use rust_apt::cache::PackageSort;

use crate::apt::{AptManager, format_apt_errors};
use crate::search::SearchIndex;
use crate::types::*;

/// Search state management
#[derive(Default)]
pub struct SearchState {
    pub index: Option<SearchIndex>,
    pub query: String,
    pub results: Option<HashSet<String>>,
}

/// Sort configuration
#[derive(Clone)]
pub struct SortSettings {
    pub sort_by: SortBy,
    pub ascending: bool,
}

impl Default for SortSettings {
    fn default() -> Self {
        Self {
            sort_by: SortBy::Name,
            ascending: true,
        }
    }
}

/// Result of toggling a package's mark state
pub enum ToggleResult {
    /// Package was unmarked (was marked, now kept)
    Unmarked,
    /// Package needs confirmation due to additional dependencies
    NeedsPreview,
    /// Package was marked directly (no additional deps)
    MarkedDirectly,
    /// Cannot mark this package
    NotMarkable,
    /// Error occurred
    Error(String),
}

/// Result of previewing a mark operation
pub enum PreviewResult {
    /// Additional changes require confirmation
    NeedsConfirmation,
    /// No additional changes, can mark directly
    NoAdditionalChanges,
    /// Error occurred
    Error(String),
}

/// Core package management - UI agnostic
pub struct PackageManager {
    pub apt: AptManager,
    pub search: SearchState,
    pub list: Vec<PackageInfo>,
    pub pending: PendingChanges,
    pub mark_preview: MarkPreview,
    pub upgradable_count: usize,
    pub selected_filter: FilterCategory,
    pub sort_settings: SortSettings,
}

impl PackageManager {
    /// Create a new PackageManager with fresh APT cache
    pub fn new() -> Result<Self> {
        let apt = AptManager::new()?;
        let mut mgr = Self {
            apt,
            search: SearchState::default(),
            list: Vec::new(),
            pending: PendingChanges::default(),
            mark_preview: MarkPreview::default(),
            upgradable_count: 0,
            selected_filter: FilterCategory::Upgradable,
            sort_settings: SortSettings::default(),
        };
        mgr.upgradable_count = mgr.apt.count_upgradable();
        mgr.apply_filter(FilterCategory::Upgradable);
        Ok(mgr)
    }

    // === Filtering & Listing ===

    /// Apply a filter category and rebuild the package list
    pub fn apply_filter(&mut self, filter: FilterCategory) {
        self.selected_filter = filter;
        self.rebuild_list();
    }

    /// Rebuild the package list based on current filter and search
    pub fn rebuild_list(&mut self) -> ColumnWidths {
        self.list.clear();

        let sort = if self.selected_filter == FilterCategory::Upgradable {
            PackageSort::default().upgradable()
        } else {
            PackageSort::default()
        };

        // First pass: collect all matching packages
        for pkg in self.apt.packages(&sort) {
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
            let matches_search = match &self.search.results {
                Some(results) => results.contains(pkg.name()),
                None => true,
            };

            if matches_category && matches_search {
                if let Some(info) = self.apt.extract_package_info(&pkg) {
                    self.list.push(info);
                }
            }
        }

        // Second pass: find duplicate names and append :arch
        let mut name_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for pkg in &self.list {
            *name_counts.entry(pkg.name.clone()).or_insert(0) += 1;
        }
        for pkg in &mut self.list {
            if name_counts.get(&pkg.name).copied().unwrap_or(0) > 1 {
                pkg.name = format!("{}:{}", pkg.name, pkg.architecture);
            }
        }

        // Calculate column widths
        let mut col_widths = ColumnWidths::new();
        for pkg in &self.list {
            col_widths.name = col_widths.name.max(pkg.name.len() as u16);
            col_widths.section = col_widths.section.max(pkg.section.len() as u16);
            col_widths.installed = col_widths.installed.max(pkg.installed_version.len() as u16);
            col_widths.candidate = col_widths.candidate.max(pkg.candidate_version.len() as u16);
        }

        // Sort packages
        self.sort_list();

        col_widths
    }

    /// Sort the package list based on current sort settings
    fn sort_list(&mut self) {
        let sort_by = self.sort_settings.sort_by;
        let ascending = self.sort_settings.ascending;

        self.list.sort_by(|a, b| {
            let ord = match sort_by {
                SortBy::Name => a.name.cmp(&b.name),
                SortBy::Section => a.section.cmp(&b.section),
                SortBy::InstalledVersion => a.installed_version.cmp(&b.installed_version),
                SortBy::CandidateVersion => a.candidate_version.cmp(&b.candidate_version),
            };
            if ascending { ord } else { ord.reverse() }
        });
    }

    /// Update sort settings and re-sort
    pub fn set_sort(&mut self, sort_by: SortBy, ascending: bool) {
        self.sort_settings.sort_by = sort_by;
        self.sort_settings.ascending = ascending;
        self.sort_list();
    }

    /// Get a package by index
    pub fn get_package(&self, index: usize) -> Option<&PackageInfo> {
        self.list.get(index)
    }

    /// Get number of packages in current list
    pub fn package_count(&self) -> usize {
        self.list.len()
    }

    // === Search ===

    /// Ensure search index is built
    pub fn ensure_search_index(&mut self) -> Result<std::time::Duration> {
        if self.search.index.is_none() {
            let mut index = SearchIndex::new()?;
            let (_count, duration) = index.build(&self.apt)?;
            self.search.index = Some(index);
            return Ok(duration);
        }
        Ok(std::time::Duration::ZERO)
    }

    /// Set search query and execute search
    pub fn set_search_query(&mut self, query: &str) -> Result<()> {
        self.search.query = query.to_string();

        if query.is_empty() {
            self.search.results = None;
        } else if let Some(ref index) = self.search.index {
            self.search.results = Some(index.search(query)?);
        }
        Ok(())
    }

    /// Clear search query and results
    pub fn clear_search(&mut self) {
        self.search.query.clear();
        self.search.results = None;
    }

    /// Get current search query
    pub fn search_query(&self) -> &str {
        &self.search.query
    }

    /// Get search result count if search is active
    pub fn search_result_count(&self) -> Option<usize> {
        self.search.results.as_ref().map(|r| r.len())
    }

    // === Marking Operations ===

    /// Toggle a package's mark state
    pub fn toggle_package(&mut self, name: &str) -> ToggleResult {
        let pkg = match self.apt.get(name) {
            Some(p) => p,
            None => return ToggleResult::NotMarkable,
        };

        let currently_marked = pkg.marked_install() || pkg.marked_upgrade();
        let is_upgradable = pkg.is_upgradable();
        let is_installed = pkg.is_installed();
        drop(pkg);

        if currently_marked {
            // Unmarking
            self.apt.mark_keep(name);
            if let Err(e) = self.apt.resolve() {
                return ToggleResult::Error(format!("Dependency conflict: {}", format_apt_errors(&e)));
            }
            self.pending = self.apt.calculate_pending();
            ToggleResult::Unmarked
        } else if is_upgradable || !is_installed {
            // Need to preview
            match self.preview_mark(name) {
                PreviewResult::NeedsConfirmation => ToggleResult::NeedsPreview,
                PreviewResult::NoAdditionalChanges => {
                    self.confirm_mark();
                    ToggleResult::MarkedDirectly
                }
                PreviewResult::Error(e) => ToggleResult::Error(e),
            }
        } else {
            ToggleResult::NotMarkable
        }
    }

    /// Preview what changes would occur if marking a package
    pub fn preview_mark(&mut self, name: &str) -> PreviewResult {
        // Get current state before marking
        let before: HashSet<String> = self.apt
            .get_changes()
            .map(|p| p.name().to_string())
            .collect();

        // Temporarily mark to see what would happen
        if let Some(pkg) = self.apt.get(name) {
            pkg.mark_install(true, true);
            pkg.protect();
        }

        // Resolve dependencies
        if let Err(e) = self.apt.resolve() {
            self.restore_marks();
            return PreviewResult::Error(format!("Dependency conflict: {}", format_apt_errors(&e)));
        }

        // Calculate diff
        let mut additional_installs = Vec::new();
        let mut additional_upgrades = Vec::new();
        let mut additional_removes = Vec::new();
        let mut download_size: u64 = 0;

        for pkg in self.apt.get_changes() {
            let pkg_name = pkg.name().to_string();

            if pkg_name == name {
                if let Some(cand) = pkg.candidate() {
                    download_size += cand.size();
                }
                continue;
            }

            if !before.contains(&pkg_name) {
                if pkg.marked_install() || pkg.marked_upgrade() {
                    if let Some(cand) = pkg.candidate() {
                        download_size += cand.size();
                    }
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

        // Restore cache state
        self.restore_marks();

        // Store preview
        self.mark_preview = MarkPreview {
            package_name: name.to_string(),
            is_marking: true,
            additional_installs,
            additional_upgrades,
            additional_removes,
            download_size,
        };

        if self.mark_preview.has_additional_changes() {
            PreviewResult::NeedsConfirmation
        } else {
            PreviewResult::NoAdditionalChanges
        }
    }

    /// Confirm the pending mark operation
    pub fn confirm_mark(&mut self) {
        let name = self.mark_preview.package_name.clone();
        self.apt.mark_install(&name);
        let _ = self.apt.resolve();
        self.mark_preview = MarkPreview::default();
        self.pending = self.apt.calculate_pending();
    }

    /// Cancel the pending mark operation
    pub fn cancel_mark(&mut self) {
        self.mark_preview = MarkPreview::default();
        self.pending = self.apt.calculate_pending();
    }

    /// Mark all upgradable packages
    pub fn mark_all_upgrades(&mut self) -> Result<(), String> {
        let upgradable: Vec<String> = self.apt
            .packages(&PackageSort::default())
            .filter(|p| p.is_upgradable())
            .map(|p| p.name().to_string())
            .collect();

        for name in upgradable {
            self.apt.mark_install(&name);
        }

        if let Err(e) = self.apt.resolve() {
            return Err(format!("Dependency conflict: {}", format_apt_errors(&e)));
        }

        self.pending = self.apt.calculate_pending();
        Ok(())
    }

    /// Unmark all packages
    pub fn unmark_all(&mut self) {
        // Clear user marks first, then restore (which will now restore to nothing)
        self.apt.clear_user_marks();
        self.apt.restore_to_user_marks();
        self.pending = self.apt.calculate_pending();
    }

    /// Mark multiple packages (for visual mode)
    pub fn mark_packages(&mut self, names: &[String]) -> Result<(), String> {
        // Filter to only markable packages
        let to_mark: Vec<String> = names.iter()
            .filter_map(|name| {
                if let Some(pkg) = self.apt.get(name) {
                    if pkg.is_upgradable() || !pkg.is_installed() {
                        return Some(name.clone());
                    }
                }
                None
            })
            .collect();

        for name in &to_mark {
            self.apt.mark_install(name);
        }

        if let Err(e) = self.apt.resolve() {
            return Err(format!("Dependency conflict: {}", format_apt_errors(&e)));
        }

        self.pending = self.apt.calculate_pending();
        Ok(())
    }

    /// Restore marks to user-marked state (fast, no cache reload)
    fn restore_marks(&mut self) {
        self.apt.restore_to_user_marks();
    }

    // === Dependency Queries ===

    /// Get forward dependencies for a package
    pub fn get_dependencies(&self, name: &str) -> Vec<(String, String)> {
        self.apt.get_dependencies(name)
    }

    /// Get reverse dependencies for a package
    pub fn get_reverse_dependencies(&self, name: &str) -> Vec<(String, String)> {
        self.apt.get_reverse_dependencies(name)
    }

    /// Get status for a package by name
    pub fn get_package_status(&self, name: &str) -> PackageStatus {
        self.apt.get_package_status(name)
    }

    // === Pending Changes ===

    /// Recalculate pending changes
    pub fn calculate_pending(&mut self) {
        self.pending = self.apt.calculate_pending();
    }

    /// Check if there are pending changes
    #[must_use]
    pub fn has_pending_changes(&self) -> bool {
        !self.pending.to_install.is_empty()
            || !self.pending.to_upgrade.is_empty()
            || !self.pending.to_remove.is_empty()
            || !self.pending.auto_install.is_empty()
            || !self.pending.auto_remove.is_empty()
    }

    /// Get total count of pending changes
    #[must_use]
    pub fn total_changes_count(&self) -> usize {
        self.pending.to_install.len()
            + self.pending.to_upgrade.len()
            + self.pending.to_remove.len()
            + self.pending.auto_install.len()
            + self.pending.auto_remove.len()
    }

    /// Update upgradable count
    pub fn update_upgradable_count(&mut self) {
        self.upgradable_count = self.apt.count_upgradable();
    }

    // === System Operations ===

    /// Refresh the APT cache
    pub fn refresh(&mut self) -> Result<(), String> {
        if let Some(msg) = Self::check_apt_lock() {
            return Err(msg);
        }

        self.apt.full_refresh().map_err(|e| e.to_string())?;
        self.pending = PendingChanges::default();
        self.search.index = None;
        self.search.query.clear();
        self.search.results = None;
        self.update_upgradable_count();
        Ok(())
    }

    /// Commit pending changes using native APT
    pub fn commit(&mut self) -> Result<()> {
        self.apt.commit()?;
        self.pending = PendingChanges::default();
        self.search.index = None;
        Ok(())
    }

    /// Fetch changelog for a package
    pub fn fetch_changelog(&self, name: &str) -> Result<Vec<String>, String> {
        match std::process::Command::new("apt")
            .args(["changelog", name])
            .output()
        {
            Ok(output) => {
                if output.status.success() {
                    let content = String::from_utf8_lossy(&output.stdout);
                    let lines: Vec<String> = content.lines().map(|s| s.to_string()).collect();
                    if lines.is_empty() {
                        Ok(vec!["No changelog available.".to_string()])
                    } else {
                        Ok(lines)
                    }
                } else {
                    let err = String::from_utf8_lossy(&output.stderr);
                    Err(format!("Error: {}", err))
                }
            }
            Err(e) => Err(format!("Failed to run apt changelog: {}", e)),
        }
    }

    // === Utilities ===

    /// Check if running as root
    #[must_use]
    pub fn is_root() -> bool {
        unsafe { libc::geteuid() == 0 }
    }

    /// Check if APT lock files are held by another process
    pub fn check_apt_lock() -> Option<String> {
        let lock_paths = [
            "/var/lib/dpkg/lock-frontend",
            "/var/lib/dpkg/lock",
            "/var/lib/apt/lists/lock",
        ];

        for path in &lock_paths {
            if let Ok(file) = File::open(path) {
                let fd = file.as_raw_fd();
                let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
                if ret != 0 {
                    return Some(format!(
                        "Another package manager is running ({}). Close it and try again.",
                        path
                    ));
                }
                unsafe { libc::flock(fd, libc::LOCK_UN) };
            }
        }
        None
    }
}

// Helper trait for MarkPreview
impl MarkPreview {
    /// Check if there are any additional changes
    pub fn has_additional_changes(&self) -> bool {
        !self.additional_installs.is_empty()
            || !self.additional_upgrades.is_empty()
            || !self.additional_removes.is_empty()
    }
}
