//! Core business logic - Typestate Package Manager
//!
//! This module implements a typestate-based package manager where:
//! - `user_intent: HashMap<PackageId, UserIntent>` is the single source of truth
//! - APT marks are derived from intent via `plan()`
//! - State transitions are enforced at compile time

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::marker::PhantomData;
use std::os::unix::io::AsRawFd;

use color_eyre::Result;
use rust_apt::cache::PackageSort;

use crate::apt::{AptCache, format_apt_errors};
use crate::search::SearchIndex;
use crate::types::*;

// ============================================================================
// Search State (shared across all manager states)
// ============================================================================

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

// ============================================================================
// Shared State (across all typestate variants)
// ============================================================================

/// State shared across all PackageManager states.
/// Fields are private to prevent bypassing the typestate API.
struct SharedState {
    cache: AptCache,
    user_intent: HashMap<PackageId, UserIntent>,
    search: SearchState,
    list: Vec<PackageInfo>,
    upgradable_count: usize,
    installed_count: usize,
    total_count: usize,
    selected_filter: FilterCategory,
    sort_settings: SortSettings,
}

impl SharedState {
    fn new(cache: AptCache) -> Self {
        Self {
            cache,
            user_intent: HashMap::new(),
            search: SearchState::default(),
            list: Vec::new(),
            upgradable_count: 0,
            installed_count: 0,
            total_count: 0,
            selected_filter: FilterCategory::Upgradable,
            sort_settings: SortSettings::default(),
        }
    }

    /// Compute and cache package counts from the APT cache
    fn compute_cache_counts(&mut self) {
        self.upgradable_count = 0;
        self.installed_count = 0;
        self.total_count = 0;

        for pkg in self.cache.packages(&PackageSort::default()) {
            self.total_count += 1;
            if pkg.is_installed() {
                self.installed_count += 1;
                if pkg.is_upgradable() {
                    self.upgradable_count += 1;
                }
            }
        }
    }
}

// ============================================================================
// Typestate Package Manager
// ============================================================================

/// Package manager with compile-time state tracking
pub struct PackageManager<S> {
    shared: SharedState,
    state: S,
    _phantom: PhantomData<S>,
}

// Clean state - no user marks
impl PackageManager<Clean> {
    /// Create a new PackageManager in Clean state
    pub fn new() -> Result<Self> {
        let cache = AptCache::new()?;
        let mut shared = SharedState::new(cache);
        shared.compute_cache_counts();

        let mut mgr = Self {
            shared,
            state: Clean,
            _phantom: PhantomData,
        };
        mgr.rebuild_list();
        Ok(mgr)
    }

    /// Mark a package for install/upgrade, transitioning to Dirty
    pub fn mark_install(mut self, id: PackageId) -> PackageManager<Dirty> {
        self.shared.user_intent.insert(id, UserIntent::Install);
        PackageManager {
            shared: self.shared,
            state: Dirty,
            _phantom: PhantomData,
        }
    }

    /// Mark a package for removal, transitioning to Dirty
    pub fn mark_remove(mut self, id: PackageId) -> PackageManager<Dirty> {
        self.shared.user_intent.insert(id, UserIntent::Remove);
        PackageManager {
            shared: self.shared,
            state: Dirty,
            _phantom: PhantomData,
        }
    }
}

// Dirty state - has user marks, no computed plan
impl PackageManager<Dirty> {
    /// Mark a package for install/upgrade
    pub fn mark_install(mut self, id: PackageId) -> Self {
        self.shared.user_intent.insert(id, UserIntent::Install);
        self
    }

    /// Mark a package for removal
    pub fn mark_remove(mut self, id: PackageId) -> Self {
        self.shared.user_intent.insert(id, UserIntent::Remove);
        self
    }

    /// Unmark a package (remove user intent)
    pub fn unmark(mut self, id: PackageId) -> Self {
        self.shared.user_intent.remove(&id);
        self
    }

    /// Set intent for a package
    pub fn set_intent(mut self, id: PackageId, intent: UserIntent) -> Self {
        match intent {
            UserIntent::Default => { self.shared.user_intent.remove(&id); }
            _ => { self.shared.user_intent.insert(id, intent); }
        }
        self
    }

    /// Reset all marks, returning to Clean state
    pub fn reset(mut self) -> PackageManager<Clean> {
        self.shared.user_intent.clear();
        self.shared.cache.clear_all_marks();
        PackageManager {
            shared: self.shared,
            state: Clean,
            _phantom: PhantomData,
        }
    }

