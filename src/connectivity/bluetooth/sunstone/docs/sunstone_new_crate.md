# Creating a New Rust Crate in Sunstone

This document guides you through the process of creating a new Rust crate under
the Sunstone project and integrating it into both the Cargo workspace and
Fuchsia's build systems (GN and Bazel).

## Overview

Sunstone is designed with a modular architecture consisting of multiple `no_std`
helper crates (like `sapphire-common`, `sapphire-peer-cache`, and
`sapphire-gatt`).

Fuchsia currently uses GN as its active build system (as Bazel integration is
not yet fully ready for the platform).

To integrate our local in-tree crates into the GN build without writing manual
`BUILD.gn` files in every crate directory:

1. We register the local crate in the central
   `third_party/rust_crates/Cargo.toml`.
2. The `fx update-rustc-third-party` tool runs `cargo-gnaw` under the hood to
   automatically generate the production GN targets inside
   `third_party/rust_crates/BUILD.gn`.
3. Other GN targets can then depend on our crate via
   `//third_party/rust_crates:sapphire-<crate-name>`.

_Note: The update script also runs `crate_universe` to generate Bazel bindings
(`BUILD.bazel` files) for future migration readiness, but GN remains the active
compiler. Once Fuchsia fully transitions to Bazel, these integration steps will
change slightly._

---

## Step-by-Step Integration Guide

### Step 1: Create the Crate Directory and Files

Create a new directory for your crate under
`src/connectivity/bluetooth/sunstone/`:

```bash
cargo new <crate_name> --lib
```

You should be able to observe the following files:

#### 1. `Cargo.toml`

```toml
[package]
name = "sapphire-<crate-name>"
version = "0.1.0"
edition = "2024"
publish = false # add this to prevent accidental publication

[dependencies]
# Add crate dependencies here
```

#### 2. `src/lib.rs`

Declare `no_std` compliance if this is a bare-metal/microcontroller target:

```rust
#![no_std]

// Crate implementation...
```

---

### Step 2: Add to Sunstone Cargo Workspace

Verify that the new crate is added to the `members` list (in alphabetical order)
in the Sunstone root `Cargo.toml`
(`src/connectivity/bluetooth/sunstone/Cargo.toml`). This should already be done
when running `cargo new --lib`:

```toml
[workspace]
members = [
  # keep-sorted start
  "sapphire-<crate-name>", # Add your new crate here,
  "sapphire-common",
  "sapphire-gatt",
  "sapphire-peer-cache",
  # keep-sorted end
]
resolver = "3"
```

---

### Step 3: Register in the Central Third-Party Workspace

To allow other crates (and Bazel) to depend on your new crate, you must register
it in `third_party/rust_crates/Cargo.toml`.

#### 1. Add as a dependency

Add your crate to the main `[dependencies]` section in
`third_party/rust_crates/Cargo.toml` in alphabetical order:

```toml
[dependencies]
# keep-sorted start
...
sapphire-<crate-name> = "0.1.0"
# keep-sorted end
```

#### 2. Patch the path

Add the path redirection under the `[patch.crates-io]` section (in the
`### In-tree Crates` block):

```toml
[patch.crates-io]
...
### In-tree Crates: crates which are on crates.io but which we build from our in-tree copy
# keep-sorted start
...
sapphire-<crate-name> = { path = "intree/sunstone/sapphire-<crate-name>" }
# keep-sorted end
```

_(Note: `intree/sunstone` is a pre-existing symlink pointing to
`src/connectivity/bluetooth/sunstone`, so you do not need to create new
symlinks)._

#### 3. Add GN Package Configuration

Add a GN package configuration section (usually placed alphabetically among
other `[gn.package.*]` targets):

```toml
[gn.package.sapphire-<crate-name>."0.1.0"]
uses_fuchsia_license = true
```

---

### Step 4: Regenerate Build Files

Run the Fuchsia utility to regenerate the Cargo lockfile, GN build targets, and
Bazel workspace configurations:

