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

- [x] Ctrl+C opens changelog instead of quitting - added Ctrl+C handler that
  triggers quit/exit-confirm, matching `q` behavior.

- [x] MarkedChanges filter is stale in Dirty state - filter now checks both
  user_intent (Dirty) and APT marks (Planned) so packages appear immediately.

- [x] `x` (mark all upgrades) only marks visible packages - now uses
  `mark_all_upgradable()` which iterates the full cache via `PackageSort::upgradable()`.

- [x] Search FTS5 escaping is incomplete - now strips `+`, `(`, `)`, `*`, `^`,
  `{`, `}`, `:` in addition to `"`. Empty tokens after escaping are filtered out.

- [x] Column width overestimated for native-arch packages - column width now
  calculated from `display_name().len()` instead of full name length.

- [x] Home/End use magic number - replaced with dedicated `select_first_package()`
  and `select_last_package()` methods. Also uses i64 arithmetic in
  `move_package_selection()` to prevent overflow.

- [x] Filter state is cloned for rendering - now passes `&mut app.ui.filter_state`
  directly so ratatui's internal scroll offset is preserved.

## Performance

- [ ] Partial list updates - only update changed entries instead of full rebuild
- [ ] Changelog fetched synchronously - `apt changelog` is run as a blocking subprocess.
  The UI freezes for several seconds on slow connections. The "Loading changelog..."
  message is set but never rendered because the draw loop blocks. (`core.rs:563-583`,
  `app.rs:526`)
- [x] `display_name()` allocates on every call - now caches `native_arch_suffix` string
  in AptCache, computed once at initialization.
- [x] `mark_all_upgrades` rebuilds the list twice - removed the intermediate
  `refresh_ui_state()` call; `show_changes_preview()` handles everything.

## UI/UX

- [ ] Scrollbar position indicator in modals - show "line X of Y" or visual marker
- [x] Live upgrade output - TUI progress display during commit using rust-apt's
  native `DynAcquireProgress` and `DynInstallProgress` callbacks. Shows download
  progress bar with speed/bytes and install progress with step counter.
- [x] Download progress bar - integrated into the live upgrade progress display.
  Uses `Rc<RefCell<ProgressState>>` to share terminal between download and install phases.
- [ ] Theming - load colors from config file
- [ ] Navigation keys ignore focused pane (DEFERRED) - PageUp/PageDown/Home/End/g/G
  always move the package list even when the filter or details pane is focused. Up/Down
  correctly dispatch by pane, but bulk navigation keys don't. (`main.rs:84-107`)
- [ ] Left/Right/d/l always change details tab regardless of focus (DEFERRED) - pressing
  `l` in the package list or `h` in the filter pane changes the details tab instead of
  doing something contextual to the focused pane. (`main.rs:116-119`)
- [x] Filter pane doesn't show package counts - each filter now shows count, e.g.,
  "Upgradable (23)". Counts cached at startup/refresh, MarkedChanges tracks user_intent.
- [x] No cursor shown during text input - cursor now positioned in search bar and
  password modal via `frame.set_cursor_position()`.
- [x] Changes/changelog scroll limits are approximate - `changes_line_count()` now
  computes actual line count from grouped change categories.
- [x] Toggling an installed non-upgradable package silently does nothing - now shows
  feedback message "{pkg} is already installed and up to date".
- [x] Visual mode selection breaks on filter change - visual mode is now cancelled
  when the filter selection changes.

## Features

- [ ] Package removal - `-` key marks for removal, shows red `-` in status column
- [ ] Package pinning - `=` key holds package at current version, prevents upgrades
- [ ] Repository filter - filter by origin (main, universe, PPAs)
- [ ] Help screen - `?` or `F1` shows keybindings grouped by context
- [ ] Confirm mark-all - prompt before `x` marks hundreds of packages
- [x] Refresh package lists - `U` runs `apt update` with live TUI download progress,
  then refreshes the package list view. Uses the same progress rendering as commit.
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
- [x] Ctrl+C now quits instead of opening changelog
- [x] MarkedChanges filter works in Dirty state (checks user_intent)
- [x] `x` marks all upgradable packages from full cache, not just filtered view
- [x] Search FTS5 escaping handles `+(){}:*^` characters
- [x] Column widths use display name length (strips native arch suffix)
- [x] Home/End navigation uses dedicated methods (no i32 overflow)
- [x] Filter state no longer cloned for rendering
- [x] `display_name()` no longer allocates on every call
- [x] `mark_all_upgrades` no longer rebuilds the list twice
- [x] Filter pane shows package counts
- [x] Cursor visible during search and password input
- [x] Changes modal scroll limits computed accurately
- [x] Toggle feedback for installed non-upgradable packages
- [x] Visual mode cancelled on filter change
- [x] Live TUI progress for package commit (download + install phases)
- [x] `U` key runs `apt update` with live download progress
- [x] Root-only mode - app requires root, removed sudo subprocess and password dialog
- [x] Removed `zeroize` dependency (no more password handling)
