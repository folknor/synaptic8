//! Common types used throughout the application

use ratatui::prelude::*;

/// Package status matching Synaptic's status icons
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageStatus {
    Upgradable,      // ↑ Package can be upgraded (yellow)
    MarkedForUpgrade, // ↑ Package marked for upgrade (green)
    Install,         // + Package marked for install
    Remove,          // - Package marked for removal
    Keep,            // = Package kept at current version
    Installed,       // · Package is installed (no changes)
    NotInstalled,    //   Package is not installed
    Broken,          // ✗ Package is broken
    AutoInstall,     // + Automatically installed (dependency)
    AutoRemove,      // - Automatically removed
}

impl PackageStatus {
    pub fn symbol(&self) -> &'static str {
        match self {
            Self::Upgradable | Self::MarkedForUpgrade => "↑",
            Self::Install | Self::AutoInstall => "+",
            Self::Remove | Self::AutoRemove => "-",
            Self::Keep => "=",
            Self::Installed => "·",
            Self::NotInstalled => "",
            Self::Broken => "✗",
        }
    }

    pub fn color(&self) -> Color {
        match self {
            Self::Upgradable => Color::Yellow,
            Self::MarkedForUpgrade => Color::Green,
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
pub enum FilterCategory {
    Upgradable,
    MarkedChanges,
    Installed,
    NotInstalled,
    All,
}

impl FilterCategory {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Upgradable => "Upgradable",
            Self::MarkedChanges => "Marked Changes",
            Self::Installed => "Installed",
            Self::NotInstalled => "Not Installed",
            Self::All => "All Packages",
        }
    }

    pub fn all() -> &'static [FilterCategory] {
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
pub struct PackageInfo {
    pub name: String,
    pub status: PackageStatus,
    pub section: String,
    pub installed_version: String,
    pub candidate_version: String,
    pub installed_size: u64,
    pub download_size: u64,
    pub description: String,
    pub architecture: String,
    pub is_user_marked: bool,
}

impl PackageInfo {
    pub fn size_str(bytes: u64) -> String {
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

    pub fn installed_size_str(&self) -> String {
        Self::size_str(self.installed_size)
    }

    pub fn download_size_str(&self) -> String {
        Self::size_str(self.download_size)
    }
}

/// Which pane has focus
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FocusedPane {
    Filters,
    Packages,
    Details,
}

/// Which tab is shown in details pane
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailsTab {
    Info,
    Dependencies,
    ReverseDeps,
}

/// Application state machine
#[derive(Debug, PartialEq, Eq)]
pub enum AppState {
    Listing,
    Searching,          // User is typing a search query
    ShowingMarkConfirm, // Popup showing additional changes when marking a package
    ShowingChanges,     // Final confirmation before applying all changes
    ShowingChangelog,   // Viewing package changelog
    ShowingSettings,    // Settings/preferences view
    EnteringPassword,   // Entering sudo password
    Upgrading,
    Done,
}

/// Sort options
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortBy {
    Name,
    Section,
    InstalledVersion,
    CandidateVersion,
}

impl SortBy {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Section => "Section",
            Self::InstalledVersion => "Installed version",
            Self::CandidateVersion => "Candidate version",
        }
    }

    pub fn all() -> &'static [SortBy] {
        &[Self::Name, Self::Section, Self::InstalledVersion, Self::CandidateVersion]
    }
}

/// User settings (not persisted yet)
#[derive(Debug, Clone)]
pub struct Settings {
    pub show_status_column: bool,
    pub show_name_column: bool,
    pub show_section_column: bool,
    pub show_installed_version_column: bool,
    pub show_candidate_version_column: bool,
    pub show_download_size_column: bool,
    pub sort_by: SortBy,
    pub sort_ascending: bool,
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
pub enum ApplyResult {
    NeedsPassword,
    NeedsCommit,
}

/// Additional changes required when marking a single package
#[derive(Debug, Default, Clone)]
pub struct MarkPreview {
    pub package_name: String,
    pub is_marking: bool, // true = marking for install, false = unmarking
    pub additional_installs: Vec<String>,
    pub additional_upgrades: Vec<String>,
    pub additional_removes: Vec<String>,
    pub download_size: u64,
}

/// Changes to be applied
#[derive(Debug, Default)]
pub struct PendingChanges {
    pub to_install: Vec<String>,
    pub to_upgrade: Vec<String>,
    pub to_remove: Vec<String>,
    pub auto_install: Vec<String>,
    pub auto_upgrade: Vec<String>,
    pub auto_remove: Vec<String>,
    pub download_size: u64,
    pub install_size_change: i64,
}

/// Column configuration for the package table
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Column {
    Status,
    Name,
    Section,
    InstalledVersion,
    CandidateVersion,
    DownloadSize,
}

impl Column {
    pub fn header(&self) -> &'static str {
        match self {
            Self::Status => "S",
            Self::Name => "Package",
            Self::Section => "Section",
            Self::InstalledVersion => "Installed",
            Self::CandidateVersion => "Candidate",
            Self::DownloadSize => "Download",
        }
    }

    pub fn width(&self, col_widths: &ColumnWidths) -> Constraint {
        match self {
            Self::Status => Constraint::Length(3),
            Self::Name => Constraint::Min(col_widths.name),
            Self::Section => Constraint::Length(col_widths.section),
            Self::InstalledVersion => Constraint::Length(col_widths.installed),
            Self::CandidateVersion => Constraint::Length(col_widths.candidate),
            Self::DownloadSize => Constraint::Length(10),
        }
    }

    pub fn visible_columns(settings: &Settings) -> Vec<Column> {
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

/// Column width storage
#[derive(Debug, Clone, Default)]
pub struct ColumnWidths {
    pub name: u16,
    pub section: u16,
    pub installed: u16,
    pub candidate: u16,
}

impl ColumnWidths {
    pub fn new() -> Self {
        Self {
            name: 10,
            section: 7,
            installed: 9,
            candidate: 9,
        }
    }

    pub fn reset(&mut self) {
        self.name = 7;      // "Package"
        self.section = 7;   // "Section"
        self.installed = 9; // "Installed"
        self.candidate = 9; // "Candidate"
    }
}