---
name: ffx-inspect
description: >
  Browse, query, filter, search, and diff Fuchsia Component Inspect data locally
  using JSON exports and jq.
---

# Fuchsia Component Inspect Skill (`ffx-inspect`)

This skill provides the ability to browse, query, filter, search, and diff
diagnostic data exposed by components on a running Fuchsia target through the
Inspect API.

To avoid repeated hits to the target device and reduce developer environment
load, this skill implements a **two-step local processing strategy**:

1. Dump the full inspect state once as a JSON file.
2. Query, filter, and compare the JSON locally using `jq` and standard diffing
   tools.

> **Important:**
> Always use `ffx --machine json inspect show` to capture the inspect data, and save it to
> a temporary file in the workspace or `/tmp`. Clean up these temporary files
> when you are done.
> 
> The agent or user can capture a new snapshot of the inspect state from the
> device at any time (for example, to see if states changed after triggering an action).

## Workflow

### 1. Capture full inspect state
To capture the full inspect state of the running system and save it locally:

```posix-terminal
ffx --machine json inspect show > /tmp/inspect_dump.json
```

You can re-read the inspect state from the device at any time (for example, to
`/tmp/inspect_dump2.json`) to check if values have changed or to perform local
diffing.

### 2. Search and filter locally with jq

#### A. Filter by component moniker
To find only components whose monikers match a pattern (for example,
`archivist`):

```posix-terminal
jq '[.[] | select(.moniker | test("archivist"; "i"))]' /tmp/inspect_dump.json
```

#### B. Search for node or property keys
To recursively search the tree for any node or property keys matching a pattern
(for example, `status`):

```posix-terminal
jq '[.[] | . as $item | {moniker: .moniker, matches: [paths | select(.[-1] | tostring | test("status"; "i")) | {path: (.[1:] | join("/")), value: ($item | getpath(.))}]}]' /tmp/inspect_dump.json
```

#### C. Search for leaf values
To recursively search for properties whose values match a pattern (for example,
`OK`):

```posix-terminal
jq '[.[] | . as $item | {moniker: .moniker, matches: [paths(scalars) | select(($item | getpath(.)) | tostring | test("OK"; "i")) | {path: (.[1:] | join("/")), value: ($item | getpath(.))}]}]' /tmp/inspect_dump.json
```

#### D. Truncate tree depth
To prune the inspect tree nesting level beyond depth `N` (for example, `2`
levels deep relative to the payload root):

```posix-terminal
jq 'map(delpaths([paths | select(length > 3)]))' /tmp/inspect_dump.json
```

---

### 3. Compare and diff runs
To find values that changed between two inspect runs (for example,
`/tmp/run1.json` and `/tmp/run2.json`):

1. **Flatten and filter volatile fields** (such as timestamps) from both runs
   into sorted text files:

   ```posix-terminal
   jq -r '.[] | .moniker as $m | paths(scalars) as $p | select($p[-1] | tostring | test("timestamp|time"; "i") | not) | "\($m):\($p[1:] | join("/")) = \(. | getpath($p))"' /tmp/run1.json | sort > /tmp/run1_flat.txt
   jq -r '.[] | .moniker as $m | paths(scalars) as $p | select($p[-1] | tostring | test("timestamp|time"; "i") | not) | "\($m):\($p[1:] | join("/")) = \(. | getpath($p))"' /tmp/run2.json | sort > /tmp/run2_flat.txt
   ```

2. **Diff the flattened text files**:

   ```posix-terminal
   diff -u /tmp/run1_flat.txt /tmp/run2_flat.txt
   ```

---

### 4. Fast component health check (fx system-status)
Fuchsia has a dedicated host command `fx system-status` that fetches and parses
standard `fuchsia.inspect.Health` nodes from all components:

```posix-terminal
fx system-status
```

This is a quick way to check if any component has crashed or failed to boot
correctly before doing a deep inspect dive.

---

### 5. Clean up
Always clean up any temporary files generated during your troubleshooting
session:

```posix-terminal
rm -f /tmp/inspect_dump.json /tmp/run*_flat.txt
```

---

### 6. Explain results
When explaining your findings or when the user asks how to reproduce your
output, **always** provide them with the exact `ffx inspect` command piped to
`jq` and describe what it does.
