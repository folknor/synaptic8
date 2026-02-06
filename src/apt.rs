//! APT cache operations and package management
//!
//! This module provides a thin wrapper around rust-apt with PackageId handles.
//! User intent tracking is handled by the core module, not here.

use std::collections::HashMap;

use color_eyre::Result;
use rust_apt::cache::{Cache, PackageSort};
use rust_apt::error::AptErrors;
use rust_apt::progress::{AcquireProgress, InstallProgress};
use rust_apt::{Package, Version};

use crate::types::*;

/// Manages APT cache interactions with stable PackageId handles.
/// Each unique package (including multi-arch variants) gets its own PackageId.
pub struct AptCache {
    cache: Cache,
    /// Map from package full name (e.g., "libfoo:amd64") to stable PackageId
    pub fullname_to_id: HashMap<String, PackageId>,
    /// Reverse map: PackageId -> full name
    id_to_fullname: Vec<String>,
    /// Native architecture (e.g., "amd64")
    native_arch: String,
    /// Cached suffix for display_name stripping (e.g., ":amd64")
    native_arch_suffix: String,
}

impl AptCache {
    /// Create a new AptCache with a fresh APT cache, pre-populating all PackageIds.
    /// Each multi-arch variant gets its own unique PackageId.
    pub fn new() -> Result<Self> {
        let cache = Cache::new::<&str>(&[])?;

        // Get native architecture from dpkg
        let native_arch = std::process::Command::new("dpkg")
            .arg("--print-architecture")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| "amd64".to_string());
        let mut fullname_to_id = HashMap::new();
        let mut id_to_fullname = Vec::new();

        // Pre-populate all package IDs at creation time using FULL names
        // This properly handles multi-arch: libfoo:amd64 and libfoo:i386 get different IDs
        for pkg in cache.packages(&PackageSort::default()) {
            let fullname = pkg.fullname(false);
            let id = PackageId(id_to_fullname.len() as u32);
            id_to_fullname.push(fullname.clone());
            fullname_to_id.insert(fullname, id);
        }

        let native_arch_suffix = format!(":{native_arch}");

