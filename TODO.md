# apt-tui TODO

## Critical

### Security
- [ ] Use `zeroize` crate to securely clear `sudo_password` from memory
- [ ] Use absolute path `/usr/bin/sudo` instead of `sudo` to prevent PATH hijacking

### Missing Core Features
- [ ] Add ability to mark packages for removal (not just install/upgrade)
- [ ] Add confirmation dialog when exiting with pending changes

## High Priority

### Error Handling
- [ ] Stop silently discarding `Result` values with `let _ =` pattern
- [ ] Capture and display apt-get stderr on failure (not just generic "failed" message)
- [ ] Check for APT lock files (`/var/lib/dpkg/lock`) before operations
- [ ] Show specific conflict information when dependency resolution fails

### Performance
- [ ] Cache upgradable package count (avoid iterating all packages in `update_status_message`)
- [ ] Wrap SQLite FTS index inserts in a transaction for faster builds
- [ ] Consider updating only affected entries in `apply_current_filter()` instead of full rebuild
- [ ] Avoid per-frame string clones in render loop (use `Cow` or references)

### Architecture
- [ ] Move `update_cached_deps()` out of render function (side effect in render)
- [ ] Split `App` struct (30+ fields) into smaller focused types:
  - `PackageManager` - cache operations, marks, changes
  - `UiState` - focus, scroll, selection
  - `SearchState` - query, results, index

### UI/UX
- [ ] Fix scroll bounds checking (scroll can exceed content height causing blank screens)
- [ ] Add scrollbar position indicator to modals
- [ ] Ensure table viewport scrolls to keep selection visible
- [ ] Live output during upgrade (async)
- [ ] Progress bar for downloads
- [ ] Confirm dialog styling

## Medium Priority

### Features
- [ ] Add package pinning/holding (prevent specific packages from upgrading)
- [ ] Add repository/origin filter
- [ ] Add `?` or `F1` help screen showing all keybindings
- [ ] Add confirmation before mark-all-upgrades (`a`/`x` keys)
- [ ] Add `apt update` equivalent (refresh package lists from repositories)
- [ ] Show cache age / last update time
- [ ] Add `--dry-run` command line flag
- [ ] Persist settings to config file (~/.config/apt-tui/config.toml)

### Accessibility
- [ ] Fix color-only status differentiation (Yellow `↑` vs Green `↑` for colorblind users)
- [ ] Use higher contrast colors for help text (DarkGray is hard to read)
- [ ] Add `--accessible` flag for text-only status indicators
- [ ] Add high-contrast mode option
- [ ] Test with `NO_COLOR` environment variable
- [ ] Theming/color customization

### Code Quality
- [ ] Add `#[must_use]` attributes to functions like `has_pending_changes()`, `is_root()`
- [ ] Rename `PackageStatus::Upgrade` to `MarkedForUpgrade` for clarity
- [ ] Make state transitions more explicit (typestate pattern or state machine)
- [ ] Split `main.rs` into multiple files:
  - `app.rs` - App state and logic
  - `ui.rs` - Render functions
  - `apt.rs` - APT cache wrapper
  - `search.rs` - FTS5 index

### Error Messages
- [ ] Detect APT lock held by another process and show friendly message
- [ ] Show "Resuming interrupted operation" when APT detects partial commit

## Low Priority

### Features
- [ ] Package history view
- [ ] Custom filters
- [ ] Fix broken packages option
- [ ] Version selection (when multiple candidates exist)
- [ ] Expose dependency resolution "fix broken" mode as user option

### Polish
- [ ] Remember scroll position when switching filter categories
- [ ] Make search query more visually prominent when active
- [ ] Remove duplicate keybinding (`a` and `x` both do mark-all-upgrades)
- [ ] Consider lazy evaluation or async loading for cache reload in `restore_marks()`

### Documentation
- [ ] Document the `s` (settings) key in help bar
- [ ] Man page
- [ ] Command-line arguments (--help, --version)

### Packaging
- [ ] Debian packaging (.deb)
- [ ] AUR package

## Notes from Code Review

### What's Working Well
- Solid Rust fundamentals and idiomatic code
- Mark-preview-apply workflow mirrors Synaptic's successful model
- FTS5 search is arguably better than Synaptic's search
- Color-coded status symbols with legend panel
- Contextual help bar that changes based on state
- Smart column width calculation based on content
- Efficient dependency caching (only recalculates on selection change)
- Vim-style keybindings feel natural to Linux power users

### Feature Parity with Synaptic
| Feature | apt-tui | Synaptic |
|---------|---------|----------|
| Filter by status | Yes | Yes |
| Search | Yes (FTS5) | Yes |
| Dependencies view | Yes | Yes |
| Reverse deps | Yes | Yes |
| Multi-select | Yes (visual mode) | Yes |
| Package removal | No | Yes |
| Pin/hold packages | No | Yes |
| Repository management | No | Yes |
| Package history | No | Yes |
| Custom filters | No | Yes |
| Origin/repository filter | No | Yes |
| Fix broken packages | No | Yes |
| Download changelogs | Yes | Yes |
| Lock version | No | Yes |
