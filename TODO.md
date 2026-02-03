# synh8 TODO

## Refactoring

- [ ] Use indices instead of string names for mark/unmark operations
  ```rust
  // In core.rs - operations take index into the list
  pub fn toggle(&mut self, index: usize) -> ToggleResult
  pub fn mark(&mut self, indices: &[usize]) -> PreviewResult
  pub fn unmark(&mut self, indices: &[usize]) -> PreviewResult
  ```
  Benefits:
  1. UI just passes the selected index
  2. Core looks up `self.list[index].name` (always base name)
  3. No string confusion between `name` and `display_name`

## Performance
- [ ] Partial list updates in `apply_current_filter()` - only update changed entries instead of full rebuild

## UI/UX
- [ ] Scrollbar position indicator in modals - show "line X of Y" or visual marker
- [ ] Live upgrade output - show apt output in scrolling pane instead of blank screen
- [ ] Download progress bar - visual feedback during package fetches
- [ ] Theming - load colors from config file

## Features
- [ ] Package removal - `-` key marks for removal, shows red `-` in status column
- [ ] Package pinning - `=` key holds package at current version, prevents upgrades
- [ ] Repository filter - filter by origin (main, universe, PPAs)
- [ ] Help screen - `?` or `F1` shows keybindings grouped by context
- [ ] Confirm mark-all - prompt before `x` marks hundreds of packages
- [ ] Refresh package lists - `U` runs `apt update`, shows progress, refreshes view
- [ ] Persist settings - save column visibility and sort order to ~/.config/synh8/config.toml
- [ ] Package history - show install/upgrade dates from /var/log/apt/history.log
- [ ] Custom filters - user-defined filters (e.g., "packages > 100MB")
- [ ] Fix broken packages - `B` attempts to resolve broken dependencies
- [ ] Version selection - picker when multiple candidates exist (different repos/pins)

## Error Messages
- [ ] Interrupted operation - detect and show "Resuming interrupted dpkg operation"

## Polish
- [ ] Typestate pattern - make state transitions compile-time checked
- [ ] Remember scroll position - preserve position when switching filter categories
- [ ] Prominent search indicator - highlight active search query in status bar

## Documentation
- [ ] CLI arguments - `--help`, `--version`, `--dry-run`
