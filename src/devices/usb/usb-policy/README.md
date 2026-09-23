# USB Policy

## Module Layout

This document lays out the main responsibilities of the modules within the USB Policy codebase. It is not intended to capture all nuances of the modules' behaviors, but rather provide an orientation to the layout.

Note: module names (e.g. Controller) are capitalized throughout for clarity.

### Main Loop

Implemented in: [`main.rs`](./src/main.rs)

Responsibilities:

- Serves the FIDL protocols.
- Handles FIDL requests by making calls to other modules.

## Examples of data flow in common situations

The situations below illustrate how the modules cooperate to handle common scenarios. Similar to above, these situations aren't intended to capture every nuance of behavior.

TODO: b/496345275 - Add examples of data flow in common situations
