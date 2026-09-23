---
trigger: glob
description: Mandatory style guide and coding standards specific to the Netstack3 codebase.
globs: ["src/connectivity/network/netstack3/**"]
---

# Style Guide: Netstack3 Codebase

This style guide encodes mandatory guidelines and conventions specific to the
Netstack3 codebase on Fuchsia, in addition to the general network style guide.

For general Netstack3 contributor guidance and architecture patterns, also refer to the
in-tree documentation at
[netstack3](../../../../../docs/contribute/contributing-to-netstack/netstack3.md).

---

## 1. Sync & Concurrency
*   **No std Mutex**: Only use `Mutex` and `RwLock` from `netstack3_core::sync`
    (often aliased or imported as `CoreMutex`/`CoreRwLock` in bindings) or
    appropriate async-aware locks, not `std::sync` primitives, to support
    testing with loom and avoid lock poisoning issues.

---

## 2. Bindings & Helpers
*   **Utility Module Scoping**: Common helper functions in bindings should be
    stored in a `util` module. Refer to them via the module path prefix
    (e.g. `util::foo(...)`) to clarify they are helper functions.
