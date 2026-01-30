use std::collections::{HashMap, HashSet};
use std::io;
use std::time::Instant;

use color_eyre::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::prelude::*;
use ratatui::widgets::{
    Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Scrollbar,
    ScrollbarOrientation, ScrollbarState, Table, TableState, Wrap,
};
use rust_apt::cache::{Cache, PackageSort};
use rust_apt::progress::{AcquireProgress, InstallProgress};
use rust_apt::{Package, Version};
use rusqlite::{Connection, params};

/// Package status matching Synaptic's status icons
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PackageStatus {
    Upgradable,   // ↑ Package can be upgraded (yellow)
    Upgrade,      // ↑ Package marked for upgrade (green)
    Install,      // + Package marked for install
    Remove,       // - Package marked for removal
    Keep,         // = Package kept at current version
    Installed,    // · Package is installed (no changes)
    NotInstalled, //   Package is not installed
    Broken,       // ✗ Package is broken
    AutoInstall,  // + Automatically installed (dependency)
    AutoRemove,   // - Automatically removed
}

impl PackageStatus {
    fn symbol(&self) -> &'static str {
        match self {
            Self::Upgradable | Self::Upgrade => "↑",
            Self::Install | Self::AutoInstall => "+",
            Self::Remove | Self::AutoRemove => "-",
            Self::Keep => "=",
            Self::Installed => "·",
            Self::NotInstalled => "",
            Self::Broken => "✗",
        }
    }

    fn color(&self) -> Color {
        match self {
            Self::Upgradable => Color::Yellow,
            Self::Upgrade => Color::Green,
            Self::Install | Self::AutoInstall => Color::Green,
            Self::Remove | Self::AutoRemove => Color::Red,
            Self::Keep => Color::Blue,
            Self::Installed => Color::DarkGray,
            Self::NotInstalled => Color::Gray,
            Self::Broken => Color::LightRed,
        }
    }
}

/// Filter categories (left panel)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FilterCategory {
    Upgradable,
    MarkedChanges,
    Installed,
    NotInstalled,
    All,
}

impl FilterCategory {
    fn label(&self) -> &'static str {
        match self {
            Self::Upgradable => "Upgradable",
            Self::MarkedChanges => "Marked Changes",
            Self::Installed => "Installed",
            Self::NotInstalled => "Not Installed",
            Self::All => "All Packages",
        }
    }

    fn all() -> &'static [FilterCategory] {
        &[
            Self::Upgradable,
            Self::MarkedChanges,
            Self::Installed,
            Self::NotInstalled,
            Self::All,
        ]
    }
}

/// Displayed package info (extracted from rust-apt Package)
#[derive(Debug, Clone)]
struct PackageInfo {
    name: String,
    status: PackageStatus,
    section: String,
    installed_version: String,
    candidate_version: String,
    installed_size: u64,
    download_size: u64,
    description: String,
    architecture: String,
    is_user_marked: bool,
}

impl PackageInfo {
    fn size_str(bytes: u64) -> String {
        if bytes == 0 {
            return String::from("-");
        }
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        const GB: u64 = MB * 1024;

        if bytes >= GB {
            format!("{:.1} GB", bytes as f64 / GB as f64)
        } else if bytes >= MB {
            format!("{:.1} MB", bytes as f64 / MB as f64)
        } else if bytes >= KB {
            format!("{:.1} KB", bytes as f64 / KB as f64)
        } else {
            format!("{} B", bytes)
        }
    }

    fn installed_size_str(&self) -> String {
        Self::size_str(self.installed_size)
    }

    fn download_size_str(&self) -> String {
        Self::size_str(self.download_size)
    }
}

/// Column configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Column {
    Status,
    Name,
    Section,
    InstalledVersion,
    CandidateVersion,
    DownloadSize,
}

impl Column {
    fn header(&self) -> &'static str {
        match self {
            Self::Status => "S",
            Self::Name => "Package",
            Self::Section => "Section",
            Self::InstalledVersion => "Installed",
            Self::CandidateVersion => "Candidate",
            Self::DownloadSize => "Download",
        }
    }

    fn width(&self, app: &App) -> Constraint {
        match self {
            Self::Status => Constraint::Length(3),
            Self::Name => Constraint::Min(app.col_width_name),
            Self::Section => Constraint::Length(app.col_width_section),
            Self::InstalledVersion => Constraint::Length(app.col_width_installed),
            Self::CandidateVersion => Constraint::Length(app.col_width_candidate),
            Self::DownloadSize => Constraint::Length(10),
        }
    }

    fn visible_columns(settings: &Settings) -> Vec<Column> {
        let mut cols = Vec::new();
        if settings.show_status_column {
            cols.push(Self::Status);
        }
        if settings.show_name_column {
            cols.push(Self::Name);
        }
        if settings.show_section_column {
            cols.push(Self::Section);
        }
        if settings.show_installed_version_column {
            cols.push(Self::InstalledVersion);
        }
        if settings.show_candidate_version_column {
            cols.push(Self::CandidateVersion);
        }
        if settings.show_download_size_column {
            cols.push(Self::DownloadSize);
        }
        cols
    }
}

/// Which pane has focus
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusedPane {
    Filters,
    Packages,
    Details,
}

/// Which tab is shown in details pane
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailsTab {
    Info,
    Dependencies,
    ReverseDeps,
}

#[derive(Debug, PartialEq, Eq)]
enum AppState {
    Listing,
    Searching,          // User is typing a search query
    ShowingMarkConfirm, // Popup showing additional changes when marking a package
    ShowingChanges,     // Final confirmation before applying all changes
    ShowingChangelog,   // Viewing package changelog
    ShowingSettings,    // Settings/preferences view
    Upgrading,
    Done,
}

/// Sort options
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SortBy {
    Name,
    Section,
    InstalledVersion,
    CandidateVersion,
}