    /// Compute plan from user intent, transitioning to Planned
    pub fn plan(mut self) -> PackageManager<Planned> {
        // 1. Clear all APT marks
        self.shared.cache.clear_all_marks();

        // 2. Apply user intent to APT cache
        for (&id, &intent) in &self.shared.user_intent {
            match intent {
                UserIntent::Install => self.shared.cache.mark_install_id(id),
                UserIntent::Remove => self.shared.cache.mark_delete_id(id),
                UserIntent::Hold => self.shared.cache.mark_keep_id(id),
                UserIntent::Default => {}
            }
        }

        // 3. Resolve dependencies
        let errors = match self.shared.cache.resolve() {
            Ok(()) => Vec::new(),
            Err(e) => vec![format_apt_errors(&e)],
        };

        // 4. Build changeset from APT state
        // Collect package data first to avoid borrow conflict
        let change_data: Vec<_> = self.shared.cache.get_changes()
            .map(|pkg| {
                let fullname = pkg.fullname(false);
                let is_installed = pkg.is_installed();
                let marked_install = pkg.marked_install();
                let marked_upgrade = pkg.marked_upgrade();
                let marked_delete = pkg.marked_delete();
                let candidate_info = pkg.candidate().map(|c| (c.size(), c.installed_size()));
                (fullname, is_installed, marked_install, marked_upgrade, marked_delete, candidate_info)
            })
            .collect();

        let mut changes = Vec::new();
        let mut download_size = 0u64;
        let mut install_size_change = 0i64;

        for (fullname, is_installed, marked_install, marked_upgrade, marked_delete, candidate_info) in change_data {
            // Use FULL name for ID lookup - PackageId maps to full names
            let id = self.shared.cache.id_for(&fullname);

            let is_user_requested = self.shared.user_intent.contains_key(&id);

            let (action, reason) = if marked_install || marked_upgrade {
                let action = if is_installed {
                    ChangeAction::Upgrade
                } else {
                    ChangeAction::Install
                };
                let reason = if is_user_requested {
                    ChangeReason::UserRequested
                } else {
                    ChangeReason::Dependency
                };
                (action, reason)
            } else if marked_delete {
                let reason = if is_user_requested {
                    ChangeReason::UserRequested
                } else {
                    ChangeReason::AutoRemove
                };
                (ChangeAction::Remove, reason)
            } else {
                continue;
            };

            let (pkg_download, pkg_size_change) = if let Some((dl_size, inst_size)) = candidate_info {
                let sz = if action == ChangeAction::Remove {
                    -(inst_size as i64)
                } else {
                    inst_size as i64
                };
                (dl_size, sz)
            } else {
                (0, 0)
            };

            download_size += pkg_download;
            install_size_change += pkg_size_change;

            // PlannedChange only stores PackageId - name is derived when needed
            changes.push(PlannedChange {
                package: id,
                action,
                reason,
                download_size: pkg_download,
                size_change: pkg_size_change,
            });
        }

        let planned = Planned {
            changes,
            download_size,
            install_size_change,
            errors,
        };

        PackageManager {
            shared: self.shared,
            state: planned,
            _phantom: PhantomData,
        }
    }

    /// Check if there are any user marks
    pub fn has_marks(&self) -> bool {
        !self.shared.user_intent.is_empty()
    }
}

// Planned state - dependencies resolved, changeset computed
impl PackageManager<Planned> {
    /// Get the computed changes
    pub fn changes(&self) -> &[PlannedChange] {
        &self.state.changes
    }

    /// Apply planned changes to update package statuses in the list.
    /// No distinction between user-marked and dependency - all marked packages look the same.
    pub fn apply_planned_statuses(&mut self) {
        // Build a map of PackageId -> action from planned changes
        let change_map: HashMap<PackageId, ChangeAction> = self.state.changes
            .iter()
            .map(|c| (c.package, c.action))
            .collect();

        // Update statuses in the list - no user vs dependency distinction
        for pkg in &mut self.shared.list {
            if let Some(&action) = change_map.get(&pkg.id) {
                pkg.status = match action {
                    ChangeAction::Install => PackageStatus::MarkedForInstall,
                    ChangeAction::Upgrade => PackageStatus::MarkedForUpgrade,
                    ChangeAction::Remove => PackageStatus::MarkedForRemove,
                    ChangeAction::Downgrade => PackageStatus::MarkedForUpgrade,
                };
            }
        }
    }

    /// Get total download size
    pub fn download_size(&self) -> u64 {
        self.state.download_size
    }

    /// Get install size change
    pub fn install_size_change(&self) -> i64 {
        self.state.install_size_change
    }

    /// Check if there are planning errors
    pub fn has_errors(&self) -> bool {
        !self.state.errors.is_empty()
    }

    /// Get planning errors
    pub fn errors(&self) -> &[String] {
        &self.state.errors
    }

    /// Go back to modify marks (keeps marks, discards plan)
    pub fn modify(self) -> PackageManager<Dirty> {
        PackageManager {
            shared: self.shared,
            state: Dirty,
            _phantom: PhantomData,
        }
    }

    /// Commit the changes to the system
    pub fn commit(mut self) -> Result<PackageManager<Clean>> {
        self.shared.cache.commit()?;
        self.shared.user_intent.clear();
        self.shared.search.index = None; // Invalidate search index

        Ok(PackageManager {
            shared: self.shared,
            state: Clean,
            _phantom: PhantomData,
        })
    }

    /// Commit the changes using caller-provided progress implementations
    pub fn commit_with_progress(
        mut self,
        acquire_progress: &mut rust_apt::progress::AcquireProgress,
        install_progress: &mut rust_apt::progress::InstallProgress,
    ) -> Result<PackageManager<Clean>> {
        self.shared.cache.commit_with_progress(acquire_progress, install_progress)?;
        self.shared.user_intent.clear();
        self.shared.search.index = None;

        Ok(PackageManager {
            shared: self.shared,
            state: Clean,
            _phantom: PhantomData,
        })
    }
}

