//! Debug CLI for testing the package manager state machine
//!
//! This CLI tests the exact user flow from docs/user-flow.md
//!
//! Usage:
//!   cargo run --bin debug_cli -- <command> [args]
//!
//! Commands:
//!   status              Show current state and all marked packages
//!   info <name>         Show package state
//!   toggle <name>       Toggle package mark (simulates Space key)
//!   reset               Clear all marks
//!   list [filter]       List packages (upgradable, installed, all, marked)

use std::env;
use std::fs;
use std::path::Path;

use color_eyre::Result;

use synh8::core::ManagerState;
use synh8::types::*;

const STATE_FILE: &str = "debug_state.json";

fn main() -> Result<()> {
    color_eyre::install()?;

    let args: Vec<String> = env::args().collect();
    let cmd = args.get(1).map(String::as_str).unwrap_or("help");

    match cmd {
        "status" => cmd_status()?,
        "info" => {
            let name = args.get(2).ok_or_else(|| color_eyre::eyre::eyre!("Usage: info <package_name>"))?;
            cmd_info(name)?;
        }
        "toggle" => {
            let name = args.get(2).ok_or_else(|| color_eyre::eyre::eyre!("Usage: toggle <package_name>"))?;
            cmd_toggle(name)?;
        }
        "reset" => cmd_reset()?,
        "list" => cmd_list(args.get(2).map(String::as_str))?,
        "deps" => {
            let name = args.get(2).ok_or_else(|| color_eyre::eyre::eyre!("Usage: deps <package_name>"))?;
            cmd_deps(name)?;
        }
        "help" | _ => {
            println!("Debug CLI for synh8 package manager");
            println!();
            println!("Commands:");
            println!("  status              Show current state and all marked packages");
            println!("  info <name>         Show package state");
            println!("  toggle <name>       Toggle package mark (simulates Space key)");
            println!("  reset               Clear all marks");
            println!("  list [filter]       List packages (upgradable, installed, all, marked)");
            println!();
            println!("Example flow (from docs/user-flow.md):");
            println!("  cli reset");
            println!("  cli info libinput10:amd64       # Upgradable");
            println!("  cli toggle libinput10:amd64     # Marks libinput10 + deps");
            println!("  cli info libinput10:amd64       # MarkedForUpgrade");
            println!("  cli info libinput-bin:amd64     # MarkedForUpgrade (same as user-marked)");
        }
    }

    Ok(())
}

fn cmd_status() -> Result<()> {
    let state = load_state()?;

    println!("=== Package Manager Status ===");
    println!();

    // Count marked packages
    let marked: Vec<_> = state.list().iter()
        .filter(|p| p.status.is_marked())
        .collect();

    if marked.is_empty() {
        println!("No packages marked.");
    } else {
        println!("Marked packages ({}):", marked.len());
        for pkg in &marked {
            println!("  {} {}", pkg.status.symbol(), pkg.name);
        }
    }

    println!();
    println!("Upgradable: {}", state.upgradable_count());

    Ok(())
}

fn cmd_info(name: &str) -> Result<()> {
    let state = load_state()?;

    let pkg = state.list().iter()
        .find(|p| p.name == name)
        .cloned();

    match pkg {
        Some(p) => {
            println!("{}: {}", p.name, status_display(p.status));
        }
        None => {
            println!("Package '{}' not found", name);
        }
    }

    Ok(())
}