impl SortBy {
    fn label(&self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Section => "Section",
            Self::InstalledVersion => "Installed version",
            Self::CandidateVersion => "Candidate version",
        }
    }

    fn all() -> &'static [SortBy] {
        &[Self::Name, Self::Section, Self::InstalledVersion, Self::CandidateVersion]
    }
}

/// User settings (not persisted yet)
#[derive(Debug, Clone)]
struct Settings {
    show_status_column: bool,
    show_name_column: bool,
    show_section_column: bool,
    show_installed_version_column: bool,
    show_candidate_version_column: bool,
    show_download_size_column: bool,
    sort_by: SortBy,
    sort_ascending: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            show_status_column: true,
            show_name_column: true,
            show_section_column: false,
            show_installed_version_column: false,
            show_candidate_version_column: true,
            show_download_size_column: false,
            sort_by: SortBy::CandidateVersion,
            sort_ascending: true,
        }
    }
}

/// Result of attempting to apply changes
enum ApplyResult {
    NotRoot,
    NeedsCommit,
}

/// Additional changes required when marking a single package
#[derive(Debug, Default, Clone)]
struct MarkPreview {
    package_name: String,
    is_marking: bool, // true = marking for install, false = unmarking
    additional_installs: Vec<String>,
    additional_removes: Vec<String>,
    download_size: u64,
}

/// Changes to be applied
#[derive(Debug, Default)]
struct PendingChanges {
    to_install: Vec<String>,
    to_upgrade: Vec<String>,
    to_remove: Vec<String>,
    auto_install: Vec<String>,
    auto_remove: Vec<String>,
    download_size: u64,
    install_size_change: i64,
}

/// SQLite FTS5 search index
struct SearchIndex {
    conn: Connection,
}

impl SearchIndex {
    fn new() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute(
            "CREATE VIRTUAL TABLE IF NOT EXISTS packages USING fts5(name, description)",
            [],
        )?;
        Ok(Self { conn })
    }

    fn build(&mut self, cache: &Cache) -> Result<(usize, std::time::Duration)> {
        let start = Instant::now();
        let mut count = 0;

        // Clear existing data
        self.conn.execute("DELETE FROM packages", [])?;

        // Insert all packages
        let mut stmt = self.conn.prepare("INSERT INTO packages (name, description) VALUES (?, ?)")?;

        for pkg in cache.packages(&PackageSort::default()) {
            let name = pkg.name();
            let desc = pkg.candidate()
                .and_then(|c| c.summary())
                .unwrap_or_default();
            stmt.execute(params![name, desc])?;
            count += 1;
        }

        Ok((count, start.elapsed()))
    }

    fn search(&self, query: &str) -> Result<HashSet<String>> {
        let mut results = HashSet::new();

        if query.is_empty() {
            return Ok(results);
        }

        // Escape special FTS5 characters and add prefix matching
        let fts_query = query
            .split_whitespace()
            .map(|word| format!("{}*", word.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" ");

        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT name FROM packages WHERE packages MATCH ?"
        )?;

        let rows = stmt.query_map([&fts_query], |row| row.get::<_, String>(0))?;

        for name in rows.flatten() {
            results.insert(name);
        }

        Ok(results)
    }
}

struct App {
    cache: Cache,
    packages: Vec<PackageInfo>,
    user_marked: HashMap<String, bool>,
    table_state: TableState,
    filter_state: ListState,
    state: AppState,
    status_message: String,
    output_lines: Vec<String>,
    focused_pane: FocusedPane,
    selected_filter: FilterCategory,
    detail_scroll: u16,
    details_tab: DetailsTab,
    pending_changes: PendingChanges,
    changes_scroll: u16,
    mark_preview: MarkPreview, // Preview of additional changes when marking
    // Search
    search_index: Option<SearchIndex>,
    search_query: String,
    search_results: Option<HashSet<String>>, // None = no active search filter
    search_build_time: Option<std::time::Duration>,
    // Settings
    settings: Settings,
    settings_selection: usize,
    // Changelog
    changelog_content: Vec<String>,
    changelog_scroll: u16,
    // Cached dependencies for current selection (avoid recalc every frame)
    cached_deps: Vec<(String, String)>,
    cached_rdeps: Vec<(String, String)>,
    cached_pkg_name: String,
    // Column widths (calculated from content)
    col_width_name: u16,
    col_width_section: u16,
    col_width_installed: u16,
    col_width_candidate: u16,
}

impl App {
    fn new() -> Result<Self> {
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
            col_width_name: 10,
            col_width_section: 7,
            col_width_installed: 9,
            col_width_candidate: 9,
        };

