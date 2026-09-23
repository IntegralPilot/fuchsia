# Component monikers

A [component moniker][glossary.moniker] identifies a specific component instance
in the component tree using a topological path of child names.

This section describes the syntax used for displaying monikers to users.

## Child names {#child}

Parents assign names to each of their children. Dynamically created children
are arranged by their parent into named collections.

A child name is represented by the child's static name (assigned in a
component manifest), or collection name and the runtime-assigned child name
delimited by `:`.

Syntax: `{name}` or `{collection}:{name}`

Examples:

- `carol`
- `support:dan` - The collection `support` with the child `dan`.

The `{name}` and `{collection}` must follow the regex `[-_.a-z0-9]{1,100}`.
That is, a string of 1-100 of the following characters: `a-z`, `0-9`, `_`, `.`,
`-`.

See the [component manifest reference][cml-reference] for more details.

## Monikers {#monikers}

Represented by the minimal sequence of child names encountered when tracing
downwards to the target delimited by a `/` (slash).

Monikers do not support upward traversal (i.e. `..`) (from child to parent).

Examples:

- `.` - self - no traversal needed
- `carol` - a child - traverse down `carol`
- `carol/sandy` - a grandchild - traverse down `carol` then down `sandy`
- `support:dan` - a child - traverse down into collection child `support:dan`

[glossary.moniker]: /docs/glossary/README.md#moniker
[cml-reference]: https://fuchsia.dev/reference/cml
