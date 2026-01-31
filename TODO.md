# apt-tui TODO

## Critical

### Missing Core Features
- [ ] Add ability to mark packages for removal (not just install/upgrade)
- [ ] Add confirmation dialog when exiting with pending changes

## High Priority

### Error Handling
- [ ] Show specific conflict information when dependency resolution fails

### Performance
- [ ] Consider updating only affected entries in `apply_current_filter()` instead of full rebuild
- [ ] Avoid per-frame string clones in render loop (use `Cow` or references)


### UI/UX
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
- [ ] Make state transitions more explicit (typestate pattern or state machine)

### Error Messages
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
- [ ] Consider lazy evaluation or async loading for cache reload in `restore_marks()`

### Documentation
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