        app.reload_packages()?;
        Ok(app)
    }

    fn update_cached_deps(&mut self) {
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
    fn get_package_status(&self, name: &str) -> PackageStatus {
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

    fn ensure_search_index(&mut self) -> Result<()> {
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

    fn start_search(&mut self) {
        if let Err(e) = self.ensure_search_index() {
            self.status_message = format!("Failed to build search index: {}", e);
            return;
        }
        self.search_query.clear();
        self.state = AppState::Searching;
    }

    fn execute_search(&mut self) {
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

    fn cancel_search(&mut self) {
        self.search_query.clear();
        self.search_results = None;
        self.state = AppState::Listing;
        self.apply_current_filter();
        self.update_status_message();
    }

    fn confirm_search(&mut self) {
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

    fn reload_packages(&mut self) -> Result<()> {
        self.packages.clear();
        self.apply_current_filter();

        if !self.packages.is_empty() {
            self.table_state.select(Some(0));
        }

        self.update_status_message();
        Ok(())
    }

    fn apply_current_filter(&mut self) {
        self.packages.clear();

        // Reset column widths to header minimums
        self.col_width_name = 7;      // "Package"
        self.col_width_section = 7;   // "Section"
        self.col_width_installed = 9; // "Installed"
        self.col_width_candidate = 9; // "Candidate"

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
                    self.col_width_name = self.col_width_name.max(info.name.len() as u16);
                    self.col_width_section = self.col_width_section.max(info.section.len() as u16);
                    self.col_width_installed = self.col_width_installed.max(info.installed_version.len() as u16);
                    self.col_width_candidate = self.col_width_candidate.max(info.candidate_version.len() as u16);
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

    fn selected_package(&self) -> Option<&PackageInfo> {
        self.table_state
            .selected()
            .and_then(|i| self.packages.get(i))
    }

    fn toggle_details_tab(&mut self) {
        self.next_details_tab();
    }

    fn next_details_tab(&mut self) {
        self.details_tab = match self.details_tab {
            DetailsTab::Info => DetailsTab::Dependencies,
            DetailsTab::Dependencies => DetailsTab::ReverseDeps,
            DetailsTab::ReverseDeps => DetailsTab::Info,
        };
        self.detail_scroll = 0;
    }

    fn prev_details_tab(&mut self) {
        self.details_tab = match self.details_tab {
            DetailsTab::Info => DetailsTab::ReverseDeps,
            DetailsTab::Dependencies => DetailsTab::Info,
            DetailsTab::ReverseDeps => DetailsTab::Dependencies,
        };
        self.detail_scroll = 0;
    }

    fn show_changelog(&mut self) {
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

    fn show_settings(&mut self) {
        self.settings_selection = 0;
        self.state = AppState::ShowingSettings;
    }

    fn toggle_setting(&mut self) {
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

    fn settings_item_count() -> usize {
        8 // 6 column toggles + 2 sort options
    }

    fn toggle_current(&mut self) {
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
                    additional_installs.push(pkg_name);
                } else if pkg.marked_delete() {
                    additional_removes.push(pkg_name);
                }
            }
        }

        // UNDO - restore cache to previous state before showing preview
        self.restore_marks();

        // If there are additional changes, show the confirmation popup
        if !additional_installs.is_empty() || !additional_removes.is_empty() {
            self.mark_preview = MarkPreview {
                package_name: name.to_string(),
                is_marking: true,
                additional_installs,
                additional_removes,
                download_size,
            };
            self.state = AppState::ShowingMarkConfirm;
        } else {
            // No additional changes, just apply directly
            self.mark_preview = MarkPreview {
                package_name: name.to_string(),
                is_marking: true,
                additional_installs: Vec::new(),
                additional_removes: Vec::new(),
                download_size,
            };
            self.confirm_mark();
        }
    }

    /// Restore cache marks to match user_marked state
    fn restore_marks(&mut self) {
        // Clear all marks
        for pkg in self.cache.packages(&PackageSort::default()) {
            if pkg.marked_install() || pkg.marked_delete() || pkg.marked_upgrade() {
                pkg.mark_keep();
            }
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
            let _ = self.cache.resolve(true);
        }
    }

    /// User confirmed the mark, now actually apply it
    fn confirm_mark(&mut self) {
        let name = self.mark_preview.package_name.clone();

        // Now actually apply the mark
        if let Some(pkg) = self.cache.get(&name) {
            pkg.mark_install(true, true);
            pkg.protect();
        }
        let _ = self.cache.resolve(true);

        // Record in user_marked
        self.user_marked.insert(name, true);

        self.mark_preview = MarkPreview::default();
        self.calculate_pending_changes();
        self.apply_current_filter();
        self.update_status_message();
        self.state = AppState::Listing;
    }

    /// User cancelled the mark - nothing to undo since we didn't apply yet
    fn cancel_mark(&mut self) {
        self.mark_preview = MarkPreview::default();
        self.state = AppState::Listing;
    }

    fn mark_all_upgrades(&mut self) {
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
    }

    fn unmark_all(&mut self) {
        for pkg in self.cache.packages(&PackageSort::default()) {
            pkg.mark_keep();
        }
        self.user_marked.clear();

        self.calculate_pending_changes();
        self.apply_current_filter();
        self.update_status_message();
    }

    fn calculate_pending_changes(&mut self) {
        self.pending_changes = PendingChanges::default();

        for pkg in self.cache.get_changes(false) {
            let name = pkg.name().to_string();
            let is_user = self.user_marked.get(&name).copied().unwrap_or(false);

            if pkg.marked_install() {
                if pkg.is_installed() {
                    if is_user {
                        self.pending_changes.to_upgrade.push(name);
                    } else {
                        self.pending_changes.auto_install.push(name);
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

    fn show_changes_preview(&mut self) {
        self.calculate_pending_changes();
        if self.has_pending_changes() {
            self.state = AppState::ShowingChanges;
            self.changes_scroll = 0;
        } else {
            self.status_message = "No changes to apply".to_string();
        }
    }

    fn has_pending_changes(&self) -> bool {
        !self.pending_changes.to_install.is_empty()
            || !self.pending_changes.to_upgrade.is_empty()
            || !self.pending_changes.to_remove.is_empty()
            || !self.pending_changes.auto_install.is_empty()
            || !self.pending_changes.auto_remove.is_empty()
    }

    fn total_changes_count(&self) -> usize {
        self.pending_changes.to_install.len()
            + self.pending_changes.to_upgrade.len()
            + self.pending_changes.to_remove.len()
            + self.pending_changes.auto_install.len()
            + self.pending_changes.auto_remove.len()
    }

    fn update_status_message(&mut self) {
        let upgradable_count = self
            .cache
            .packages(&PackageSort::default())
            .filter(|p| p.is_upgradable())
            .count();

        let changes = self.total_changes_count();

        if changes > 0 {
            self.status_message = format!(
                "{} changes pending ({} download) | {} upgradable | Press 'u' to review",
                changes,
                PackageInfo::size_str(self.pending_changes.download_size),
                upgradable_count
            );
        } else {
            self.status_message = format!("{} packages upgradable", upgradable_count);
        }
    }

    fn move_package_selection(&mut self, delta: i32) {
        if self.packages.is_empty() {
            return;
        }
        let current = self.table_state.selected().unwrap_or(0) as i32;
        let new_idx = (current + delta).clamp(0, self.packages.len() as i32 - 1) as usize;
        self.table_state.select(Some(new_idx));
        self.detail_scroll = 0;
    }

    fn move_filter_selection(&mut self, delta: i32) {
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

    fn cycle_focus(&mut self) {
        self.focused_pane = match self.focused_pane {
            FocusedPane::Filters => FocusedPane::Packages,
            FocusedPane::Packages => FocusedPane::Details,
            FocusedPane::Details => FocusedPane::Filters,
        };
    }

    fn cycle_focus_back(&mut self) {
        self.focused_pane = match self.focused_pane {
            FocusedPane::Filters => FocusedPane::Details,
            FocusedPane::Packages => FocusedPane::Filters,
            FocusedPane::Details => FocusedPane::Packages,
        };
    }

    fn is_root() -> bool {
        unsafe { libc::geteuid() == 0 }
    }

    fn apply_changes(&mut self) -> ApplyResult {
        if !Self::is_root() {
            self.state = AppState::Listing;
            self.status_message = "Root privileges required. Run with sudo.".to_string();
            return ApplyResult::NotRoot;
        }

        self.state = AppState::Upgrading;
        ApplyResult::NeedsCommit
    }

    fn commit_changes(&mut self) -> Result<()> {
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

    fn refresh_cache(&mut self) -> Result<()> {
        self.cache = Cache::new::<&str>(&[])?;
        self.user_marked.clear();
        self.pending_changes = PendingChanges::default();
        self.search_index = None; // Force rebuild on next search
        self.search_query.clear();
        self.search_results = None;
        self.reload_packages()?;
        Ok(())
    }
}

fn main() -> Result<()> {
    color_eyre::install()?;

    enable_raw_mode()?;
    io::stdout().execute(EnterAlternateScreen)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;

    let mut app = App::new()?;

    loop {
        terminal.draw(|f| ui(f, &mut app))?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                match app.state {
                    AppState::Listing => match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => {
                            app.cycle_focus_back()
                        }
                        KeyCode::Tab => app.cycle_focus(),
                        KeyCode::BackTab => app.cycle_focus_back(),
                        KeyCode::Char('/') => app.start_search(),
                        KeyCode::Esc => {
                            // Clear search filter
                            if app.search_results.is_some() {
                                app.search_query.clear();
                                app.search_results = None;
                                app.apply_current_filter();
                                app.update_status_message();
                            }
                        }
                        KeyCode::Up | KeyCode::Char('k') => match app.focused_pane {
                            FocusedPane::Filters => app.move_filter_selection(-1),
                            FocusedPane::Packages => app.move_package_selection(-1),
                            FocusedPane::Details => {
                                app.detail_scroll = app.detail_scroll.saturating_sub(1)
                            }
                        },
                        KeyCode::Down | KeyCode::Char('j') => match app.focused_pane {
                            FocusedPane::Filters => app.move_filter_selection(1),
                            FocusedPane::Packages => app.move_package_selection(1),
                            FocusedPane::Details => {
                                app.detail_scroll = app.detail_scroll.saturating_add(1)
                            }
                        },
                        KeyCode::PageDown => app.move_package_selection(10),
                        KeyCode::PageUp => app.move_package_selection(-10),
                        KeyCode::Char(' ') => {
                            if app.focused_pane == FocusedPane::Packages {
                                app.toggle_current();
                            }
                        }
                        KeyCode::Left | KeyCode::Char('h') => {
                            if app.focused_pane == FocusedPane::Details {
                                app.prev_details_tab();
                            }
                        }
                        KeyCode::Right | KeyCode::Char('l') => {
                            if app.focused_pane == FocusedPane::Details {
                                app.next_details_tab();
                            }
                        }
                        KeyCode::Char('a') => app.mark_all_upgrades(),
                        KeyCode::Char('n') => app.unmark_all(),
                        KeyCode::Char('d') => app.toggle_details_tab(),
                        KeyCode::Char('c') => app.show_changelog(),
                        KeyCode::Char('s') => app.show_settings(),
                        KeyCode::Char('u') | KeyCode::Enter => {
                            if app.focused_pane == FocusedPane::Packages {
                                app.show_changes_preview();
                            }
                        }
                        KeyCode::Char('r') => {
                            let _ = app.refresh_cache();
                        }
                        _ => {}
                    },
                    AppState::Searching => match key.code {
                        KeyCode::Esc => app.cancel_search(),
                        KeyCode::Enter => app.confirm_search(),
                        KeyCode::Backspace => {
                            app.search_query.pop();
                            app.execute_search();
                        }
                        KeyCode::Up => app.move_package_selection(-1),
                        KeyCode::Down => app.move_package_selection(1),
                        KeyCode::Char(' ') => app.toggle_current(),
                        KeyCode::Char(c) => {
                            app.search_query.push(c);
                            app.execute_search();
                        }
                        _ => {}
                    },
                    AppState::ShowingMarkConfirm => match key.code {
                        KeyCode::Char('y') | KeyCode::Char(' ') | KeyCode::Enter => {
                            app.confirm_mark();
                        }
                        KeyCode::Char('n') | KeyCode::Esc | KeyCode::Char('q') => {
                            app.cancel_mark();
                        }
                        _ => {}
                    },
                    AppState::ShowingChanges => match key.code {
                        KeyCode::Char('y') | KeyCode::Enter => {
                            match app.apply_changes() {
                                ApplyResult::NotRoot => {
                                    // Status message already set, just continue
                                }
                                ApplyResult::NeedsCommit => {
                                    // Exit TUI temporarily for apt output
                                    disable_raw_mode()?;
                                    io::stdout().execute(LeaveAlternateScreen)?;

                                    // Run the actual commit
                                    let _ = app.commit_changes();

                                    // Wait for user to acknowledge
                                    println!("\nPress Enter to continue...");
                                    let mut input = String::new();
                                    let _ = std::io::stdin().read_line(&mut input);

                                    // Re-enter TUI
                                    enable_raw_mode()?;
                                    io::stdout().execute(EnterAlternateScreen)?;

                                    // Refresh the cache
                                    let _ = app.refresh_cache();
                                }
                            }
                        }
                        KeyCode::Char('n') | KeyCode::Esc | KeyCode::Char('q') => {
                            app.state = AppState::Listing;
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            app.changes_scroll = app.changes_scroll.saturating_sub(1);
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            app.changes_scroll = app.changes_scroll.saturating_add(1);
                        }
                        _ => {}
                    },
                    AppState::ShowingChangelog => match key.code {
                        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('c') => {
                            app.state = AppState::Listing;
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            app.changelog_scroll = app.changelog_scroll.saturating_sub(1);
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            app.changelog_scroll = app.changelog_scroll.saturating_add(1);
                        }
                        KeyCode::PageUp => {
                            app.changelog_scroll = app.changelog_scroll.saturating_sub(20);
                        }
                        KeyCode::PageDown => {
                            app.changelog_scroll = app.changelog_scroll.saturating_add(20);
                        }
                        _ => {}
                    },
                    AppState::ShowingSettings => match key.code {
                        KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('s') => {
                            app.state = AppState::Listing;
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            if app.settings_selection > 0 {
                                app.settings_selection -= 1;
                            }
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            if app.settings_selection < App::settings_item_count() - 1 {
                                app.settings_selection += 1;
                            }
                        }
                        KeyCode::Enter | KeyCode::Char(' ') => {
                            app.toggle_setting();
                        }
                        _ => {}
                    },
                    AppState::Upgrading => {}
                    AppState::Done => match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char('r') => {
                            app.state = AppState::Listing;
                            let _ = app.refresh_cache();
                        }
                        _ => {}
                    },
                }
            }
        }
    }

    disable_raw_mode()?;
    io::stdout().execute(LeaveAlternateScreen)?;

    Ok(())
}

fn ui(frame: &mut Frame, app: &mut App) {
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(10),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

    let changes_count = app.total_changes_count();
    let title_text = if changes_count > 0 {
        format!(
            " APT TUI │ {} changes │ {} download ",
            changes_count,
            PackageInfo::size_str(app.pending_changes.download_size)
        )
    } else {
        " APT TUI │ No changes pending ".to_string()
    };
    let title = Paragraph::new(title_text)
        .style(Style::default().fg(Color::White).bg(Color::Blue).bold());
    frame.render_widget(title, main_chunks[0]);

    match app.state {
        AppState::Listing | AppState::Searching => {
            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(19),
                    Constraint::Min(40),
                    Constraint::Length(35),
                ])
                .split(main_chunks[1]);

            render_filter_pane(frame, app, panes[0]);
            render_package_table(frame, app, panes[1]);
            render_details_pane(frame, app, panes[2]);
        }
        AppState::ShowingMarkConfirm => {
            // Render the package list in background
            let panes = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(19),
                    Constraint::Min(40),
                    Constraint::Length(35),
                ])
                .split(main_chunks[1]);

            render_filter_pane(frame, app, panes[0]);
            render_package_table(frame, app, panes[1]);
            render_details_pane(frame, app, panes[2]);

            // Overlay the confirmation modal
            render_mark_confirm_modal(frame, app, main_chunks[1]);
        }
        AppState::ShowingChanges => {
            render_changes_modal(frame, app, main_chunks[1]);
        }
        AppState::ShowingChangelog => {
            render_changelog_view(frame, app, main_chunks[1]);
        }
        AppState::ShowingSettings => {
            render_settings_view(frame, app, main_chunks[1]);
        }
        AppState::Upgrading | AppState::Done => {
            let output_text = app.output_lines.join("\n");
            let output = Paragraph::new(output_text)
                .block(Block::default().title(" Output ").borders(Borders::ALL))
                .wrap(Wrap { trim: false });
            frame.render_widget(output, main_chunks[1]);
        }
    }

    let status_style = match app.state {
        AppState::Listing => Style::default().fg(Color::Yellow),
        AppState::Searching => Style::default().fg(Color::White),
        AppState::ShowingMarkConfirm => Style::default().fg(Color::Magenta),
        AppState::ShowingChanges => Style::default().fg(Color::Cyan),
        AppState::ShowingChangelog => Style::default().fg(Color::Cyan),
        AppState::ShowingSettings => Style::default().fg(Color::Yellow),
        AppState::Upgrading => Style::default().fg(Color::Cyan),
        AppState::Done => Style::default().fg(Color::Green),
    };
    let status_text = match app.state {
        AppState::Searching => format!("/{}_", app.search_query),
        AppState::ShowingMarkConfirm => format!(
            "Mark '{}' requires additional changes",
            app.mark_preview.package_name
        ),
        _ => {
            // Show active search filter in status if present
            if app.search_results.is_some() {
                format!("[Search: {}] {}", app.search_query, app.status_message)
            } else {
                app.status_message.clone()
            }
        }
    };
    let status = Paragraph::new(status_text)
        .style(status_style)
        .block(Block::default().borders(Borders::ALL));
    frame.render_widget(status, main_chunks[2]);

    let help_text = match app.state {
        AppState::Listing => {
            if app.search_results.is_some() {
                "/:Search │ Esc:Clear │ Space:Mark │ d:Deps │ c:Changelog │ s:Settings │ u:Apply │ q:Quit"
            } else {
                "/:Search │ Space:Mark │ d:Deps │ c:Changelog │ s:Settings │ u:Apply │ r:Refresh │ q:Quit"
            }
        }
        AppState::Searching => "Enter:Confirm │ Esc:Cancel │ Type to search...",
        AppState::ShowingMarkConfirm => "y/Space/Enter:Confirm │ n/Esc:Cancel",
        AppState::ShowingChanges => "y/Enter:Apply │ n/Esc:Cancel │ ↑↓:Scroll",
        AppState::ShowingChangelog => "↑↓/PgUp/PgDn:Scroll │ Esc/q:Close",
        AppState::ShowingSettings => "↑↓:Navigate │ Space/Enter:Toggle │ Esc/q:Close",
        AppState::Upgrading => "Applying changes...",
        AppState::Done => "r:Refresh │ q:Quit",
    };
    let help = Paragraph::new(help_text)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    frame.render_widget(help, main_chunks[3]);
}

fn render_filter_pane(frame: &mut Frame, app: &mut App, area: Rect) {
    let is_focused = app.focused_pane == FocusedPane::Filters;

    // Split into filters and legend
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(7), Constraint::Length(7)])
        .split(area);

    let items: Vec<ListItem> = FilterCategory::all()
        .iter()
        .map(|cat| {
            let style = if *cat == app.selected_filter {
                Style::default().fg(Color::Yellow).bold()
            } else {
                Style::default()
            };
            ListItem::new(cat.label()).style(style)
        })
        .collect();

    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let list = List::new(items)
        .block(
            Block::default()
                .title(" Filters ")
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(list, chunks[0], &mut app.filter_state.clone());

    // Legend
    let legend = vec![
        Line::from(vec![
            Span::styled("↑", Style::default().fg(Color::Yellow)),
            Span::raw(" Upg avail"),
        ]),
        Line::from(vec![
            Span::styled("↑", Style::default().fg(Color::Green)),
            Span::raw(" Upgrading"),
        ]),
        Line::from(vec![
            Span::styled("+", Style::default().fg(Color::Green)),
            Span::raw(" Install"),
        ]),
        Line::from(vec![
            Span::styled("-", Style::default().fg(Color::Red)),
            Span::raw(" Remove"),
        ]),
        Line::from(vec![
            Span::styled("·", Style::default().fg(Color::DarkGray)),
            Span::raw(" Installed"),
        ]),
    ];

    let legend_widget = Paragraph::new(legend)
        .block(
            Block::default()
                .title(" Legend ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        );

    frame.render_widget(legend_widget, chunks[1]);
}

fn render_package_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let is_focused = app.focused_pane == FocusedPane::Packages;
    let visible_cols = Column::visible_columns(&app.settings);

    let header_cells: Vec<Cell> = visible_cols
        .iter()
        .map(|col| Cell::from(col.header()).style(Style::default().fg(Color::Cyan).bold()))
        .collect();
    let header = Row::new(header_cells).height(1);

    let rows: Vec<Row> = app
        .packages
        .iter()
        .map(|pkg| {
            let cells: Vec<Cell> = visible_cols
                .iter()
                .map(|col| match col {
                    Column::Status => Cell::from(pkg.status.symbol())
                        .style(Style::default().fg(pkg.status.color())),
                    Column::Name => {
                        let style = if pkg.is_user_marked {
                            Style::default().fg(Color::White).bold()
                        } else {
                            Style::default()
                        };
                        Cell::from(pkg.name.clone()).style(style)
                    }
                    Column::Section => Cell::from(pkg.section.clone()),
                    Column::InstalledVersion => {
                        if pkg.installed_version.is_empty() {
                            Cell::from("-")
                        } else {
                            Cell::from(pkg.installed_version.clone())
                        }
                    }
                    Column::CandidateVersion => Cell::from(pkg.candidate_version.clone())
                        .style(Style::default().fg(Color::Green)),
                    Column::DownloadSize => Cell::from(pkg.download_size_str()),
                })
                .collect();

            Row::new(cells)
        })
        .collect();

    let widths: Vec<Constraint> = visible_cols.iter().map(|col| col.width(app)).collect();

    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(format!(" Packages ({}) ", app.packages.len()))
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .row_highlight_style(Style::default().bg(Color::DarkGray))
        .highlight_symbol("▶ ");

    frame.render_stateful_widget(table, area, &mut app.table_state);

    if !app.packages.is_empty() {
        let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(Some("↑"))
            .end_symbol(Some("↓"));

        let mut scrollbar_state = ScrollbarState::new(app.packages.len())
            .position(app.table_state.selected().unwrap_or(0));

        let scrollbar_area = Rect {
            x: area.x + area.width - 1,
            y: area.y + 1,
            width: 1,
            height: area.height.saturating_sub(2),
        };
        frame.render_stateful_widget(scrollbar, scrollbar_area, &mut scrollbar_state);
    }
}

fn render_details_pane(frame: &mut Frame, app: &mut App, area: Rect) {
    // Update cached deps if selection changed
    app.update_cached_deps();

    let is_focused = app.focused_pane == FocusedPane::Details;

    let border_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    // Tab header
    let info_style = if app.details_tab == DetailsTab::Info {
        Style::default().fg(Color::Yellow).bold()
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let deps_style = if app.details_tab == DetailsTab::Dependencies {
        Style::default().fg(Color::Yellow).bold()
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let rdeps_style = if app.details_tab == DetailsTab::ReverseDeps {
        Style::default().fg(Color::Yellow).bold()
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let mut content = vec![
        Line::from(vec![
            Span::styled("[Info]", info_style),
            Span::raw(" "),
            Span::styled("[Deps]", deps_style),
            Span::raw(" "),
            Span::styled("[RDeps]", rdeps_style),
        ]),
        Line::from(Span::styled("  (d to switch)", Style::default().fg(Color::DarkGray))),
        Line::from(""),
    ];

    if let Some(pkg) = app.selected_package() {
        match app.details_tab {
            DetailsTab::Info => {
                content.extend(vec![
                    Line::from(vec![
                        Span::styled("Package: ", Style::default().fg(Color::Cyan).bold()),
                        Span::raw(&pkg.name),
                    ]),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("Status: ", Style::default().fg(Color::Cyan)),
                        Span::styled(pkg.status.symbol(), Style::default().fg(pkg.status.color())),
                        Span::raw(format!(" {:?}", pkg.status)),
                    ]),
                    Line::from(vec![
                        Span::styled("Section: ", Style::default().fg(Color::Cyan)),
                        Span::raw(&pkg.section),
                    ]),
                    Line::from(vec![
                        Span::styled("Arch: ", Style::default().fg(Color::Cyan)),
                        Span::raw(&pkg.architecture),
                    ]),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("Installed: ", Style::default().fg(Color::Cyan)),
                        Span::raw(if pkg.installed_version.is_empty() {
                            "(none)"
                        } else {
                            &pkg.installed_version
                        }),
                    ]),
                    Line::from(vec![
                        Span::styled("Candidate: ", Style::default().fg(Color::Green)),
                        Span::raw(&pkg.candidate_version),
                    ]),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("Download: ", Style::default().fg(Color::Cyan)),
                        Span::raw(pkg.download_size_str()),
                    ]),
                    Line::from(vec![
                        Span::styled("Inst Size: ", Style::default().fg(Color::Cyan)),
                        Span::raw(pkg.installed_size_str()),
                    ]),
                    Line::from(""),
                    Line::from(Span::styled(
                        "Description:",
                        Style::default().fg(Color::Cyan).bold(),
                    )),
                    Line::from(pkg.description.clone()),
                ]);
            }
            DetailsTab::Dependencies => {
                if app.cached_deps.is_empty() {
                    content.push(Line::from(Span::styled(
                        "No dependencies",
                        Style::default().fg(Color::DarkGray),
                    )));
                } else {
                    // Group by dep type
                    let mut current_type = String::new();

                    for (dep_type, target) in &app.cached_deps {
                        if dep_type != &current_type {
                            if !current_type.is_empty() {
                                content.push(Line::from(""));
                            }
                            content.push(Line::from(Span::styled(
                                format!("{}:", dep_type),
                                Style::default().fg(Color::Cyan).bold(),
                            )));
                            current_type = dep_type.clone();
                        }

                        let status = app.get_package_status(target);
                        let symbol = status.symbol();
                        let color = status.color();

                        content.push(Line::from(vec![
                            Span::raw("  "),
                            Span::styled(format!("{:1}", symbol), Style::default().fg(color)),
                            Span::raw(" "),
                            Span::raw(target.clone()),
                        ]));
                    }
                }
            }
            DetailsTab::ReverseDeps => {
                if app.cached_rdeps.is_empty() {
                    content.push(Line::from(Span::styled(
                        "No reverse dependencies",
                        Style::default().fg(Color::DarkGray),
                    )));
                } else {
                    content.push(Line::from(Span::styled(
                        format!("{} packages depend on this:", app.cached_rdeps.len()),
                        Style::default().fg(Color::Cyan).bold(),
                    )));
                    content.push(Line::from(""));

                    // Group by dep type
                    let mut current_type = String::new();

                    for (dep_type, pkg_name) in &app.cached_rdeps {
                        if dep_type != &current_type {
                            if !current_type.is_empty() {
                                content.push(Line::from(""));
                            }
                            content.push(Line::from(Span::styled(
                                format!("{}:", dep_type),
                                Style::default().fg(Color::Cyan).bold(),
                            )));
                            current_type = dep_type.clone();
                        }

                        let status = app.get_package_status(pkg_name);
                        let symbol = status.symbol();
                        let color = status.color();

                        content.push(Line::from(vec![
                            Span::raw("  "),
                            Span::styled(format!("{:1}", symbol), Style::default().fg(color)),
                            Span::raw(" "),
                            Span::raw(pkg_name.clone()),
                        ]));
                    }
                }
            }
        }
    } else {
        content.push(Line::from(Span::styled(
            "No package selected",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let title = match app.details_tab {
        DetailsTab::Info => " Details ",
        DetailsTab::Dependencies => " Dependencies ",
        DetailsTab::ReverseDeps => " Reverse Deps ",
    };

    let details = Paragraph::new(content)
        .block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(border_style),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.detail_scroll, 0));

    frame.render_widget(details, area);
}

fn render_mark_confirm_modal(frame: &mut Frame, app: &App, area: Rect) {
    let preview = &app.mark_preview;

    // Build content first to calculate height
    let mut lines = vec![
        Line::from(Span::styled(
            "Mark this package requires additional changes:",
            Style::default().bold(),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("Package: ", Style::default().fg(Color::Cyan)),
            Span::styled(&preview.package_name, Style::default().bold()),
        ]),
        Line::from(""),
    ];

    if !preview.additional_installs.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("The following {} packages will be INSTALLED:", preview.additional_installs.len()),
            Style::default().fg(Color::Green).bold(),
        )));
        for name in &preview.additional_installs {
            lines.push(Line::from(Span::styled(
                format!("  + {}", name),
                Style::default().fg(Color::Green),
            )));
        }
        lines.push(Line::from(""));
    }

    if !preview.additional_removes.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("The following {} packages will be REMOVED:", preview.additional_removes.len()),
            Style::default().fg(Color::Red).bold(),
        )));
        for name in &preview.additional_removes {
            lines.push(Line::from(Span::styled(
                format!("  - {}", name),
                Style::default().fg(Color::Red),
            )));
        }
        lines.push(Line::from(""));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(format!(
        "Total download: {}",
        PackageInfo::size_str(preview.download_size)
    )));

    // Calculate modal size based on content
    let content_height = lines.len() as u16 + 2; // +2 for borders
    let modal_width = 60.min(area.width.saturating_sub(4));
    let modal_height = content_height.min(area.height.saturating_sub(2));
    let modal_x = area.x + (area.width - modal_width) / 2;
    let modal_y = area.y + (area.height - modal_height) / 2;
    let modal_area = Rect::new(modal_x, modal_y, modal_width, modal_height);

    frame.render_widget(Clear, modal_area);

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Additional Changes Required ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Magenta)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(modal, modal_area);
}

