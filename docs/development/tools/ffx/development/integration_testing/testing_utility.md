# Testing utility library

This library implements helpers for writing e2e tests for ffx subtools.

It includes:

* A testing fixture to reduce setup boilerplate
* A hermetic execution environment for ffx ([via `ffx_isolate`](/docs/development/tools/ffx/development/integration_testing/README.md))
* Low-level emulator spawning utilities (for flash and OTA testing)

And is planned to include:

* Matchers for ffx command output (b/293217132)
* `ffx emu` integration by merging with `ffx_e2e_emu` (b/295230484)

## Usage

Subtool e2e tests typically live in `src/developer/ffx/tests`. This is akin to
`ffx self-test` but using `rustc_test` to execute tests instead of a subtool.

In `BUILD.gn`:

```gn
import("//build/host.gni")
import("//build/rust/rustc_binary.gni")

if (is_host) {
  rustc_test("ffx_<something>_test") {
    testonly = true
    edition = "2024"
    source_root = "src/tests.rs"

    sources = [ "src/tests.rs" ]

    deps = [
      "//src/developer/ffx/testing:ffx_testing",
      "//src/lib/fuchsia",
      "//src/lib/testing/fixture",
    ]

    # If using ffx_testing::Emu: Only one emulator can be spawned at a time.
    # These lower level emulators use TAP networking, which is required for IPv6
    # support.
    # args = [ "--test-threads=1" ]
  }
}
```

In `src/tests.rs`:

```rust
use ffx_testing::{base_fixture, TestContext};
use fixture::fixture;

// base_fixture sets up the test environment this test
#[fixture(base_fixture)]
#[fuchsia::test]
async fn my_test(ctx: TestContext) {
  let _daemon = ctx.isolate().start_daemon().await.unwrap();

  let output = ctx.isolate().ffx(&["daemon", "echo"]).await.expect("daemon echo");
  assert!(output.status.success());
}
```

## Low Level Emulators

A low-level emulator can be brought up with `Emu::start`. It can be controlled
via serial and talked to by ffx. For a test that uses the emulator, see
[ffx_target_test](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/developer/ffx/tests/target/src/tests.rs).

**/!\ This emulator is brought up with an empty disk /!\\**

This is by design. To e2e test with a fuchsia image use the `ffx_e2e_emu`
library under `//src/developer/ffx/lib/e2e_emu`.

```rust
use ffx_testing::Emu;

let emu = Emu::start(&ctx);

// NOTE: If using serial, make sure to drain or close the serial stream when no longer in use.
// Failure to do so will result in QEMU halting.
{
  let serial = emu.serial().await;

  // ...
}

// Dropping or letting the emu go out of scope cleans up the emulator.
```
