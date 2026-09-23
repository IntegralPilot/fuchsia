# TRF Codegen Tool: Internal Codewalk

TRF (Test Realm Factory) is a system for authoring integration tests in Fuchsia by combining a Test Driver (TD), an Injectable Universe (IU), and a Component Under Test (CUT).
However, doing this manually requires writing a lot of boilerplate CML and Rust setup code.

`trf_codegen` provides macro-based tooling to automatically generate this boilerplate for a test.

## Codewalk (TLDR)

This module provides three main macros: `#[trf::test]`, `#[trf::mock]`, and `#[trf::control]`, and an accompanying GN template (`trf_test.gni`).

1. **`trf_test.gni` (`//src/testing/trf_codegen/trf_test.gni`)**
   This defines the build integration. Given a Rust source file and a Component Under Test (CUT), this template:
   - Invokes the `trf_codegen` tool on the source file to parse the macros and generate CML + FIDL components.
   - Generates the Injectable Universe (IU) and Test Driver (TD) components dynamically.
   - Packages them into a `fuchsia_test_package` containing the Test Driver and the RealmBuilder server, with the CUT passed natively as a subpackage.

2. **The Rust Codegen Tool (`//src/testing/trf_codegen/bin`)**
   - **`parser.rs`**: Uses `syn` to parse the provided test source file, specifically extracting `#[trf::test]` entries, `#[trf::mock]` implementation blocks, and `#[trf::control]` methods requested on them.
   - **`consistency_checker.rs`**: Verifies that the implementation does not contain conflicting or invalid mock declarations.
   - **`generator.rs`**: Takes the parsed structure and generates:
     - `mock_control.fidl`: A dynamic FIDL control protocol that the Test Driver uses to talk to the Injectable Universe containing the Mocks.
     - Component manifests for both the test driver (`test_driver.cml`) and injectable universe (`injectable_universe.cml`).
     - Rust setup code for RealmBuilder (`realm_builder_generated.rs`).
     - A stub injected `injectable_universe.rs` containing the Realm Builder local setup code to connect the user's `trf::mock` handlers.

3. **User Experience (`//src/testing/trf_codegen/lib/macro`)**
   The developer uses `#[trf::test]` to define the test entry point and overarching driver harness. They use `#[trf::mock]` for structs they implement as a mock FIDL protocol, and `#[trf::control]` on methods they want exposed for testing over the dynamic control FIDL.
   Inside the `#[trf::test]` handler, an implicit `mock_control` handle is available to steer and coordinate the mock logic from the test driver.

### Usage Example

See `//examples/components/reverser/trf_test` for a concrete usage of these tools.
