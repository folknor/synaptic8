//! APT cache operations and package management

use std::collections::HashMap;

use color_eyre::Result;
use rust_apt::cache::{Cache, PackageSort};
use rust_apt::error::AptErrors;
use rust_apt::progress::{AcquireProgress, InstallProgress};
use rust_apt::{Package, Version};

use crate::types::*;

/// Manages APT cache interactions and user-marked packages
pub struct AptManager {
    cache: Cache,
    user_marked: HashMap<String, bool>,
}

impl AptManager {
    /// Create a new AptManager with a fresh APT cache
    pub fn new() -> Result<Self> {
        let cache = Cache::new::<&str>(&[])?;
        Ok(Self {
            cache,
            user_marked: HashMap::new(),
        })
    }

    /// Get a package by name
    pub fn get(&self, name: &str) -> Option<Package<'_>> {
        self.cache.get(name)
    }

    /// Get an iterator over packages with the given sort
    pub fn packages(&self, sort: &PackageSort) -> impl Iterator<Item = Package<'_>> {
        self.cache.packages(sort)
    }

    /// Get packages with pending changes
    pub fn get_changes(&self) -> impl Iterator<Item = Package<'_>> {
        self.cache.get_changes(false)
    }

    /// Resolve dependencies
    pub fn resolve(&mut self) -> Result<(), AptErrors> {
        self.cache.resolve(true)
    }

    // === Marking operations ===

    /// Mark a package for install/upgrade and record as user-marked
    pub fn mark_install(&mut self, name: &str) {
        if let Some(pkg) = self.cache.get(name) {
            pkg.mark_install(true, true);
            pkg.protect();
            self.user_marked.insert(name.to_string(), true);
        }
    }

    /// Mark a package to keep current version
    pub fn mark_keep(&mut self, name: &str) {
        if let Some(pkg) = self.cache.get(name) {
            pkg.mark_keep();
            self.user_marked.remove(name);
        }
    }

    /// Clear all user marks
    pub fn clear_user_marks(&mut self) {
        self.user_marked.clear();
    }

    // === Package info extraction ===

    /// Get status for a package by name
    pub fn get_package_status(&self, name: &str) -> PackageStatus {
        match self.cache.get(name) {
            Some(pkg) => {
                if pkg.marked_upgrade() {
                    PackageStatus::MarkedForUpgrade
                } else if pkg.marked_install() {
                    if pkg.is_installed() {
                        PackageStatus::MarkedForUpgrade
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

    /// Extract full package info from an APT Package
    pub fn extract_package_info(&self, pkg: &Package) -> Option<PackageInfo> {
        let candidate = pkg.candidate()?;

        let status = if pkg.marked_upgrade() {
            PackageStatus::MarkedForUpgrade
        } else if pkg.marked_install() {
            if pkg.is_installed() {
                PackageStatus::MarkedForUpgrade
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

        let name = pkg.name().to_string();
        Some(PackageInfo {
            display_name: name.clone(), // Will be updated in rebuild_list if needed
            name,
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

    // === Dependency queries ===

    /// Get forward dependencies for a package
    pub fn get_dependencies(&self, name: &str) -> Vec<(String, String)> {
        let mut deps = Vec::new();

        let pkg = match self.cache.get(name) {
            Some(p) => p,
            None => return deps,
        };

        if let Some(version) = pkg.candidate() {
            if let Some(dependencies) = version.dependencies() {
                for dep in dependencies {
                    let dep_type = dep.dep_type().to_string();
                    for base_dep in dep.iter() {
                        deps.push((dep_type.clone(), base_dep.name().to_string()));
                    }
                }
            }
        }

        // Sort by type priority, then by name
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
        for (dep_type, deps) in rdep_map.iter() {
            let type_str = format!("{:?}", dep_type);
            for dep in deps {
                for base_dep in dep.iter() {
                    rdeps.push((type_str.clone(), base_dep.name().to_string()));
                }
            }
        }

        // Sort by type priority, then by name
        rdeps.sort_by(|a, b| {
            dep_type_order(&a.0)
                .cmp(&dep_type_order(&b.0))
                .then_with(|| a.1.cmp(&b.1))
        });

        rdeps
    }

    // === Change calculation ===

    /// Calculate pending changes from the cache state
    pub fn calculate_pending(&self) -> PendingChanges {
        let mut pending = PendingChanges::default();

        for pkg in self.cache.get_changes(false) {
            let name = pkg.name().to_string();
            let is_user = self.user_marked.get(&name).copied().unwrap_or(false);

            if pkg.marked_install() {
                if pkg.is_installed() {
                    if is_user {
                        pending.to_upgrade.push(name);
                    } else {
                        pending.auto_upgrade.push(name);
                    }
                } else if is_user {
                    pending.to_install.push(name);
                } else {
                    pending.auto_install.push(name);
                }

                if let Some(cand) = pkg.candidate() {
                    pending.download_size += cand.size();
                    pending.install_size_change += cand.installed_size() as i64;
                }
            } else if pkg.marked_delete() {
                if is_user {
                    pending.to_remove.push(name);
                } else {
                    pending.auto_remove.push(name);
                }

                if let Some(inst) = pkg.installed() {
                    pending.install_size_change -= inst.installed_size() as i64;
                }
            }
        }

        pending
    }

    /// Count upgradable packages
    pub fn count_upgradable(&self) -> usize {
        self.cache
            .packages(&PackageSort::default())
            .filter(|p| p.is_upgradable())
            .count()
    }

    // === Cache refresh and restore ===

    /// Restore cache to user-marked state without reloading
    /// This is faster than refresh() for undoing temporary marks (e.g., preview)
    pub fn restore_to_user_marks(&mut self) {
        // Get all currently changed packages
        let changed: Vec<String> = self.cache
            .get_changes(false)
            .map(|p| p.name().to_string())
            .collect();

        // Clear all marks
        for name in &changed {
            if let Some(pkg) = self.cache.get(name) {
                pkg.mark_keep();
            }
        }

        // Re-apply user marks
        for name in self.user_marked.keys() {
            if let Some(pkg) = self.cache.get(name) {
                pkg.mark_install(true, true);
                pkg.protect();
            }
        }

        // Resolve dependencies
        if !self.user_marked.is_empty() {
            let _ = self.cache.resolve(true);
        }
    }

    /// Full refresh clearing all marks
    pub fn full_refresh(&mut self) -> Result<()> {
        self.cache = Cache::new::<&str>(&[])?;
        self.user_marked.clear();
        Ok(())
    }

    // === System operations ===

    /// Commit changes using native APT progress
    /// Note: This replaces the internal cache
    pub fn commit(&mut self) -> Result<()> {
        let mut acquire_progress = AcquireProgress::apt();
        let mut install_progress = InstallProgress::apt();

        // Take ownership of cache for commit
        let cache = std::mem::replace(&mut self.cache, Cache::new::<&str>(&[])?);

        cache.commit(&mut acquire_progress, &mut install_progress)?;

        // Clear state after successful commit
        self.user_marked.clear();

        Ok(())
    }
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
        // Skip empty or generic messages
        if !msg.is_empty() && msg != "E:" {
            messages.push(msg);
        }
    }

    if messages.is_empty() {
        "Dependency resolution failed (no specific details available)".to_string()
    } else if messages.len() == 1 {
        messages[0].clone()
    } else {
        // Join multiple errors
        format!("{}; and {} more issue(s)", messages[0], messages.len() - 1)
    }
}
