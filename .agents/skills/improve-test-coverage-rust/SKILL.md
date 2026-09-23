---
name: improve-test-coverage-rust
description: >
  Improve test coverage for Fuchsia Rust code using local line coverage
  profiling, fx coverage, and LCOV analysis.
---

# Rust Coverage Improvement Skill

This skill documents the methodology, tooling, and advanced techniques for
achieving 100% code coverage in Rust components, libraries, and binaries under
Fuchsia.

---

## The Local Coverage Iteration Loop

To establish a coverage-driven development loop, you must compile the library
with the `coverage-rust` variant, execute tests to generate an LCOV profile,
parse the profile to find gaps, and write targeted tests.

### 1. Configure the Build
Clippy is incompatible with variant builds, so it must be disabled during
configuration. Include the target test package using `--with`.

```bash
fx set <product>.<board> --variant coverage-rust --with <test_package_label> --include-clippy=false
```

*Example for minimal product:*
```bash
fx set minimal.x64 --variant coverage-rust --with //zircon/system/ulib/fbl/rust:fbl-rust-test-pkg --include-clippy=false
```

### 2. Perform the Build
Run the incremental build to compile the codebase and package the tests:
```bash
fx build
```

### 3. Generate the LCOV Profile
Run `fx coverage` to spin up a headless emulator, launch the test package,
collect runtime coverage counters, and output an `lcov.info` file:
```bash
fx coverage --lcov-output-path $FUCHSIA_DIR/lcov.info <test_package_name>
```

### 4. Parse the LCOV File
Use the parsing script [scripts/parse-coverage.py](scripts/parse-coverage.py) to
extract coverage metrics for your target crate and list uncovered lines
concisely:
```bash
python3 .agents/skills/improve-test-coverage-rust/scripts/parse-coverage.py <lcov_file> <target_prefix_path>
```

*Example:*
```bash
python3 .agents/skills/improve-test-coverage-rust/scripts/parse-coverage.py lcov.info zircon/system/ulib/fbl/rust/
```

---

## Advanced Coverage Techniques

Many coverage gaps stem from boilerplate, compile-time constants, or safety
assertions. Use these patterns to reach 100% coverage.

### 1. Covering `debug_assert!` Format Arguments
In Rust, `debug_assert!` arguments (like custom error messages containing
formatting variables) are compiled inside the panic block and are **only
evaluated if the assertion fails**. If all tests pass, these formatting lines
show as uncovered.

**Solution**: Add targeted tests marked with `#[cfg(debug_assertions)]` and
`#[should_panic]` that pass invalid inputs to explicitly trigger the panic.
```rust
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "is not aligned")]
fn test_unaligned_pointer_panics() {
    let mut val = Align8(42);
    let raw_ptr = &mut val as *mut Align8;
    let unaligned_ptr = raw_ptr.with_addr(raw_ptr.addr() | 1);
    // Triggers the debug_assert! and executes its formatting arguments!
    let _ = PackedPointer::<Align8, 3>::new(unaligned_ptr, 0);
}
```

### 2. Covering `const fn` at Runtime
Functions marked as `const fn` evaluated at compile-time (such as const generic
parameter generators) do not generate runtime coverage counters.

**Solution**: Add a unit test that invokes the `const fn` at runtime with
non-const variables and asserts the correct output.
```rust
#[test]
fn test_magic_runtime() {
    let m = magic(b"abcd");
    assert_eq!(m, 0x61626364);
}
```

### 3. Documenting Untestable Macro Attribute Lines
Proc macro attributes or derive macros generate code. Even if 100% of the
generated code is hit, LLVM line profile mapping may mark the attribute line
itself as uncovered.
- Once all actual source code is 100% covered, identify and document these
  attribute-line exclusions as **untestable due to LLVM profile mapping
  limitations**. A line coverage of **98%+** where the only missing lines are
  macro attributes is considered functionally 100% covered.

### 4. Handling Unreachable Codepaths (`unreachable!()`, `panic!()`)
The `unreachable!()`, `panic!()`, `todo!()`, and `unimplemented!()` macros
generate panic branches that LLVM line coverage profiling reports as uncovered
unless triggered.
- If one of these macros guards a defensive check against invalid external input
  (e.g., corrupted IPC or FIDL payloads), write a unit test with
  `#[should_panic]` to cover the branch.
- If the macro guards a logically impossible compiler branch (such as an
  exhaustive `match` arm), treating those lines as uncovered mapping limitations
  is acceptable, and the code is considered functionally 100% covered.

### 5. Separation of Concerns for Coverage Tests
When writing unit tests to cover missing paths, follow the **separation of
concerns principle**. Do not write monolithic "catch-all" tests. Instead, write
small, independent unit tests targeting exactly one piece of functionality or
one specific uncovered method.

**Example of Poor Practice (Composite Test):**
```rust
#[test]
fn test_array_extra_methods() {
    // Test new_in
    let a = Array::new_in(alloc);
    assert!(a.is_empty());

    // Test default
    let a_def = Array::default();
    assert!(a_def.is_empty());

    // Test allocate_zeroed fail path
    state.fail_threshold.set(0);
    assert!(alloc.allocate_zeroed(layout).is_err());
}
```

**Example of Good Practice (Granular Separation):**
```rust
#[test]
fn test_array_new_in() {
    let a = Array::new_in(alloc);
    assert!(a.is_empty());
}

#[test]
fn test_array_default() {
    let a_def = Array::default();
    assert!(a_def.is_empty());
}

#[test]
fn test_allocator_allocate_zeroed_failure() {
    state.fail_threshold.set(0);
    assert!(alloc.allocate_zeroed(layout).is_err());
}
```
Benefits:
- If a test fails, it is immediately obvious which exact functionality has
  broken.
- Simpler setup and cleaner, more readable code.
- Avoids mixed assertion logic.
