# Rust Unsafe Analyzer (`rust-unsafe-analyzer`)

`rust-unsafe-analyzer` is a static analysis tool that parses Rust source code (`.rs` files) into an
Abstract Syntax Tree (AST) using `syn` 2.0 and locates syntactic `unsafe` constructs.

It is designed to enforce Fuchsia's Unsafe Rust code review requirements (described in
[`//docs/development/languages/rust/unsafe.md`](//docs/development/languages/rust/unsafe.md)) by
identifying additions or modifications to `unsafe` Rust code and prompting authors to request an
unsafe code review (`+1` in Gerrit from `fuchsia-rust-unsafe-reviews@google.com`).

---

## Shac Integration

The tool is integrated into Fuchsia's presubmit and host static analysis pipeline via Shac:

- **Shac Check**: `//scripts/shac/rust_unsafe_analyzer.star`
- **Execution**: Run locally via `fx host-tool shac check --only rust-unsafe-analyzer`.

When run under Shac (`//scripts/shac/rust_unsafe_analyzer.star`), the analyzer runs against affected
`.rs` files outside `third_party/`. Shac filters findings by checking whether the exact line number
of the `unsafe` keyword token (`unsafe_line`) is in the set of newly added or modified lines in the
change (`meta.new_lines()`). Existing untouched `unsafe` code in an affected file is ignored.

---

## What This Tool CAN Catch

Because `rust-unsafe-analyzer` performs a complete syntactic AST traversal, it reliably detects
explicit `unsafe` keywords in the source code of a file across all categories.

1. **`unsafe { ... }` Blocks**

   ```rust
   unsafe {
       // Raw pointer dereference, unsafe FFI call, or unsafe method call
   }
   ```

2. **`unsafe fn ...` functions and methods**

   ```rust
   unsafe fn dangerous_free_func() {}

   impl Foo {
       unsafe fn dangerous_method(&self) {}
   }

   trait Bar {
       unsafe fn dangerous_trait_method();
   }
   ```

3. **`unsafe trait ...` trait definitions**

   ```rust
   unsafe trait SendSyncTrusted {}
   ```

4. **`unsafe impl ...` trait implementations**

   ```rust
   unsafe impl SendSyncTrusted for MyType {}
   ```

6. **macro containing an unsafe keyword**

    ```rust
    macro_rules! unsafe_macro {
        ($v:expr) => {
            unsafe {
                std::str::from_utf8_unchecked($v)
            }
        }
    }
    ```

---

## What This Tool CANNOT Catch

`rust-unsafe-analyzer` is a syntactic AST parser (`syn`), not a full compiler frontend or semantic
analyzer (`rustc` / `miri`). Therefore, there are specific limitations on what it can detect:

1. **Macro Expansions & Generated Code**
   - If an external macro (`macro_rules!` or procedural macro) generates an `unsafe` block or
     function inside its expansion, `rust-unsafe-analyzer` cannot inspect the post-expansion AST
     unless the macro invocation itself explicitly includes an `unsafe` token in its arguments in
     the source file.

2. **Indirect Unsafety & Safety Invariants in Safe Code**
   - Many soundness bugs in safe abstractions occur in safe functions that uphold invariants
     required by private `unsafe` blocks. Modifying a safe helper function that breaks an assumption
     relied upon by an existing untouched `unsafe` block will not be flagged unless the line containing
     the `unsafe` keyword itself is modified.

3. **External C FFI / Foreign Module Items (`extern "C"`)**
   - Declarations inside an `extern "C" { ... }` block (`ItemForeignMod`) are implicitly unsafe to
     call in Rust, but the declaration itself does not use the `unsafe` keyword. The tool flags the
     caller (`unsafe { ... }` block invoking the extern function), but not the bare `extern` block
     declaration.

4. **Semantic Soundness or Undefined Behavior**
   - The tool does not verify whether an `unsafe` block is sound, whether invariants are documented,
     or whether pointers are valid. Its sole purpose is to detect the syntactic presence of
     new/modified `unsafe` constructs and require human expert review.

---

## Running & Testing

### Running Unit Tests

Unit tests verify AST detection across all `UnsafeKind` categories and assert on the complete
`Finding` struct (`path`, `kind`, `line`, `end_line`, `col`, `end_col`, `unsafe_line`, and
`message`).

Run tests using:

```bash
% fx test --host rust-unsafe-analyzer-test
```

### Running Manually

To run the analyzer binary directly against a set of Rust source files:

```bash
% fx host-tool shac check --only rust_unsafe_analyzer
```

It outputs any findings to `stdout`.
