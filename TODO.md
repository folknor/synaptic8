# synh8 TODO

## Bugs / Limitations

- [ ] Virtual package dependency resolution - cascade unmark doesn't work for packages
  with virtual package dependencies (e.g., nvidia packages). The dependency check only
  matches direct package names, not virtual package providers.

  Example: `libnvidia-extra-590` depends on `nvidia-kernel-common-590-590.48.01` (virtual),
  which is provided by `nvidia-kernel-common-590`. When trying to unmark
  `nvidia-kernel-common-590`, the cascade fails because the names don't match.

  Workaround: Users can unmark the original user-marked package, which correctly
  clears all associated dependencies.

## Performance

- [ ] Partial list updates - only update changed entries instead of full rebuild

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

- [ ] Remember scroll position - preserve position when switching filter categories
- [ ] Prominent search indicator - highlight active search query in status bar

## Documentation

- [ ] CLI arguments - `--help`, `--version`, `--dry-run`
- [ ] README with screenshots and feature list

## Done

- [x] Typestate pattern - state transitions are now compile-time checked via `ManagerState` enum
- [x] Consolidate mark/unmark - unified `toggle()` API with cascade handling
- [x] Use PackageId instead of string names - eliminates name mismatch bugs