// ============================================================================
// Shared functionality (all states)
// ============================================================================

impl<S: ReadableState> PackageManager<S> {
    /// Get the shared state (read-only for most things)
    pub fn shared(&self) -> &SharedState {
        &self.shared
    }

    /// Get mutable access to shared state
    pub fn shared_mut(&mut self) -> &mut SharedState {
        &mut self.shared
    }

    /// Get a package by index in current list
    pub fn get_package(&self, index: usize) -> Option<&PackageInfo> {
        self.shared.list.get(index)
    }

    /// Get a package by PackageId
    pub fn get_package_by_id(&self, id: PackageId) -> Option<&PackageInfo> {
        self.shared.list.iter().find(|p| p.id == id)
    }

    /// Get number of packages in current list
    pub fn package_count(&self) -> usize {
        self.shared.list.len()
    }

    /// Get user intent for a package
    pub fn user_intent(&self, id: PackageId) -> UserIntent {
        self.shared.user_intent.get(&id).copied().unwrap_or(UserIntent::Default)
    }

    /// Check if a package is user-marked
    pub fn is_user_marked(&self, id: PackageId) -> bool {
        self.shared.user_intent.contains_key(&id)
    }

    /// Get current filter
    pub fn selected_filter(&self) -> FilterCategory {
        self.shared.selected_filter
    }

    /// Get upgradable count
    pub fn upgradable_count(&self) -> usize {
        self.shared.upgradable_count
    }

    /// Get search query
    pub fn search_query(&self) -> &str {
        &self.shared.search.query
    }

    /// Get search result count
    pub fn search_result_count(&self) -> Option<usize> {
        self.shared.search.results.as_ref().map(std::collections::HashSet::len)
    }

    // === Filtering & Listing ===

    /// Apply a filter category and rebuild the package list
    pub fn apply_filter(&mut self, filter: FilterCategory) {
        self.shared.selected_filter = filter;
        self.rebuild_list();
    }

    /// Rebuild the package list based on current filter and search
    pub fn rebuild_list(&mut self) -> ColumnWidths {
        self.shared.list.clear();

        let sort = if self.shared.selected_filter == FilterCategory::Upgradable {
            PackageSort::default().upgradable()
        } else {
            PackageSort::default()
        };

        // First pass: collect package full names that match filters
        // (avoids borrow conflict between cache iteration and extract_package_info)
        // Use full names to properly handle multi-arch packages
        let matching_fullnames: Vec<String> = {
            let search_results = &self.shared.search.results;
            let user_intent = &self.shared.user_intent;
            let fullname_to_id = &self.shared.cache.fullname_to_id;

            self.shared.cache.packages(&sort)
                .filter(|pkg| {
                    let matches_category = match self.shared.selected_filter {
                        FilterCategory::Upgradable => pkg.is_upgradable(),
                        FilterCategory::MarkedChanges => {
                            // Check both user_intent (works in Dirty state) and
                            // APT marks (works in Planned state for dependencies)
                            let has_user_intent = fullname_to_id.get(&pkg.fullname(false))
                                .map(|id| user_intent.contains_key(id))
                                .unwrap_or(false);
                            has_user_intent || pkg.marked_install() || pkg.marked_upgrade() || pkg.marked_delete()
                        }
                        FilterCategory::Installed => pkg.is_installed(),
                        FilterCategory::NotInstalled => !pkg.is_installed(),
                        FilterCategory::All => true,
                    };

                    // Search matches base name for user convenience
                    let matches_search = match search_results {
                        Some(results) => results.contains(pkg.name()),
                        None => true,
                    };

                    matches_category && matches_search
                })
                .map(|pkg| pkg.fullname(false))
                .collect()
        };

        // Second pass: extract full package info
        for fullname in matching_fullnames {
            if let Some(mut info) = self.shared.cache.extract_package_info_by_name(&fullname) {
                // Update status based on user intent
                if let Some(&intent) = self.shared.user_intent.get(&info.id) {
                    info.status = match intent {
                        UserIntent::Install => {
                            if info.status == PackageStatus::Upgradable {
                                PackageStatus::MarkedForUpgrade
                            } else {
                                PackageStatus::MarkedForInstall
                            }
                        }
                        UserIntent::Remove => PackageStatus::MarkedForRemove,
                        UserIntent::Hold => PackageStatus::Keep,
                        UserIntent::Default => info.status,
                    };
                }
                self.shared.list.push(info);
            }
        }

        // Calculate column widths (use display name length, not full name)
        let mut col_widths = ColumnWidths::new();
        for pkg in &self.shared.list {
            let display_len = self.shared.cache.display_name(&pkg.name).len() as u16;
            col_widths.name = col_widths.name.max(display_len);
            col_widths.section = col_widths.section.max(pkg.section.len() as u16);
            col_widths.installed = col_widths.installed.max(pkg.installed_version.len() as u16);
            col_widths.candidate = col_widths.candidate.max(pkg.candidate_version.len() as u16);
        }

        self.sort_list();
        col_widths
    }