```bash
fx update-rustc-third-party
```

This command will automatically generate:

- `third_party/rust_crates/Cargo.lock`
- `third_party/rust_crates/BUILD.gn`
- `third_party/rust_crates/vendor/...` (Bazel build rules)
- `src/connectivity/bluetooth/sunstone/sapphire-<crate-name>/BUILD.bazel`

---

### Step 5: Configure GN Test Targets

To compile and run host tests, you must declare a test target in
`src/connectivity/bluetooth/sunstone/BUILD.gn`.

#### 1. Add `rustc_test` target

Add your test definition (usually inside `if (is_host)` block):

```gn
if (is_host) {
  ...
  rustc_test("sapphire_<crate_name_underscores>_test") {
    edition = "2024"
    source_root = "sapphire-<crate-name>/src/lib.rs"
    sources = [ "sapphire-<crate-name>/src/lib.rs" ] # add all of your files here
    deps = [
      # Add regular and test dependencies here (e.g. "//third_party/rust_crates:proptest")
    ]
  }
}
```

> [!IMPORTANT]
> Fuchsia's GN build system requires all Rust source files to be explicitly
> listed in the `sources` list. As your crate grows and you add new
> files/modules (e.g., `src/att.rs`), you must manually append them to the
> `sources` array in `BUILD.gn`. Otherwise, the GN build will fail or fail to
> track changes. So for any working commit on your crate, if there are any
> additional files, you must add them here.

#### 2. Add to tests group

Add the test target to the `group("tests")` target:

```gn
group("tests") {
  testonly = true
  deps = [
    ":sapphire_common_test($host_toolchain)",
    ":sapphire_gatt_test($host_toolchain)",
    ":sapphire_peer_cache_test($host_toolchain)",
    ":sapphire_<crate_name_underscores>_test($host_toolchain)", # Add here
  ]
}
```

---

## Verification

To verify that your crate is correctly integrated and compiles, run the
following commands:

1. **Verify `no_std` Compilation**: Run the Sunstone zero-allocation check from
   `src/connectivity/bluetooth/sunstone/`:
   ```bash
   ./check_no_std.sh
   ```

2. **Build the Tests**:
   ```bash
   fx build host_x64/sapphire_<crate_name_underscores>_test
   ```

3. **Run the Tests**:
   ```bash
   fx test sapphire_<crate_name_underscores>_test
   ```

---

## FAQ (Frequently Asked Questions)

### Q: When do I need to run `fx update-rustc-third-party`?

**A:** You **only** need to run this command when you make changes to:

- `third_party/rust_crates/Cargo.toml` (e.g., adding a new crate, modifying
  dependencies, or updating versions).
- Any crate-level metadata that changes how dependencies are resolved.

You **do not** need to run it when:

- You are editing source code (`.rs` files) in your local crates.
- You are adding or editing unit tests.
- You are updating `BUILD.gn` files (unless you introduced a new third-party
  dependency).

---

### Q: When do I need to run `fx build`?

**A:** Run `fx build` (or target-specific builds like
`fx build host_x64/sapphire_<name>_test`) when:

- You want to verify that your code changes compile successfully under the
  Fuchsia build toolchain.
- You modified `BUILD.gn` files.

_Tip:_ For local Rust development, running `cargo check` in your crate directory
(e.g., under `sapphire-gatt/`) is much faster for syntax and type checking. Use
`fx build` as your final compilation check.

---

### Q: Do I need to run `fx build` before `fx test`?

**A:** **No.** Running `fx test sapphire_<name>_test` will automatically trigger
an incremental build for the test target before running the tests. You can
directly run `fx test` during test iteration.

---

### Q: Do I have to run these commands every time I make a change?

**A:**

- **`fx update-rustc-third-party`**: No, only on dependency changes.
- **`fx build`**: No, you can rely on `fx test` to build and test in one step,
  or use `cargo check` for faster local iteration.
- **`fx test`**: Yes, run this whenever you want to verify that your code
  changes did not break any tests.
