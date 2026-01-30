# apt-tui TODO

## Current Status

apt-tui now uses `rust-apt` for proper libapt-pkg integration with full dependency resolution.

---

## Backlog

### Core Features
- [x] Actual package installation
  - [x] Run as root detection
  - [x] Exits TUI for apt progress output
  - [x] Error handling for failed installs
- [x] Package search (`/` to filter by name)
  - [x] SQLite FTS5 in-memory index
  - [x] ~2s build time on first search, instant thereafter
- [x] Dependencies tab in details pane
  - [x] Press 'd' to cycle Info → Deps → RDeps tabs
  - [x] Shows Depends, Pre-Depends, etc.
  - [x] Green ✓ for installed deps, yellow ○ for missing
- [x] Reverse dependencies view
  - [x] Shows packages that depend on the selected package
  - [x] Grouped by dependency type
- [ ] Changelog viewer (`apt changelog`)

### Authentication
- [ ] Sudo integration for actual installs
  - Option A: Require `sudo apt-tui`
  - Option B: Exit TUI temporarily for sudo prompt
  - Option C: Password input widget with `sudo -S`

### UX Improvements
- [x] Mouse support (click to select, scroll)
- [ ] Live output during upgrade (async)
- [ ] Progress bar for downloads
- [ ] Confirm dialog styling
- [x] Handle terminal resize (ratatui handles automatically)

### Polish
- [ ] Config file (~/.config/apt-tui/config.toml)
- [ ] Theming/color customization
- [ ] Command-line arguments (--dry-run, --auto-yes)
- [ ] Man page
- [ ] Debian packaging (.deb)
- [ ] AUR package

---

## Done

### rust-apt Integration
- [x] Integrate rust-apt 0.9 crate
- [x] Use libapt-pkg's Cache for package data
- [x] Proper dependency resolution with `mark_install()` + `resolve()`
- [x] Track user-marked vs auto-marked packages
- [x] `get_changes()` to show all affected packages
- [x] `protect()` to prevent user selections from being removed

### UI Features
- [x] 3-pane layout (Filters | Packages | Details)
- [x] Tab to cycle focus between panes
- [x] Working filter categories:
  - [x] Upgradable
  - [x] Marked Changes
  - [x] Installed
  - [x] Not Installed
  - [x] All Packages
- [x] Package table with columns:
  - [x] Status (↑ + - A X symbols)
  - [x] Package name
  - [x] Installed version
  - [x] Candidate version
  - [x] Download size
- [x] Details pane with package info
- [x] Changes preview modal showing:
  - [x] User-selected upgrades
  - [x] User-selected installs
  - [x] Auto-installed dependencies
  - [x] User-selected removals
  - [x] Auto-removed packages
  - [x] Download size and disk space change
- [x] Vim-style navigation (j/k)
- [x] Mark all upgrades (`a`)
- [x] Unmark all (`n`)
- [x] Space to toggle individual packages
- [x] Scrollbar on package list
- [x] Focus indication (cyan border)

### Dependency Resolution (Synaptic-style)
- [x] When marking a package, APT automatically resolves dependencies
- [x] Auto-marked dependencies shown separately from user selections
- [x] Changes preview shows complete picture before applying
- [x] Download and install size calculations

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                        rust-apt                              │
│                           ↓                                  │
│   Cache::new() → loads /var/lib/apt/lists/*                 │
│                           ↓                                  │
│   pkg.mark_install() → marks package + deps                 │
│   pkg.protect() → prevents removal during resolution        │
│   cache.resolve() → runs APT's dependency resolver          │
│   cache.get_changes() → returns all affected packages       │
│                           ↓                                  │
│   Distinguish user-marked vs auto-marked                    │
│                           ↓                                  │
│   Show changes preview → user confirms                      │
│                           ↓                                  │
│   cache.get_archives() + cache.do_install() (needs root)   │
└─────────────────────────────────────────────────────────────┘
```