    /// Sort the package list
    fn sort_list(&mut self) {
        let sort_by = self.shared.sort_settings.sort_by;
        let ascending = self.shared.sort_settings.ascending;

        self.shared.list.sort_by(|a, b| {
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
        self.shared.sort_settings.sort_by = sort_by;
        self.shared.sort_settings.ascending = ascending;
        self.sort_list();
    }

    // === Search ===

    /// Ensure search index is built
    pub fn ensure_search_index(&mut self) -> Result<std::time::Duration> {
        if self.shared.search.index.is_none() {
            let mut index = SearchIndex::new()?;
            let (_count, duration) = index.build(&self.shared.cache)?;
            self.shared.search.index = Some(index);
            return Ok(duration);
        }
        Ok(std::time::Duration::ZERO)
    }

    /// Set search query and execute search
    pub fn set_search_query(&mut self, query: &str) -> Result<()> {
        self.shared.search.query = query.to_string();

        if query.is_empty() {
            self.shared.search.results = None;
        } else if let Some(ref index) = self.shared.search.index {
            self.shared.search.results = Some(index.search(query)?);
        }
        Ok(())
    }

    /// Clear search query and results
    pub fn clear_search(&mut self) {
        self.shared.search.query.clear();
        self.shared.search.results = None;
    }

    // === Dependency Queries ===

    /// Get forward dependencies for a package
    pub fn get_dependencies(&self, name: &str) -> Vec<(String, String)> {
        self.shared.cache.get_dependencies(name)
    }

    /// Get reverse dependencies for a package
    pub fn get_reverse_dependencies(&self, name: &str) -> Vec<(String, String)> {
        self.shared.cache.get_reverse_dependencies(name)
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
                    let lines: Vec<String> = content.lines().map(std::string::ToString::to_string).collect();
                    if lines.is_empty() {
                        Ok(vec!["No changelog available.".to_string()])
                    } else {
                        Ok(lines)
                    }
                } else {
                    let err = String::from_utf8_lossy(&output.stderr);
                    Err(format!("Error: {err}"))
                }
            }
            Err(e) => Err(format!("Failed to run apt changelog: {e}")),
        }
    }

    /// Update all cached package counts
    pub fn update_cache_counts(&mut self) {
        self.shared.compute_cache_counts();
    }

    /// Refresh the APT cache
    pub fn refresh(&mut self) -> Result<(), String> {
        if let Some(msg) = check_apt_lock() {
            return Err(msg);
        }

        self.shared.cache.refresh().map_err(|e| e.to_string())?;
        self.shared.user_intent.clear();
        self.shared.search.index = None;
        self.shared.search.query.clear();
        self.shared.search.results = None;
        self.update_cache_counts();
        Ok(())
    }

    /// Run `apt update` with caller-provided progress, then refresh
    pub fn update_with_progress(
        &mut self,
        acquire_progress: &mut rust_apt::progress::AcquireProgress,
    ) -> Result<(), String> {
        if let Some(msg) = check_apt_lock() {
            return Err(msg);
        }

        self.shared.cache.update_with_progress(acquire_progress)
            .map_err(|e| e.to_string())?;
        self.shared.user_intent.clear();
        self.shared.search.index = None;
        self.shared.search.query.clear();
        self.shared.search.results = None;
        self.update_cache_counts();
        Ok(())
    }
}

// ============================================================================
// Manager Wrapper (for TUI that can't hold consuming-self types)
// ============================================================================

/// Wrapper enum that allows mutable access without consuming self
/// This is necessary for TUI where we can't easily handle typestate transitions
#[derive(Default)]
pub enum ManagerState {
    Clean(PackageManager<Clean>),
    Dirty(PackageManager<Dirty>),
    Planned(PackageManager<Planned>),
    /// Temporary placeholder during state transitions (never observed externally)
    #[default]
    Transitioning,
}


impl ManagerState {
    /// Create a new manager in Clean state
    pub fn new() -> Result<Self> {
        Ok(ManagerState::Clean(PackageManager::new()?))
    }

    /// Check if in Clean state
    pub fn is_clean(&self) -> bool {
        matches!(self, ManagerState::Clean(_))
    }

    /// Check if in Dirty state
    pub fn is_dirty(&self) -> bool {
        matches!(self, ManagerState::Dirty(_))
    }

    /// Check if in Planned state
    pub fn is_planned(&self) -> bool {
        matches!(self, ManagerState::Planned(_))
    }

    /// Get the planned changes (only valid in Planned state)
    pub fn planned_changes(&self) -> Option<&[PlannedChange]> {
        match self {
            ManagerState::Planned(m) => Some(m.changes()),
            _ => None,
        }
    }

    /// Get plan errors (only valid in Planned state)
    pub fn plan_errors(&self) -> Option<&[String]> {
        match self {
            ManagerState::Planned(m) => Some(m.errors()),
            _ => None,
        }
    }

