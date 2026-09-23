<!-- Copyright 2026 The Fuchsia Authors. All rights reserved.
     Use of this source code is governed by a BSD-style license that can be
     found in the LICENSE file. -->

# TRF Codegen: Declarative Integration Testing Framework

The TRF (Test Realm Factory) Codegen suite automates the boilerplate
involved in setting up integration tests using the Realm Factory pattern
in Fuchsia. It aims to generate typed Rust harnesses that eliminate
manual FIDL routing overhead, providing developers with a streamlined,
ergonomic testing experience.

It eliminates boilerplate in component integration tests by automatically
parsing Rust test definitions at build time to generate:
- **FIDL Mock Control Libraries** (`fuchsia.trf.mockcontrol`) for
  inspecting and manipulating mock component state.
- **Injectable Universe (IU) Mock Host Components** serving
  protocol mocks with isolated state.
- **Component Manifests (`.cml`)** for the Test Root, Test Driver,
  and Injectable Universe components.
- **Capability Topology Routing** linking Component Under Test (CUT),
  mock components, and the test driver automatically.

## Component Architecture

This directory is the root of the TRF Codegen infrastructure. Currently,
it holds foundational dependencies that the generated code relies upon:

- **`fidl/`**: Contains the protocol definitions (e.g.,
  `fuchsia.trf.factory`) that define the contract for configuring and
  launching a test realm dynamically via a factory component.
- **`lib/runtime/`**: Provides the runtime Rust library
  (`trf_codegen_runtime`). Generated tests use this library's
  capabilities (such as `TestRealm`) as a common, high-level harness
  wrapper to manage component capabilities, connect to mock controls,
  and stream lifecycle events.

---

## User's Guide

### 1. Define the GN Build Target

Import `//src/testing/trf_codegen/trf_test.gni` in your `BUILD.gn` file
and define a `trf_test` target specifying the Rust test source file and
the target Component Under Test (CUT) dependency.

```gn
import("//src/testing/trf_codegen/trf_test.gni")

if (is_fuchsia) {
  trf_test("reverser_trf_integration_test") {
    source = "src/reverser_trf_tester.rs"
    cut = [ "//examples/components/reverser" ]

    # Optional: List of source files containing TRF mock implementations
    mocks = [ "src/mocks.rs" ]

    # Optional: Archive test in the Compatibility Test Framework (CTF)
    # preserving it for backwards compatibility testing on branch cuts
    is_ctf = true
  }
}
```

### 2. Write the Integration Test

In your Rust test source file, import the TRF macros and runtime harness.
Annotate test entry points with `#[trf_test]`:

```rust
// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_examples_reverser as freverser;
use trf_codegen_macro::trf_test;
// Note: The `TestRealm` argument type is mapped to a generated wrapper struct.

#[trf_test(config(switch_case = false, use_replacer = true), mocks(MockReplacer))]
pub async fn test_reverser_trf_mock(realm: &TestRealm) -> Result<(), Error> {
    // Call control proxy methods directly on the realm
    // (Generated only if `mocks(MockReplacer)` is specified)
    realm.set_replacement("Hello".to_string(), "Greetings".to_string()).await?;

    // Connect to protocols exposed by the Component Under Test (CUT)
    let client = realm.connect_to_protocol::<freverser::ReverserMarker>()?;

    let input = "Hello Fuchsia TRF!";
    let output = client.reverse(input).await?;
    assert_eq!(output, "!FRT aishcuF sgniteerG");

    Ok(())
}
```

### 3. Define Protocol Mocks (Optional)

If your Component Under Test relies on external protocols, you can author mock implementations to satisfy them. Mocks are placed in dedicated Rust source files (e.g. `src/mocks.rs`) and passed via the `mocks` argument of the `trf_test` GN template. Use `#[trf::mock]` to implement the mocked protocol, and `#[trf::control]` to define a control channel for the test driver to manipulate the mock's local state:

```rust
use fidl_fuchsia_examples_reverser as freverser;
use futures::StreamExt;
use std::sync::Mutex;
use trf_codegen_macro as trf;

#[trf::mock(protocol = "fuchsia.examples.reverser.Replacer")]
pub struct MockReplacer {
    // Internal mock state must be safely shared across concurrent requests
    from: Mutex<String>,
    to: Mutex<String>,
}

impl MockReplacer {
    pub fn new() -> Self {
        Self { from: Mutex::new(String::new()), to: Mutex::new(String::new()) }
    }

    // 1. Define the control interface for the Test Driver to drive the mock
    #[trf::control]
    pub fn set_replacement(&self, from: String, to: String) {
        *self.from.lock().unwrap() = from;
        *self.to.lock().unwrap() = to;
    }

    // 2. Define the protocol server loop served to the CUT.
    pub async fn serve_replacer(&self, mut stream: freverser::ReplacerRequestStream) -> Result<(), anyhow::Error> {
        while let Some(Ok(req)) = stream.next().await {
            match req {
                freverser::ReplacerRequest::Replace { value, responder } => {
                    let from = self.from.lock().unwrap().clone();
                    let to = self.to.lock().unwrap().clone();
                    let result = if !from.is_empty() { value.replace(&from, &to) } else { value };
                    let _ = responder.send(&result);
                }
            }
        }
        Ok(())
    }
}
```

### 4. Run the Test

Execute your integration test using standard `fx` workflow commands:

```bash
fx add-test //path/to/your/tests:reverser_trf_integration_test
fx test //path/to/your/tests
```