fn cmd_toggle(name: &str) -> Result<()> {
    let mut state = load_state()?;

    // Find the package
    let pkg = state.list().iter()
        .find(|p| p.name == name)
        .cloned();

    let pkg = match pkg {
        Some(p) => p,
        None => {
            println!("Package '{}' not found", name);
            return Ok(());
        }
    };

    // Use the library's toggle function
    let result = state.toggle(pkg.id);

    // Get names for display
    let cache = state.cache();

    match &result {
        ToggleResult::Marked { package, additional } => {
            let pkg_name = cache.fullname_of(*package).unwrap_or("(unknown)");
            println!("=== Toggle {} (mark) ===", pkg_name);
            println!("Marked: {}", pkg_name);

            if !additional.is_empty() {
                println!("Also marked ({} deps):", additional.len());
                for dep_id in additional {
                    let dep_name = cache.fullname_of(*dep_id).unwrap_or("(unknown)");
                    println!("  ↑ {}", dep_name);
                }
            }
        }
        ToggleResult::Unmarked { package, also_unmarked } => {
            let pkg_name = cache.fullname_of(*package).unwrap_or("(unknown)");
            println!("=== Toggle {} (unmark) ===", pkg_name);
            println!("Unmarked: {}", pkg_name);

            if !also_unmarked.is_empty() {
                println!("Also unmarked ({}):", also_unmarked.len());
                for dep_id in also_unmarked {
                    let dep_name = cache.fullname_of(*dep_id).unwrap_or("(unknown)");
                    println!("  {}", dep_name);
                }
            }
        }
        ToggleResult::NoChange { package } => {
            let pkg_name = cache.fullname_of(*package).unwrap_or("(unknown)");
            println!("=== Toggle {} (no change) ===", pkg_name);
            println!("{} is a dependency - unmark the package that requires it", pkg_name);
        }
    }

    // Save state
    save_state(&state)?;

    Ok(())
}

fn cmd_reset() -> Result<()> {
    // Delete state file
    if Path::new(STATE_FILE).exists() {
        fs::remove_file(STATE_FILE)?;
    }
    println!("All marks cleared.");
    Ok(())
}

fn cmd_list(filter: Option<&str>) -> Result<()> {
    let mut state = load_state()?;

    let filter_cat = match filter {
        Some("upgradable") | None => FilterCategory::Upgradable,
        Some("installed") => FilterCategory::Installed,
        Some("marked") => FilterCategory::MarkedChanges,
        Some("all") => FilterCategory::All,
        Some(f) => {
            println!("Unknown filter: {}. Using 'upgradable'", f);
            FilterCategory::Upgradable
        }
    };

    state.apply_filter(filter_cat);
    state.rebuild_list();

    let list = state.list();
    println!("Packages ({}) - filter: {:?}:", list.len(), filter_cat);
    println!();

    for pkg in list.iter().take(30) {
        println!("  {} {}", pkg.status.symbol(), pkg.name);
    }

    if list.len() > 30 {
        println!("  ... and {} more", list.len() - 30);
    }

    Ok(())
}

fn cmd_deps(name: &str) -> Result<()> {
    let state = load_state()?;
    let deps = state.get_dependencies(name);

    println!("Dependencies for {}:", name);
    for (dep_type, dep_name) in deps {
        println!("  {} {}", dep_type, dep_name);
    }

    Ok(())
}

/// Display status name (simplified - no Auto* distinction)
fn status_display(status: PackageStatus) -> &'static str {
    match status {
        PackageStatus::Installed => "Installed",
        PackageStatus::NotInstalled => "NotInstalled",
        PackageStatus::Upgradable => "Upgradable",
        PackageStatus::MarkedForInstall => "MarkedForInstall",
        PackageStatus::MarkedForUpgrade => "MarkedForUpgrade",
        PackageStatus::MarkedForRemove => "MarkedForRemove",
        PackageStatus::Keep => "Keep",
        PackageStatus::Broken => "Broken",
    }
}

// === State persistence ===

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct SavedState {
    /// Packages the user explicitly marked (user_intent)
    user_marked: Vec<String>,
}

fn load_state() -> Result<ManagerState> {
    let mut state = ManagerState::new()?;
    state.apply_filter(FilterCategory::All);
    state.rebuild_list();

    if Path::new(STATE_FILE).exists() {
        let content = fs::read_to_string(STATE_FILE)?;
        let saved: SavedState = serde_json::from_str(&content)?;

        // Re-mark the saved packages
        for name in &saved.user_marked {
            if let Some(pkg) = state.list().iter().find(|p| p.name == *name).cloned() {
                state.mark_install(pkg.id);
            }
        }

        // Compute plan to resolve dependencies
        if state.has_marks() {
            state.compute_plan();
        }
        state.rebuild_list();
    }

    Ok(state)
}

fn save_state(state: &ManagerState) -> Result<()> {
    // Save only user_intent packages (not dependencies)
    let user_marked: Vec<String> = state.list()
        .iter()
        .filter(|p| state.is_user_marked(p.id))
        .map(|p| p.name.clone())
        .collect();

    let saved = SavedState { user_marked };
    let content = serde_json::to_string_pretty(&saved)?;
    fs::write(STATE_FILE, content)?;
    Ok(())
}