fn render_changes_modal(frame: &mut Frame, app: &mut App, area: Rect) {
    let modal_width = 60.min(area.width.saturating_sub(4));
    let modal_height = 20.min(area.height.saturating_sub(2));
    let modal_x = area.x + (area.width - modal_width) / 2;
    let modal_y = area.y + (area.height - modal_height) / 2;
    let modal_area = Rect::new(modal_x, modal_y, modal_width, modal_height);

    frame.render_widget(Clear, modal_area);

    let mut lines = vec![
        Line::from(Span::styled(
            "The following changes will be made:",
            Style::default().bold(),
        )),
        Line::from(""),
    ];

    if !app.pending_changes.to_upgrade.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("UPGRADE ({}):", app.pending_changes.to_upgrade.len()),
            Style::default().fg(Color::Yellow).bold(),
        )));
        for name in &app.pending_changes.to_upgrade {
            lines.push(Line::from(format!("  ↑ {}", name)));
        }
        lines.push(Line::from(""));
    }

    if !app.pending_changes.to_install.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("INSTALL ({}):", app.pending_changes.to_install.len()),
            Style::default().fg(Color::Green).bold(),
        )));
        for name in &app.pending_changes.to_install {
            lines.push(Line::from(format!("  + {}", name)));
        }
        lines.push(Line::from(""));
    }

    if !app.pending_changes.auto_install.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(
                "AUTO-INSTALL (dependencies) ({}):",
                app.pending_changes.auto_install.len()
            ),
            Style::default().fg(Color::Cyan).bold(),
        )));
        for name in &app.pending_changes.auto_install {
            lines.push(Line::from(format!("  A {}", name)));
        }
        lines.push(Line::from(""));
    }

    if !app.pending_changes.to_remove.is_empty() {
        lines.push(Line::from(Span::styled(
            format!("REMOVE ({}):", app.pending_changes.to_remove.len()),
            Style::default().fg(Color::Red).bold(),
        )));
        for name in &app.pending_changes.to_remove {
            lines.push(Line::from(format!("  - {}", name)));
        }
        lines.push(Line::from(""));
    }

    if !app.pending_changes.auto_remove.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(
                "AUTO-REMOVE (no longer needed) ({}):",
                app.pending_changes.auto_remove.len()
            ),
            Style::default().fg(Color::Magenta).bold(),
        )));
        for name in &app.pending_changes.auto_remove {
            lines.push(Line::from(format!("  X {}", name)));
        }
        lines.push(Line::from(""));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(format!(
        "Download size: {}",
        PackageInfo::size_str(app.pending_changes.download_size)
    )));

    let size_change = if app.pending_changes.install_size_change >= 0 {
        format!(
            "+{}",
            PackageInfo::size_str(app.pending_changes.install_size_change as u64)
        )
    } else {
        format!(
            "-{}",
            PackageInfo::size_str((-app.pending_changes.install_size_change) as u64)
        )
    };
    lines.push(Line::from(format!("Disk space change: {}", size_change)));

    let modal = Paragraph::new(lines)
        .block(
            Block::default()
                .title(" Confirm Changes ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.changes_scroll, 0));

    frame.render_widget(modal, modal_area);
}

fn render_changelog_view(frame: &mut Frame, app: &mut App, area: Rect) {
    let pkg_name = app
        .selected_package()
        .map(|p| p.name.clone())
        .unwrap_or_else(|| "Unknown".to_string());

    let lines: Vec<Line> = app
        .changelog_content
        .iter()
        .map(|s| Line::from(s.as_str()))
        .collect();

    let changelog = Paragraph::new(lines)
        .block(
            Block::default()
                .title(format!(" Changelog: {} ", pkg_name))
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .wrap(Wrap { trim: false })
        .scroll((app.changelog_scroll, 0));

    frame.render_widget(changelog, area);
}

fn render_settings_view(frame: &mut Frame, app: &mut App, area: Rect) {
    let column_items = [
        ("Status column (S)", app.settings.show_status_column),
        ("Name column", app.settings.show_name_column),
        ("Section column", app.settings.show_section_column),
        ("Installed version column", app.settings.show_installed_version_column),
        ("Candidate version column", app.settings.show_candidate_version_column),
        ("Download size column", app.settings.show_download_size_column),
    ];

    let mut content = vec![
        Line::from(Span::styled(
            "Column Visibility",
            Style::default().fg(Color::Cyan).bold(),
        )),
        Line::from(""),
    ];

    // Column toggles (items 0-5)
    for (i, (name, enabled)) in column_items.iter().enumerate() {
        let checkbox = if *enabled { "[✓]" } else { "[ ]" };
        let style = if i == app.settings_selection {
            Style::default().fg(Color::Yellow).bold()
        } else {
            Style::default()
        };
        content.push(Line::from(vec![
            Span::styled(
                if i == app.settings_selection { "▶ " } else { "  " },
                style,
            ),
            Span::styled(checkbox, if *enabled { Style::default().fg(Color::Green) } else { Style::default().fg(Color::DarkGray) }),
            Span::raw(" "),
            Span::styled(*name, style),
        ]));
    }

    // Sort section
    content.push(Line::from(""));
    content.push(Line::from(Span::styled(
        "Sorting",
        Style::default().fg(Color::Cyan).bold(),
    )));
    content.push(Line::from(""));

    // Sort by (item 6)
    let sort_style = if app.settings_selection == 6 {
        Style::default().fg(Color::Yellow).bold()
    } else {
        Style::default()
    };
    content.push(Line::from(vec![
        Span::styled(
            if app.settings_selection == 6 { "▶ " } else { "  " },
            sort_style,
        ),
        Span::styled("Sort by: ", sort_style),
        Span::styled(
            app.settings.sort_by.label(),
            Style::default().fg(Color::Green),
        ),
    ]));

    // Sort direction (item 7)
    let dir_style = if app.settings_selection == 7 {
        Style::default().fg(Color::Yellow).bold()
    } else {
        Style::default()
    };
    let dir_label = if app.settings.sort_ascending { "Ascending" } else { "Descending" };
    content.push(Line::from(vec![
        Span::styled(
            if app.settings_selection == 7 { "▶ " } else { "  " },
            dir_style,
        ),
        Span::styled("Direction: ", dir_style),
        Span::styled(dir_label, Style::default().fg(Color::Green)),
    ]));

    let settings = Paragraph::new(content)
        .block(
            Block::default()
                .title(" Settings ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow)),
        );

    frame.render_widget(settings, area);
}
