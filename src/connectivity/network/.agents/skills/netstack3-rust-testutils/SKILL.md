---
name: netstack3-rust-testutils
description: >
  Describes how to add a new `testutil` module to a Netstack3 Rust library
  using the rustc_library_with_features GN template so that other crates can
  depend on its test utilities.
---

# Adding Rust Crate `testutil` Modules with Proper Visibility

When developing netstack components or helper libraries under
`src/connectivity/network/netstack3`, you often need to share test mocks, fakes,
and utilities across multiple crates.

To avoid duplicate definitions and ensure type compatibility between different
build configurations, you should use the custom GN template
`rustc_library_with_features` (defined in
`//src/connectivity/rustc_library_with_features.gni`). This template compiles
the exact same Rust crate multiple times using different features, allowing
test-only helper functions in one crate to seamlessly depend on test-only helper
functions in another crate without changing the library name in Rust source
code.

Remember, this is only necessary if the test utilities need to be exposed to
other crates.

## Core Concepts

### 1. The GN Pattern: `rustc_library_with_features`

In Fuchsia, a target name in a `BUILD.gn` file defines the name of the output
library or binary. Normally, if you defined a separate crate like
`netstack3-base-testutils`, Rust would see it as a different crate from
`netstack3-base`, making types defined in one incompatible with the other.

The `rustc_library_with_features` template solves this by compiling multiple
variants of the *same* Rust crate. The production variant compiles with standard
settings, and the testutils variant compiles with the `"testutils"` feature
enabled and a custom `target_name` (e.g. `netstack3-base-testutils`), but both
variants generate a crate named `netstack3_base` in Rust.

### 2. Why Gate `testutil` with `#[cfg(any(test, feature = "testutils"))]`?

Gating your `testutil` module with the conditional compilation attribute
`#[cfg(any(test, feature = "testutils"))]` is crucial for two key reasons:

1.  Eliminating Production Overhead: Test-only code (such as mocks, fakes, and
    assertion macros) should never be compiled into a production binary. By
    gating the entire module, you ensure that all test utilities, helper
    functions, and their test-only dependencies (like `assert_matches`) are
    stripped out when building for release, keeping the binary lightweight and
    secure.
2.  Enabling Cross-Crate Reuse: Standard unit tests use `cfg(test)`.  However,
    when library A is compiled as a dependency of library B's tests, library A's
    `cfg(test)` is *not* active, meaning library B cannot see library A's
    internal test helpers. Sponsoring a `"testutils"` feature compiles library A
    with the `testutils` flag enabled, which satisfies `feature = "testutils"`
    and exposes its public test helpers to library B's tests.

## Step-by-Step Implementation Guide

### Step 1: Update the Crate's `BUILD.gn`

To convert a standard library or add test utilities to an existing library,
follow these steps in the library's `BUILD.gn`:

1.  Import the GNI template: Ensure the following import is at the top of your
    `BUILD.gn`:
   ```gn
   import("//src/connectivity/rustc_library_with_features.gni")
   ```

2.  Use the template: Replace your `rustc_library` target with
    `rustc_library_with_features`.
3.  Add the `"testutils"` feature set: Within the `rustc_library_with_features`
    block, specify your library details and define `feature_sets`.
   - The default feature set defines the production version.
   - The second feature set defines the `testutils` variant (which sets
     `testonly = true`, enables the `"testutils"` feature, and specifies a
     suffix or distinct `target_name`).

   ```gn
   import("//src/connectivity/rustc_library_with_features.gni")

   rustc_library_with_features("my-library") {
     edition = "2024"

     # List all source files, including your testutil files
     sources = [
       "src/lib.rs",
       "src/testutil.rs", # Or src/testutil/mod.rs, etc.
     ]

     # Production dependencies
     deps = [
       "//sdk/fidl/fuchsia.net:fuchsia.net_rust",
     ]

     # Additional dependencies required ONLY when compiling the testutils feature
     _testutils_deps = [
       "//third_party/rust_crates:assert_matches",
     ]

     feature_sets = [
       # 1. Default Production Crate target
       {
         features = []
       },

       # 2. Shared Test Utilities Crate target
       {
         target_name = "my-library-testutils"
         testonly = true
         features = [ "testutils" ]
         deps += _testutils_deps

         # If depending on other libraries with testutils, depend on their -testutils targets:
         # deps += [ "//src/connectivity/network/netstack3/core/base:netstack3-base-testutils" ]
       },
     ]
   }
   ```

