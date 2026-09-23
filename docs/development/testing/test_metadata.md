# Test metadata

Fuchsia uses `TESTING.json5` files to provide additional metadata
for tests on a per-directory basis. This metadata is used to organize
and visualize test coverage in dashboards and reporting tools.

## Purpose

The primary purpose of `TESTING.json5` is to associate a directory
(and the tests within or under it) with a specific category
and optionally other metadata.

The primary use cases include:

- Visualizing test coverage across different areas of the operating system.
- Identifying areas that lack tests or have uncategorized tests.

## Location

`TESTING.json5` files should be placed at the root of a component,
subsystem, or major directory in the source tree. For example:

- `src/diagnostics/TESTING.json5`

Files placed in a directory apply to that directory and all its
subdirectories, unless overridden by another `TESTING.json5` file
deeper in the tree.

## Format

The file is a JSON5 document containing a single object with a
`coverage` field. The `coverage` field contains zero or more of the
following fields:

- `category`: An optional string representing the category.
- `subcategory`: An optional string providing a more specific subcategory.
- `tags`: An optional list of strings representing tags.

Here is the schema:

```json5
{
  coverage: {
    category: "Category Name",
    subcategory: "Subcategory Name",
    tags: ["tag1", "tag2"],
  },
}
```

## Overriding behavior

A `TESTING.json5` file applies to all subdirectories, but individual fields can
be overridden as follows:

- The `coverage.category` and `coverage.subcategory` fields are exclusive.
  If a subdirectory has one of those fields set, it overrides the matching
  field from the nearest `TESTING.json5` parent.
- The `coverage.tags` field is additive. If a subdirectory has a
  `coverage.tags` field, the tags are merged with the tags from each ancestor
  `TESTING.json5` file.

## Viewing categories

The `fx test-category` tool can be used to view the computed categories for
any directory in the tree. Use this tool to verify that the computed categories
look reasonable and to find uncategorized directories.

Pass a directory to the tool to see the final computed categories:

```posix-terminal
fx test-category src/diagnostics/archivist
```

This shows output:

```none {:.devsite-disable-click-to-copy}
src/diagnostics/archivist:
  category: "Diagnostics"
  subcategory: "Archivist"
  tags: []
```

Pass the `--stats` flag to show instead the aggregated statistics
for the given subdirectory or the whole tree:

```posix-terminal
fx test-category src/diagnostics --stats
```

This shows output:

```none {:.devsite-disable-click-to-copy}
Categories and Subcategories:
  Diagnostics: 214
    Archivist: 45
    Test Only: 38
    None: 32
    Libraries: 26
    Detect: 18
    Utilities: 16
    Sampler: 16
    Persistence: 9
    Triage: 9
    Tools: 5

Tags:
  lib: 26
```

Pass the `--web` flag to launch a simple web page that allows you
to interactively navigate and view categories for the tree:

```posix-terminal
fx test-category --web
```

This shows output:

```none {:.devsite-disable-click-to-copy}
View categories at http://localhost:6240?files=metadata.json
Not seeing the categories you expect? Run `touch BUILD.gn` and then `fx build`

Press enter to stop the server.
```

## Category style guide

The format of `TESTING.json5` is intentionally loose to support incremental
refinement of categories over time, and getting categories wrong temporarily
is very low cost.

There are only a few hard rules to follow, which are enforced automatically:

- Every directory has a category.

  This is satisfied by the top-level `//TESTING.json5` file.

- A directory may not have a subcategory if its category is "Uncategorized".
- Every `TESTING.json5` file must be formatted by `fx format-code`.

### Suggested category layout

Consider the following recommendations:

- Provide categories for the top-level directory of a subsystem or component,
  but not for every subdirectory.

  For example, `src/diagnostics` can set `category: "Diagnostics"`, and
  subdirectories `src/diagnostics/lib` and `src/diagnostics/archivist` simply
  inherit that category.

- Categories may generally map to a team name, but do not have to. They should
  generally match the directory naming.

  For example, category "Diagnostics" matches the team who owns the directory.

- Set subcategories in subdirectories of a categorized directory. These should
  relate to the purpose of the subdirectory.

  For example, `src/diagnostics/archivist` sets
  `subcategory: "Archivist"` to group all Archivist-related tests
  together.

- Set tags for cross-team or cross-component concerns.

  For example, `src/diagnostics/lib` and `src/lib/diagnostics` both set the
  `"lib"` tag. We can then group coverage for all "libs" across the tree.

## Examples

Given the following source layout:

```
src/my_team/
src/my_team/component1/
src/my_team/component2/
src/my_team/lib/
src/my_team/lib/component1_client
src/lib/my_team/server_library
```

Consider creating the following files:

```json5
// src/my_team/TESTING.json5
{
  coverage: {
    category: "My Team",
  },
}

// src/my_team/component1/TESTING.json5
{
  coverage: {
    subcategory: "Component1",
  },
}

// src/my_team/component2/TESTING.json5
{
  coverage: {
    subcategory: "Component2",
  },
}

// src/my_team/lib/TESTING.json5
{
  coverage: {
    tags: ["lib"],
  },
}

// src/lib/my_team/TESTING.json5
{
  coverage: {
    category: "My Team",
    tags: ["lib"],
  },
}
```