        Ok(Self {
            cache,
            fullname_to_id,
            id_to_fullname,
            native_arch,
            native_arch_suffix,
        })
    }

    /// Get the native architecture (e.g., "amd64")
    pub fn native_arch(&self) -> &str {
        &self.native_arch
    }

    /// Get display name for a package (strips native arch suffix)
    /// "libfoo:amd64" -> "libfoo" (if amd64 is native)
    /// "libfoo:i386" -> "libfoo:i386" (keeps non-native arch)
    pub fn display_name<'a>(&self, fullname: &'a str) -> &'a str {
        fullname.strip_suffix(&self.native_arch_suffix).unwrap_or(fullname)
    }

    // ========================================================================
    // PackageId management
    // ========================================================================

    /// Get or create a stable PackageId for a package full name (e.g., "libfoo:amd64")
    pub fn id_for(&mut self, fullname: &str) -> PackageId {
        if let Some(&id) = self.fullname_to_id.get(fullname) {
            id
        } else {
            let id = PackageId(self.id_to_fullname.len() as u32);
            self.id_to_fullname.push(fullname.to_string());
            self.fullname_to_id.insert(fullname.to_string(), id);
            id
        }
    }

    /// Get the PackageId for a full name (returns None if not known)
    pub fn get_id(&self, fullname: &str) -> Option<PackageId> {
        self.fullname_to_id.get(fullname).copied()
    }

    /// Get the full name for a PackageId
    pub fn fullname_of(&self, id: PackageId) -> Option<&str> {
        self.id_to_fullname.get(id.index()).map(std::string::String::as_str)
    }

    /// Get a package by PackageId
    pub fn get_by_id(&self, id: PackageId) -> Option<Package<'_>> {
        self.fullname_of(id).and_then(|fullname| self.cache.get(fullname))
    }

    /// Get a package by full name (e.g., "libfoo:amd64" or just "libfoo")
    pub fn get(&self, fullname: &str) -> Option<Package<'_>> {
        self.cache.get(fullname)
    }

    // ========================================================================
    // Package iteration
    // ========================================================================

    /// Get an iterator over packages with the given sort
    pub fn packages(&self, sort: &PackageSort) -> impl Iterator<Item = Package<'_>> {
        self.cache.packages(sort)
    }

    /// Get packages with pending changes
    pub fn get_changes(&self) -> impl Iterator<Item = Package<'_>> {
        self.cache.get_changes(false)
    }

    // ========================================================================
    // Marking operations (low-level, crate-only)
    // These are only accessible from core.rs via SharedState.
    // External code must use ManagerState methods which enforce the typestate.
    // ========================================================================

    /// Mark a package for install/upgrade (by name)
    pub(crate) fn mark_install(&self, name: &str) {
        if let Some(pkg) = self.cache.get(name) {
            pkg.mark_install(true, true);
            // Note: We don't call protect() because we manage state through
            // user_intent in core.rs, not through APT's protection mechanism.
            // This allows clear_all_marks() to properly reset the cache.
        }
    }

    /// Mark a package for install/upgrade (by id)
    pub(crate) fn mark_install_id(&self, id: PackageId) {
        if let Some(name) = self.fullname_of(id) {
            self.mark_install(name);
        }
    }

    /// Mark a package for removal (by name)
    pub(crate) fn mark_delete(&self, name: &str) {
        if let Some(pkg) = self.cache.get(name) {
            pkg.mark_delete(false);
            // Note: No protect() - we manage state through user_intent
        }
    }

    /// Mark a package for removal (by id)
    pub(crate) fn mark_delete_id(&self, id: PackageId) {
        if let Some(name) = self.fullname_of(id) {
            self.mark_delete(name);
        }
    }

    /// Mark a package to keep current version (unmark)
    pub(crate) fn mark_keep(&self, name: &str) {
        if let Some(pkg) = self.cache.get(name) {
            pkg.mark_keep();
        }
    }

    /// Mark a package to keep (by id)
    pub(crate) fn mark_keep_id(&self, id: PackageId) {
        if let Some(name) = self.fullname_of(id) {
            self.mark_keep(name);
        }
    }

    /// Clear all marks on all packages
    pub(crate) fn clear_all_marks(&self) {
        // Collect fullnames first to avoid iterator invalidation issues
        let to_clear: Vec<_> = self.cache.get_changes(false)
            .map(|pkg| pkg.fullname(false))
            .collect();

        for fullname in &to_clear {
            if let Some(pkg) = self.cache.get(fullname) {
                pkg.mark_keep();
            }
        }
    }

    /// Resolve dependencies
    pub(crate) fn resolve(&mut self) -> Result<(), AptErrors> {
        self.cache.resolve(true)
    }

    // ========================================================================
    // Package info extraction (status determined by APT state only)
    // ========================================================================

    /// Get basic status for a package (without user intent - that's in core)
    pub fn get_apt_status(&self, pkg: &Package) -> AptPackageState {
        AptPackageState {
            is_installed: pkg.is_installed(),
            is_upgradable: pkg.is_upgradable(),
            marked_install: pkg.marked_install(),
            marked_delete: pkg.marked_delete(),
            marked_upgrade: pkg.marked_upgrade(),
        }
    }

    /// Extract package info by name
    pub fn extract_package_info_by_name(&self, name: &str) -> Option<PackageInfo> {
        let pkg = self.cache.get(name)?;
        self.extract_package_info(&pkg)
    }

    /// Extract package info from an APT Package.
    /// Returns BASE status (installed/upgradable/not-installed) - ignores APT marks.
    /// The core module will compute final display status based on user_intent.
    pub fn extract_package_info(&self, pkg: &Package) -> Option<PackageInfo> {
        let candidate = pkg.candidate()?;

        // Return BASE status only - ignore APT marks
        // core.rs will overlay user_intent and dependency info for display
        let status = if pkg.is_installed() {
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

        let fullname = pkg.fullname(false);
        // Use get_id with FULL name since IDs are mapped to full names
        let id = self.get_id(&fullname).unwrap_or(PackageId(u32::MAX));

        Some(PackageInfo {
            id,
            name: fullname,
            status,
            section: candidate.section().unwrap_or("unknown").to_string(),
            installed_version,
            candidate_version: candidate.version().to_string(),
            installed_size: candidate.installed_size(),
            download_size: candidate.size(),
            description: candidate.summary().unwrap_or_default().clone(),
            architecture: candidate.arch().to_string(),
        })
    }

    // ========================================================================
    // Dependency queries
    // ========================================================================

    /// Get forward dependencies for a package
    pub fn get_dependencies(&self, name: &str) -> Vec<(String, String)> {
        let mut deps = Vec::new();

        let pkg = match self.cache.get(name) {
            Some(p) => p,
            None => return deps,
        };

        if let Some(version) = pkg.candidate()
            && let Some(dependencies) = version.dependencies() {
                for dep in dependencies {
                    let dep_type = dep.dep_type().to_string();
                    for base_dep in dep.iter() {
                        deps.push((dep_type.clone(), base_dep.name().to_string()));
                    }
                }
            }

        deps.sort_by(|a, b| {
            dep_type_order(&a.0)
                .cmp(&dep_type_order(&b.0))
                .then_with(|| a.1.cmp(&b.1))
        });

        deps
    }

    /// Get reverse dependencies for a package
    pub fn get_reverse_dependencies(&self, name: &str) -> Vec<(String, String)> {
        let mut rdeps = Vec::new();

        let pkg = match self.cache.get(name) {
            Some(p) => p,
            None => return rdeps,
        };

        let rdep_map = pkg.rdepends();
        for (dep_type, deps) in rdep_map {
            let type_str = format!("{dep_type:?}");
            for dep in deps {
                for base_dep in dep.iter() {
                    rdeps.push((type_str.clone(), base_dep.name().to_string()));
                }
            }
        }

        rdeps.sort_by(|a, b| {
            dep_type_order(&a.0)
                .cmp(&dep_type_order(&b.0))
                .then_with(|| a.1.cmp(&b.1))
        });

        rdeps
    }

    // ========================================================================
    // Statistics
    // ========================================================================

    /// Count upgradable packages
    pub fn count_upgradable(&self) -> usize {
        self.cache
            .packages(&PackageSort::default())
            .filter(rust_apt::Package::is_upgradable)
            .count()
    }

    // ========================================================================
    // Cache lifecycle
    // ========================================================================

    /// Full refresh - reload cache from disk
    pub fn refresh(&mut self) -> Result<()> {
        self.cache = Cache::new::<&str>(&[])?;
        // Note: We keep the id mappings - they're still valid names
        Ok(())
    }

    /// Commit changes using native APT progress
    pub(crate) fn commit(&mut self) -> Result<()> {
        let mut acquire_progress = AcquireProgress::apt();
        let mut install_progress = InstallProgress::apt();

        let cache = std::mem::replace(&mut self.cache, Cache::new::<&str>(&[])?);
        cache.commit(&mut acquire_progress, &mut install_progress)?;

        Ok(())
    }
}

/// Raw APT state for a package (no interpretation)
#[derive(Debug, Clone, Copy)]
pub struct AptPackageState {
    pub is_installed: bool,
    pub is_upgradable: bool,
    pub marked_install: bool,
    pub marked_delete: bool,
    pub marked_upgrade: bool,
}

/// Helper function to order dependency types by priority
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

/// Format AptErrors into a user-friendly string with specific conflict details
pub fn format_apt_errors(errors: &AptErrors) -> String {
    let mut messages = Vec::new();

    for error in errors.iter() {
        let msg = error.to_string();
        if !msg.is_empty() && msg != "E:" {
            messages.push(msg);
        }
    }

    if messages.is_empty() {
        "Dependency resolution failed (no specific details available)".to_string()
    } else if messages.len() == 1 {
        messages[0].clone()
    } else {
        format!("{}; and {} more issue(s)", messages[0], messages.len() - 1)
    }
}
