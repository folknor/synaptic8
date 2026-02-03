# User Flow: Package Manager TUI

## Package States

### Base States (not marked)
- `Installed` - Package is installed, no changes pending
- `NotInstalled` - Package is not installed, no changes pending
- `Upgradable` - Package has an upgrade available, no changes pending

### Marked States
- `MarkedForInstall` - Package will be installed
- `MarkedForUpgrade` - Package will be upgraded
- `MarkedForRemove` - Package will be removed

No visual distinction between "user explicitly marked" vs "marked as dependency". All marked packages look identical.

## Core Rules

1. **Never broken state**: The set of marked packages is always valid and satisfiable
2. **Marking is atomic**: Mark package + all required dependencies together, or nothing
3. **Unmarking cascades**: Unmarking a package also unmarks everything that depends on it

## User Flow

### Marking a Package

1. User presses Space on an unmarked package (e.g., `foo`)
2. APT resolves dependencies (e.g., `foo` needs `bar` and `baz`)
3. If additional **unmarked** packages needed:
   - Show confirmation: "Also mark bar, baz?"
   - **Yes** → `foo`, `bar`, `baz` all become marked
   - **No** → nothing changes, `foo` stays unmarked
4. If no additional unmarked packages needed (deps already marked or none):
   - `foo` becomes marked immediately (no confirmation)

### Unmarking a Package

1. User presses Space on a marked package (e.g., `bar`)
2. Find all marked packages that depend on `bar` (e.g., `foo` depends on `bar`)
3. If cascade needed:
   - Show confirmation: "This will also unmark foo"
   - **Yes** → `bar` and `foo` both become unmarked
   - **No** → nothing changes, all stay marked
4. If no cascade needed:
   - `bar` becomes unmarked immediately (no confirmation)

### Example Scenario

```
Initial state:
  foo      -> Upgradable
  bar      -> Upgradable  (dependency of foo)
  baz      -> Upgradable  (dependency of foo)
  qux      -> Upgradable  (also depends on bar)

Step 1: Toggle foo
  Confirmation: "Also mark bar, baz?"
  User: Yes
  Result:
    foo    -> MarkedForUpgrade
    bar    -> MarkedForUpgrade
    baz    -> MarkedForUpgrade
    qux    -> Upgradable

Step 2: Toggle qux
  bar is already marked, so no confirmation needed
  Result:
    foo    -> MarkedForUpgrade
    bar    -> MarkedForUpgrade
    baz    -> MarkedForUpgrade
    qux    -> MarkedForUpgrade

Step 3(a): Toggle bar (unmark)
  Confirmation: "This will also unmark foo, qux"
  User: Yes
  Result:
    foo    -> Upgradable
    bar    -> Upgradable
    baz    -> Upgradable  (was only needed by foo)
    qux    -> Upgradable

Step 3(b): Toggle qux (unmark) - alternative to 3(a)
  qux has no dependents, so no confirmation needed
  Result:
    foo    -> MarkedForUpgrade
    bar    -> MarkedForUpgrade
    baz    -> MarkedForUpgrade
    qux    -> Upgradable
```

## View Changes (u key)

Shows all marked packages grouped by action:
- Packages to upgrade
- Packages to install
- Packages to remove

## Apply Changes (Enter)

Commits all marked packages to the system, then resets to clean state.