    /// Check if we have any user intent (marks)
    pub fn has_marks(&self) -> bool {
        match self {
            ManagerState::Clean(_) => false,
            ManagerState::Dirty(m) => m.has_marks(),
            ManagerState::Planned(m) => !m.shared.user_intent.is_empty(),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    // Accessor methods that work in any state

    pub fn list(&self) -> &[PackageInfo] {
        match self {
            ManagerState::Clean(m) => &m.shared.list,
            ManagerState::Dirty(m) => &m.shared.list,
            ManagerState::Planned(m) => &m.shared.list,
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn get_package(&self, index: usize) -> Option<&PackageInfo> {
        match self {
            ManagerState::Clean(m) => m.get_package(index),
            ManagerState::Dirty(m) => m.get_package(index),
            ManagerState::Planned(m) => m.get_package(index),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn package_count(&self) -> usize {
        match self {
            ManagerState::Clean(m) => m.package_count(),
            ManagerState::Dirty(m) => m.package_count(),
            ManagerState::Planned(m) => m.package_count(),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn user_intent(&self, id: PackageId) -> UserIntent {
        match self {
            ManagerState::Clean(m) => m.user_intent(id),
            ManagerState::Dirty(m) => m.user_intent(id),
            ManagerState::Planned(m) => m.user_intent(id),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn is_user_marked(&self, id: PackageId) -> bool {
        match self {
            ManagerState::Clean(m) => m.is_user_marked(id),
            ManagerState::Dirty(m) => m.is_user_marked(id),
            ManagerState::Planned(m) => m.is_user_marked(id),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn selected_filter(&self) -> FilterCategory {
        match self {
            ManagerState::Clean(m) => m.selected_filter(),
            ManagerState::Dirty(m) => m.selected_filter(),
            ManagerState::Planned(m) => m.selected_filter(),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn upgradable_count(&self) -> usize {
        match self {
            ManagerState::Clean(m) => m.upgradable_count(),
            ManagerState::Dirty(m) => m.upgradable_count(),
            ManagerState::Planned(m) => m.upgradable_count(),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn search_query(&self) -> &str {
        match self {
            ManagerState::Clean(m) => m.search_query(),
            ManagerState::Dirty(m) => m.search_query(),
            ManagerState::Planned(m) => m.search_query(),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn search_result_count(&self) -> Option<usize> {
        match self {
            ManagerState::Clean(m) => m.search_result_count(),
            ManagerState::Dirty(m) => m.search_result_count(),
            ManagerState::Planned(m) => m.search_result_count(),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn get_dependencies(&self, name: &str) -> Vec<(String, String)> {
        match self {
            ManagerState::Clean(m) => m.get_dependencies(name),
            ManagerState::Dirty(m) => m.get_dependencies(name),
            ManagerState::Planned(m) => m.get_dependencies(name),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn get_reverse_dependencies(&self, name: &str) -> Vec<(String, String)> {
        match self {
            ManagerState::Clean(m) => m.get_reverse_dependencies(name),
            ManagerState::Dirty(m) => m.get_reverse_dependencies(name),
            ManagerState::Planned(m) => m.get_reverse_dependencies(name),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn fetch_changelog(&self, name: &str) -> Result<Vec<String>, String> {
        match self {
            ManagerState::Clean(m) => m.fetch_changelog(name),
            ManagerState::Dirty(m) => m.fetch_changelog(name),
            ManagerState::Planned(m) => m.fetch_changelog(name),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    // Mutating methods that work in any state

    pub fn apply_filter(&mut self, filter: FilterCategory) {
        match self {
            ManagerState::Clean(m) => m.apply_filter(filter),
            ManagerState::Dirty(m) => m.apply_filter(filter),
            ManagerState::Planned(m) => m.apply_filter(filter),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn rebuild_list(&mut self) -> ColumnWidths {
        match self {
            ManagerState::Clean(m) => m.rebuild_list(),
            ManagerState::Dirty(m) => m.rebuild_list(),
            ManagerState::Planned(m) => {
                let col_widths = m.rebuild_list();
                // Apply planned changes to update statuses for dependencies
                m.apply_planned_statuses();
                col_widths
            }
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn set_sort(&mut self, sort_by: SortBy, ascending: bool) {
        match self {
            ManagerState::Clean(m) => m.set_sort(sort_by, ascending),
            ManagerState::Dirty(m) => m.set_sort(sort_by, ascending),
            ManagerState::Planned(m) => m.set_sort(sort_by, ascending),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn ensure_search_index(&mut self) -> Result<std::time::Duration> {
        match self {
            ManagerState::Clean(m) => m.ensure_search_index(),
            ManagerState::Dirty(m) => m.ensure_search_index(),
            ManagerState::Planned(m) => m.ensure_search_index(),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn set_search_query(&mut self, query: &str) -> Result<()> {
        match self {
            ManagerState::Clean(m) => m.set_search_query(query),
            ManagerState::Dirty(m) => m.set_search_query(query),
            ManagerState::Planned(m) => m.set_search_query(query),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn clear_search(&mut self) {
        match self {
            ManagerState::Clean(m) => m.clear_search(),
            ManagerState::Dirty(m) => m.clear_search(),
            ManagerState::Planned(m) => m.clear_search(),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn has_search_results(&self) -> bool {
        match self {
            ManagerState::Clean(m) => m.shared.search.results.is_some(),
            ManagerState::Dirty(m) => m.shared.search.results.is_some(),
            ManagerState::Planned(m) => m.shared.search.results.is_some(),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn search_query_pop(&mut self) {
        match self {
            ManagerState::Clean(m) => { m.shared.search.query.pop(); }
            ManagerState::Dirty(m) => { m.shared.search.query.pop(); }
            ManagerState::Planned(m) => { m.shared.search.query.pop(); }
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn search_query_push(&mut self, c: char) {
        match self {
            ManagerState::Clean(m) => m.shared.search.query.push(c),
            ManagerState::Dirty(m) => m.shared.search.query.push(c),
            ManagerState::Planned(m) => m.shared.search.query.push(c),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn refresh(&mut self) -> Result<(), String> {
        match self {
            ManagerState::Clean(m) => m.refresh(),
            ManagerState::Dirty(m) => m.refresh(),
            ManagerState::Planned(m) => m.refresh(),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    pub fn update_cache_counts(&mut self) {
        match self {
            ManagerState::Clean(m) => m.update_cache_counts(),
            ManagerState::Dirty(m) => m.update_cache_counts(),
            ManagerState::Planned(m) => m.update_cache_counts(),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    /// Get the count for a filter category
    pub fn filter_count(&self, filter: FilterCategory) -> usize {
        let (upgradable, installed, total, user_marks) = match self {
            ManagerState::Clean(m) => (m.shared.upgradable_count, m.shared.installed_count, m.shared.total_count, m.shared.user_intent.len()),
            ManagerState::Dirty(m) => (m.shared.upgradable_count, m.shared.installed_count, m.shared.total_count, m.shared.user_intent.len()),
            ManagerState::Planned(m) => (m.shared.upgradable_count, m.shared.installed_count, m.shared.total_count, m.shared.user_intent.len()),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        };

        match filter {
            FilterCategory::Upgradable => upgradable,
            FilterCategory::MarkedChanges => user_marks,
            FilterCategory::Installed => installed,
            FilterCategory::NotInstalled => total - installed,
            FilterCategory::All => total,
        }
    }

    /// Mark all upgradable packages in the entire cache (not just filtered view)
    pub fn mark_all_upgradable(&mut self) {
        let upgradable_ids: Vec<PackageId> = {
            let cache = self.cache();
            cache.packages(&PackageSort::default().upgradable())
                .map(|pkg| pkg.fullname(false))
                .filter_map(|name| cache.get_id(&name))
                .collect()
        };

        for id in upgradable_ids {
            self.mark_install(id);
        }
    }

    // State-specific shared access

    pub fn sort_settings(&self) -> &SortSettings {
        match self {
            ManagerState::Clean(m) => &m.shared.sort_settings,
            ManagerState::Dirty(m) => &m.shared.sort_settings,
            ManagerState::Planned(m) => &m.shared.sort_settings,
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    /// Get reference to the APT cache for ID lookups
    pub fn cache(&self) -> &AptCache {
        match self {
            ManagerState::Clean(m) => &m.shared.cache,
            ManagerState::Dirty(m) => &m.shared.cache,
            ManagerState::Planned(m) => &m.shared.cache,
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }
}

// ============================================================================
// Macros for reducing boilerplate in ManagerState
// ============================================================================

/// Helper to take ownership and perform state transition
impl ManagerState {
    /// Mark a package for install, handling state transitions
    pub fn mark_install(&mut self, id: PackageId) {
        *self = match std::mem::take(self) {
            ManagerState::Clean(m) => ManagerState::Dirty(m.mark_install(id)),
            ManagerState::Dirty(m) => ManagerState::Dirty(m.mark_install(id)),
            ManagerState::Planned(m) => ManagerState::Dirty(m.modify().mark_install(id)),
            ManagerState::Transitioning => panic!("ManagerState::Transitioning should not be observed"),
        };
    }

    /// Mark a package for removal, handling state transitions
    pub fn mark_remove(&mut self, id: PackageId) {
        *self = match std::mem::take(self) {
            ManagerState::Clean(m) => ManagerState::Dirty(m.mark_remove(id)),
            ManagerState::Dirty(m) => ManagerState::Dirty(m.mark_remove(id)),
            ManagerState::Planned(m) => ManagerState::Dirty(m.modify().mark_remove(id)),
            ManagerState::Transitioning => panic!("ManagerState::Transitioning should not be observed"),
        };
    }

    /// Unmark a package from user_intent (low-level, doesn't handle cascade).
    /// For proper toggle behavior with cascade, use `toggle()` instead.
    pub fn unmark(&mut self, id: PackageId) {
        if !self.is_user_marked(id) {
            return;
        }

        *self = match std::mem::take(self) {
            ManagerState::Clean(m) => ManagerState::Clean(m),
            ManagerState::Dirty(m) => ManagerState::Dirty(m.unmark(id)),
            ManagerState::Planned(m) => ManagerState::Dirty(m.modify().unmark(id)),
            ManagerState::Transitioning => panic!("ManagerState::Transitioning should not be observed"),
        };
    }

    /// Toggle a package's mark state with full cascade/orphan handling.
    /// Returns (packages_affected, is_marking) for UI confirmation.
    ///
    /// - If not marked: marks it + computes plan (deps shown in plan)
    /// - If marked (user or dep): unmarks with cascade, returns affected packages
    pub fn toggle(&mut self, id: PackageId) -> ToggleResult {
        // First, compute plan if needed to know current marked state
        self.compute_plan();
        self.rebuild_list();

        // Check if package is currently marked (in the plan)
        let is_currently_marked = self.list().iter()
            .find(|p| p.id == id)
            .map(|p| p.status.is_marked())
            .unwrap_or(false);

        if is_currently_marked {
            // UNMARK flow
            self.toggle_unmark(id)
        } else {
            // MARK flow
            self.toggle_mark_impl(id)
        }
    }

    /// Internal: handle marking a package
    fn toggle_mark_impl(&mut self, id: PackageId) -> ToggleResult {
        // Get marked packages before
        let marked_before: HashSet<PackageId> = self.list().iter()
            .filter(|p| p.status.is_marked())
            .map(|p| p.id)
            .collect();

        // Mark and compute plan
        self.mark_install(id);
        self.compute_plan();
        self.rebuild_list();

        // Find newly marked packages (deps)
        let newly_marked: Vec<PackageId> = self.list().iter()
            .filter(|p| p.status.is_marked() && !marked_before.contains(&p.id) && p.id != id)
            .map(|p| p.id)
            .collect();

        ToggleResult::Marked {
            package: id,
            additional: newly_marked,
        }
    }

    /// Internal: handle unmarking a package with cascade
    fn toggle_unmark(&mut self, id: PackageId) -> ToggleResult {
        // Get all currently marked packages
        let marked_before: Vec<PackageId> = self.list().iter()
            .filter(|p| p.status.is_marked())
            .map(|p| p.id)
            .collect();

        // Determine what to remove from user_intent
        let to_remove: Vec<PackageId> = if self.is_user_marked(id) {
            // Package is user-marked: just remove it
            vec![id]
        } else {
            // Package is a dependency: find user_intent packages that depend on it
            self.find_user_intent_depending_on(id)
        };

        // Remove from user_intent
        for pkg_id in &to_remove {
            self.unmark(*pkg_id);
        }

        // Recompute plan (orphans automatically disappear)
        self.compute_plan();
        self.rebuild_list();

        // Find what got unmarked
        let marked_after: HashSet<PackageId> = self.list().iter()
            .filter(|p| p.status.is_marked())
            .map(|p| p.id)
            .collect();

        // Check if the target package is still marked (unmark failed)
        let target_still_marked = marked_after.contains(&id);

        if target_still_marked {
            // Couldn't unmark - this is a dependency we can't trace back to user_intent
            // Return NoChange to signal the UI should show a message
            return ToggleResult::NoChange { package: id };
        }

        let also_unmarked: Vec<PackageId> = marked_before.iter()
            .filter(|pkg_id| !marked_after.contains(pkg_id) && **pkg_id != id)
            .copied()
            .collect();

        ToggleResult::Unmarked {
            package: id,
            also_unmarked,
        }
    }

    /// Find user_intent packages that (transitively) depend on the given package
    fn find_user_intent_depending_on(&self, target_id: PackageId) -> Vec<PackageId> {
        let cache = self.cache();
        let target_name = match cache.fullname_of(target_id) {
            Some(n) => n,
            None => return Vec::new(),
        };
        let target_base = target_name.split(':').next().unwrap_or(target_name);

        let mut result = Vec::new();

        // Check each user_intent package
        let intent_ids: Vec<PackageId> = self.user_intent_ids().copied().collect();

        for intent_id in intent_ids {
            if let Some(intent_name) = cache.fullname_of(intent_id)
                && self.package_depends_on(intent_name, target_base) {
                    result.push(intent_id);
            }
        }

        result
    }

    /// Check if package A (transitively) depends on package B
    fn package_depends_on(&self, pkg_name: &str, target_base: &str) -> bool {
        let cache = self.cache();
        let mut visited = HashSet::new();
        let mut to_check = vec![pkg_name.to_string()];

        while let Some(current) = to_check.pop() {
            if visited.contains(&current) {
                continue;
            }
            visited.insert(current.clone());

            let deps = cache.get_dependencies(&current);
            for (dep_type, dep_name) in deps {
                if dep_type != "Depends" && dep_type != "PreDepends" {
                    continue;
                }

                if dep_name == target_base {
                    return true;
                }

                // Add to check list for transitive deps
                if let Some(&dep_id) = cache.fullname_to_id.get(&dep_name)
                    .or_else(|| cache.fullname_to_id.get(&format!("{dep_name}:amd64")))
                    && let Some(fullname) = cache.fullname_of(dep_id) {
                        to_check.push(fullname.to_string());
                }
            }
        }

        false
    }

    /// Get iterator over user_intent PackageIds
    fn user_intent_ids(&self) -> impl Iterator<Item = &PackageId> {
        match self {
            ManagerState::Clean(m) => m.shared.user_intent.keys(),
            ManagerState::Dirty(m) => m.shared.user_intent.keys(),
            ManagerState::Planned(m) => m.shared.user_intent.keys(),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    /// Reset all marks
    pub fn reset(&mut self) {
        *self = match std::mem::take(self) {
            ManagerState::Clean(m) => ManagerState::Clean(m),
            ManagerState::Dirty(m) => ManagerState::Clean(m.reset()),
            ManagerState::Planned(m) => ManagerState::Clean(m.modify().reset()),
            ManagerState::Transitioning => panic!("ManagerState::Transitioning should not be observed"),
        };
    }

    /// Compute plan from current marks
    pub fn compute_plan(&mut self) {
        *self = match std::mem::take(self) {
            ManagerState::Clean(m) => ManagerState::Clean(m), // No marks, stay clean
            ManagerState::Dirty(m) => ManagerState::Planned(m.plan()),
            ManagerState::Planned(m) => ManagerState::Planned(m), // Already planned
            ManagerState::Transitioning => panic!("ManagerState::Transitioning should not be observed"),
        };
    }

    /// Commit planned changes
    pub fn commit(&mut self) -> Result<()> {
        *self = match std::mem::take(self) {
            ManagerState::Clean(m) => ManagerState::Clean(m),
            ManagerState::Dirty(m) => {
                // Must plan first
                let planned = m.plan();
                let clean = planned.commit()?;
                ManagerState::Clean(clean)
            }
            ManagerState::Planned(m) => {
                let clean = m.commit()?;
                ManagerState::Clean(clean)
            }
            ManagerState::Transitioning => panic!("ManagerState::Transitioning should not be observed"),
        };
        Ok(())
    }

    /// Commit planned changes with caller-provided progress implementations
    pub fn commit_with_progress(
        &mut self,
        acquire_progress: &mut rust_apt::progress::AcquireProgress,
        install_progress: &mut rust_apt::progress::InstallProgress,
    ) -> Result<()> {
        *self = match std::mem::take(self) {
            ManagerState::Clean(m) => ManagerState::Clean(m),
            ManagerState::Dirty(m) => {
                let planned = m.plan();
                let clean = planned.commit_with_progress(acquire_progress, install_progress)?;
                ManagerState::Clean(clean)
            }
            ManagerState::Planned(m) => {
                let clean = m.commit_with_progress(acquire_progress, install_progress)?;
                ManagerState::Clean(clean)
            }
            ManagerState::Transitioning => panic!("ManagerState::Transitioning should not be observed"),
        };
        Ok(())
    }

    /// Run `apt update` with caller-provided progress
    pub fn update_with_progress(
        &mut self,
        acquire_progress: &mut rust_apt::progress::AcquireProgress,
    ) -> Result<(), String> {
        match self {
            ManagerState::Clean(m) => m.update_with_progress(acquire_progress),
            ManagerState::Dirty(m) => m.update_with_progress(acquire_progress),
            ManagerState::Planned(m) => m.update_with_progress(acquire_progress),
            ManagerState::Transitioning => panic!("Transitioning state observed"),
        }
    }

    /// Build a MarkPreview from the current Planned state's changes.
    /// Call this after marking a package and computing the plan.
    /// `previously_planned` contains PackageIds that were already in the plan
    /// before this mark — they are excluded from the "additional" lists.
    pub fn build_mark_preview(
        &self,
        marked_pkg_id: PackageId,
        previously_planned: &HashSet<PackageId>,
    ) -> Option<MarkPreview> {
        let changes = self.planned_changes()?;
        let cache = self.cache();

        // Use display name (strips native arch suffix)
        let marked_pkg_name = cache.fullname_of(marked_pkg_id)
            .map(|n| cache.display_name(n).to_string())?;

        let mut additional_installs = Vec::new();
        let mut additional_upgrades = Vec::new();
        let mut additional_removes = Vec::new();
        let mut download_size = 0u64;
        let mut is_upgrade = false;

        for change in changes {
            // Check if the marked package is an upgrade vs install
            if change.package == marked_pkg_id {
                download_size += change.download_size;
                is_upgrade = change.action == ChangeAction::Upgrade;
                continue;
            }

            // Skip packages that were already planned before this mark
            if previously_planned.contains(&change.package) {
                continue;
            }

            download_size += change.download_size;

            // Derive display name from PackageId (strips native arch suffix)
            let name = cache.fullname_of(change.package)
                .map(|n| cache.display_name(n).to_string())
                .unwrap_or_else(|| format!("(unknown:{})", change.package.index()));

            match change.action {
                ChangeAction::Install => additional_installs.push(name),
                ChangeAction::Upgrade => additional_upgrades.push(name),
                ChangeAction::Remove => additional_removes.push(name),
                ChangeAction::Downgrade => additional_upgrades.push(name),
            }
        }

        Some(MarkPreview {
            package_name: marked_pkg_name,
            is_upgrade,
            is_marking: true,
            was_user_marked: false, // N/A for mark operations (package wasn't marked before)
            additional_installs,
            additional_upgrades,
            additional_removes,
            download_size,
        })
    }
}

// ============================================================================
// Standalone utility functions
// ============================================================================

/// Check if running as root
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
                    "Another package manager is running ({path}). Close it and try again."
                ));
            }
            unsafe { libc::flock(fd, libc::LOCK_UN) };
        }
    }
    None
}
