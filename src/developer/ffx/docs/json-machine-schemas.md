<!-- Copyright 2026 The Fuchsia Authors. All rights reserved. -->
<!-- Use of this source code is governed by a BSD-style license that can be -->
<!-- found in the LICENSE file. -->

# FFX Machine-Readable JSON Schemas Specification

This document blueprints the architectural framework and type safety constraints governing machine-readable JSON outputs inside `fuchsia.git`.

## Architectural Overview

To support automation scripts, continuous integration hooks, and high-level developer orchestration tools, many `ffx` subcommand plugins emit structured, machine-parseable outputs when invoked with the global flag:

```sh
ffx --machine json <subcommand>
```

To ensure these data blocks maintain strict backward compatibility contracts and do not induce silent breakdown regressions downstream, all machine-parseable types are strongly typed and validated via compile-time schemas.

## The Schemars Integration Framework

Fuchsia utilizes the `schemars` Rust crate to enforce programmatic schema generation. Plugin authors define an output payload enum or structure type representing the console data payload and derive the traits:

```rust
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Debug, Serialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CommandOutputMessage {
    Message(String),
    Data(serde_json::Value),
}
```

Deriving `JsonSchema` enables the compilation toolchain to automatically reflect and extract the corresponding schema layout mapping matrix.

## Output Transmission via `VerifiedMachineWriter`

Plugins that support machine-readable output use a specialized writer class `VerifiedMachineWriter` provided by the `ffx_writer` framework library:

```rust
impl FfxMain for MyTool {
    type Writer = VerifiedMachineWriter<CommandOutputMessage>;

    async fn main(self, mut writer: Self::Writer) -> fho::Result<()> {
        // ...
        writer.item(&CommandOutputMessage::Message("Operation successful.".into()))?;
        Ok(())
    }
}
```

`VerifiedMachineWriter` guarantees that all values written to stdout cleanly match the derived structural layout specification parameters.

## Golden Schema Verification Gates

To prevent unintentional API breaks, the build system automatically tracks and asserts all CLI schemas using an automated test integration sub-suite called `cli-goldens` located at:

```
//src/developer/ffx/tests/cli-goldens/
```

When fields are altered, added, or modified inside a plugin's argument or output structures, the build system generates the dynamic actual schemas and asserts them against the source Goldens under `cli-goldens/goldens/ffx/`. Mismatches will instantly halt the build process, requiring explicit developer review and synchronization acknowledgement commands:

```sh
cp out/default/host_x64/obj/src/developer/ffx/tests/cli-goldens/goldens/... <source_tree_path>
```

This double-check mechanism ensures complete schema governance health across the repository tree.