### Step 2: Conditionally Compile the Rust Module

In your Rust source files, guard your test utility modules and exports using the
conditional compilation attribute `#[cfg(any(test, feature = "testutils"))]`.

For example, in `my-library/src/lib.rs`:

```rust
// Only compile the `testutil` module when testing or building with testutils.
#[cfg(any(test, feature = "testutils"))]
pub mod testutil;
```

And in `my-library/src/testutil.rs`:

```rust
// Types and functions defined here will be available to both internal tests
// and external crates depending on `my-library-testutils`.
pub struct FakeContext {
    // ...
}

impl FakeContext {
    pub fn new() -> Self {
        Self { /* ... */ }
    }
}
```

### Step 3: Propagate Dependencies Consistently

If another crate (e.g. `my-dependent-crate`) needs to consume your test
utilities, it *must* also propagate the variants consistently to avoid type
incompatibilities.

1.  In `my-dependent-crate/BUILD.gn`: When defining its own variants using
    `rustc_library_with_features`, map the dependency correctly:

   ```gn
   rustc_library_with_features("my-dependent-crate") {
     ...
     feature_sets = [
       # Production target depends on standard library
       {
         features = []
         deps += [
           "//src/connectivity/network/path/to:my-library",
         ]
       },

       # Testutils target depends on my-library-testutils
       {
         target_name = "my-dependent-crate-testutils"
         testonly = true
         features = [ "testutils" ]
         deps += [
           "//src/connectivity/network/path/to:my-library-testutils",
         ]
       },
     ]
   }
   ```

2.  In `my-dependent-crate/src/lib.rs`: Use the types from your helper library
    normally. The Rust compiler will resolve the crate name `my_library` to the
    appropriate compiled variant:

   ```rust
   #[cfg(any(test, feature = "testutils"))]
   pub mod testutil {
       use my_library::testutil::FakeContext;

       pub fn create_helper() -> FakeContext {
           FakeContext::new()
       }
   }
   ```

## Advanced: Coalescing and Re-exporting Nested Test Utilities

In complex crates (such as those under `netstack3-core`), implementations are
typically encapsulated inside private `internal` submodules to hide
implementation details, while public APIs are re-exported at the crate root or
via public domain submodules (like `ip`, `device`, `tcp`, etc.).

Instead of exposing all the internal submodules or creating a single giant flat
`testutil.rs` file at the root, you should keep test utilities located next to
the code they test inside the `internal` modules, and then coalesce and
re-export them under a single public `testutil` module at the crate's public
boundaries.

### Example: Netstack3 Re-export Pattern

1.  Define the internal test utility: In a deep internal file (e.g.,
    `src/internal/device/slaac.rs`):
   ```rust
   #[cfg(any(test, feature = "testutils"))]
   pub mod testutil {
       pub fn calculate_slaac_addr() { ... }
   }
   ```

2.  Expose it via public boundaries: In your public module definition (e.g.,
    `src/lib.rs` or a public domain submodule like `pub mod device` in
    `src/lib.rs`):
   ```rust
   pub mod device {
       // Expose standard public APIs...
       pub use crate::internal::device::slaac::SlaacConfiguration;

       // Coalesce and re-export all device-related testutils into a single pub mod testutil
       #[cfg(any(test, feature = "testutils"))]
       pub mod testutil {
           pub use crate::internal::device::slaac::testutil::calculate_slaac_addr;
           pub use crate::internal::device::testutil::with_assigned_ipv4_addr_subnets;
       }
   }
   ```

This pattern ensures that:

- Test utilities live next to the code they verify, maintaining modularity.
- Consumers of your crate interact with a clean, consolidated interface at
  `my_crate::device::testutil` without needing to understand or traverse the
  crate's internal directory structure.
