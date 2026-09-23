---
trigger: glob
description: Mandatory style guide and coding standards for Rust code in the network codebase.
globs: ["src/connectivity/network/**"]
---

# Rust Style Guide: network codebase

This style guide encodes the mandatory guidelines, conventions, and design
patterns established for Rust code in the network codebase on Fuchsia. All agents and
developers working on Rust code in this area must strictly adhere to these rules.

For general network contributor guidance and Rust patterns, also refer to the
in-tree documentation at
[contributing-to-netstack](../../../../../docs/contribute/contributing-to-netstack/README.md)
and
[rust-patterns](../../../../../docs/contribute/contributing-to-netstack/rust-patterns.md).

---

## 1. Module Architecture & Structure
*   **Module Landing Files**: Always use the modern Rust approach of using a
    flat file module landing (e.g., `src/ip.rs`) instead of the legacy `mod.rs`
    in a folder (e.g., `src/ip/mod.rs`).
*   **Submodules**: Place submodule implementations in a sibling folder (e.g.,
    `src/ip/device.rs` and `src/ip/types.rs` are declared inside `src/ip.rs`).

---

## 2. General styling and preferences.
*   **No Dead Code Blanket Suppressions**: Never use `#![allow(dead_code)]` or
    `#[allow(dead_code)]` blanket overrides. For scaffolded or unused structs,
    fields, and constants, always use **`#[expect(unused)]`** (or
    `#[expect(dead_code)]`) directly on the symbols to ensure clean
    compiler-driven warnings.
*   **Response Destructuring**: Always prefer **destructuring** FIDL response
    structs/tables/unions directly at the binding site instead of using
    dot-accessors. This makes it explicitly clear to reviewers what parameters
    are being processed and what fields are ignored.
    *   *Exhaustive Struct Destructuring*: Always destructure FIDL and local
        structs **exhaustively**. Avoid using the `..` wildcard ellipsis for
        structs (the `..` ellipsis is strictly reserved only for FIDL tables).
        Use an underscore prefix for any explicitly ignored variables or markers
        (e.g., `__source_breaking`) to suppress unused warnings.
        ```rust
        // Example of exhaustive FIDL struct destructuring:
        let MyFidlStruct {
            field_we_use,
            __source_breaking: fidl::marker::SourceBreaking,
        } = my_fidl_struct;
        ```
*   **Exhaustive Matching & Flexible Types**: Prefer to match enum and union
    variants exhaustively.
    *   *Flexible Enums/Unions*: For FIDL flexible enums and unions, always
        include the `__SourceBreaking` variant match to handle unknown variants.
    *   *Capturing Unsupported Variants*: When handling or logging an
        unsupported or unknown variant, always capture the unknown ordinal
        and print it dynamically inside your error/debug log line.
        ```rust
        // Example of capturing unsupported variant for a FIDL flexible union:
        match my_fidl_union {
            MyFidlUnion::KnownVariant => { ... }
            MyFidlUnion::__SourceBreaking { unknown_ordinal } => {
                log::warn!("unsupported variant ordinal: {unknown_ordinal}");
            }
        }
        ```
*   **Inlined Format Arguments**: Always prefer inlining format arguments inside
    format strings and macros (e.g., `log::error!("failed: {error:?}");`) rather
    than passing them as trailing positional arguments (e.g.,
    `log::error!("failed: {:?}", error);`), unless formatting a complex
    expression.
*   **No Exclamation Marks**: Avoid utilizing exclamation marks (`!`) in all
    logging statements, comments, and user-facing strings. Keep these
    strings objective, descriptive, and direct.
*   **Type-Safe Casting**: Avoid unsafe or silent `as` casts (e.g., `bar.size as
    usize`) unless strictly required by hot-path performance constraints. Always
    prefer type-safe conversions:
    *   Use `usize::from(...)` where the conversion is guaranteed to succeed.
    *   Use `usize::try_from(...).expect("justification")` where the conversion
        could fail, ensuring loud and explicit panic crashes in case of overflow
        with clear justification.
*   **Type-Annotating Unused Results**: When discarding non-trivial results (like
    Zircon syscall `Result`s or handles) via `let _ = ...`, always
    explicitly annotate the ignored type (e.g., `let _: Result<(), zx::Status> = ...`)
    to ensure type clarity and prevent silent mistakes.
*   **Mandatory Unsafe Safety Comments**: All `unsafe` blocks must be preceded
    by a descriptive `// SAFETY: <explanation>` comment detailing exactly why
    the unsafe call is valid, how the preconditions are met, and why it is
    guaranteed to be safe and not trigger undefined behavior.
*   **Import macros from log**: Always import the used macros from the log crate
    instead of using FQN at call site.
*   **Alias fuchsia_async**: Always alias `fuchsia_async` to `fasync` when
    importing the module. You can import unambiguous symbols from
    `fuchsia_async` directly.
*   **No numbered comment lists**: Don't use comments with numbered stages in
    function bodies like `// 1. Foo`, `// 2. Bar`. Keep the section comments as
    simple sentences but avoid the numbering.
*   **Futures**: Always import `Future`, `Stream`, `Sink` directly from the
    `futures` crate and don't use FQN.
*   **Present Tense in Comments**: All comments (both doc comments and inline
    comments) must use the **present tense** (e.g., "Returns the status," not
    "Will return the status" or "Returned the status").

---

## 3. Protocol & RFC Specifications
*   **Context & Citations**: Always document protocol state machines, packet
    formats, and complex logic with explicit doc comments detailing their
    purpose, alongside **verbatim RFC section and paragraph citations**
    (e.g., RFC 8200 Section 4.5).

---

## 4. Crate scaffolding
* **Lints**: All Rust crates and binaries in `src/connectivity/network`
  must have configs:
  ```gn
  configs += [
    "//build/config/rust/lints:deny_unused_results",
  ]
  ```
