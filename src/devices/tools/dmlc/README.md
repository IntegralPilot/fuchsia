# Driver Manifest Language Compiler (`dmlc`)

`dmlc` is the compiler for the **Driver Manifest Language (DML)** in Fuchsia's
[Driver Framework v2 (DFv2)][dfv2].

In DFv2, authoring device drivers and board drivers historically required maintaining
multiple disconnected manifest files with duplicate information:
* Component Manifests (`.cml`) specifying runner, binary path, service routing, and capabilities.
* Bind Rules (`.bind`) specifying device matching bytecode and composite parent node definitions.
* Board configurations (`.fidl`) and metadata serialization code.

**DML** consolidates these declarations into a single, unified JSON5 manifest (`.dml`).
`dmlc` compiles this single source of truth into valid `.cml`, compiled `.bind` rules,
board FIDL binaries, and auto-generated C++ / Rust metadata parser libraries.

---

## Features

* **Unified Manifests**: Define component lifecycle, service capabilities, and hardware bind rules in one file.
* **Hardware Bus Blocks**: Native support for **Platform Bus**, **PCI**, **USB**, **ACPI**, and **Devicetree**.
* **Composite Binding**: Automatic parent node binding, optional parents, and `match_name: true` matching.
* **Conditional Branching**: Declarative `one_of` branches and `accept` value lists.
* **Automatic Init Step Filtering**: Automatically configures init step bind rules (`fuchsia.gpio.Init`, etc.) and filters them out of runtime CML.
* **Schema-Driven Metadata Parsers**: Auto-generates type-safe C++ and Rust parsers from embedded Draft-07 JSON Schemas.
* **Board Topology Compilation**: Compiles board manifests, capability offers, and metadata mappings directly to FIDL board configs.

---

## Schema & Validation

The official JSON Schema for DML files is located at:
```text
src/devices/tools/dmlc/dml.schema.json
```

It is a Draft-07 JSON Schema providing syntax definitions and editor autocomplete for:
* Driver manifests (`name`, `program`, `use`, `capabilities`, `expose`, `include`, `config`)
* Board manifests (`children`, `offer`, `metadata_mappings`)
* Bus blocks (`pci`, `usb`, `acpi`)
* Composite parents and inline bind blocks (`bind`, `requirements`)

For full syntax reference, see the [DML Reference Documentation][dml-reference].

---

## Command-Line Usage

The `dmlc` binary supports two subcommands: `compile-driver` and `compile-board`.

### 1. Compile Driver Manifest (`compile-driver`)

Compiles a driver `.dml` file into `.cml`, `.bind`, and optional metadata parsers:

```bash
dmlc compile-driver <input.dml> \
  --cml-output <output.cml> \
  --bind-output <output.bind> \
  --h-output <output.h> \
  --cc-output <output.cc> \
  --rs-output <output.rs> \
  --namespace <namespace>
```

Options:
* `input_file` (positional): Path to the input `.dml` file.
* `--cml-output`: Path to the generated Component Manifest (`.cml`).
* `--bind-output`: Path to the generated Bind source file (`.bind`).
* `--h-output`: Path to the generated C++ metadata parser header (`.h`).
* `--cc-output`: Path to the generated C++ metadata parser implementation (`.cc`).
* `--rs-output`: Path to the generated Rust metadata parser source (`.rs`).
* `--namespace`: Optional C++ namespace for generated metadata classes.

### 2. Compile Board Configuration (`compile-board`)

Compiles a board `.dml` manifest (and its shards) into a board CML, board bind rules, and serialized FIDL board configuration:

```bash
dmlc compile-board <board.dml> \
  --out-dir <out_dir> \
  --cml-output <board.cml> \
  --bind-output <board.bind> \
  --fidl-output <board-config.fidl> \
  --driver-dml <driver1.dml> \
  --driver-dml <driver2.dml>
```

Options:
* `input_file` (positional): Path to the root board `.dml` file.
* `--out-dir`: Output directory for generated artifacts.
* `--cml-output`: Path to the generated board `.cml`.
* `--bind-output`: Path to the generated board `.bind`.
* `--fidl-output`: Path to the serialized `fuchsia.hardware.platform.bus/BoardConfig` FIDL binary.
* `--driver-dml`: Additional driver DML files providing metadata schemas.

---

## Driver Manifest vs Board Manifest

### Driver Manifest (`compile-driver`)

Used by individual device drivers (standalone or composite).

```json5
{
  name: "acpi-battery",
  include: [
    "//sdk/lib/driver/compat/compat.shard.cml"
  ],
  program: {
    compat: "driver/acpi-battery.so",
    colocate: "true",
    default_dispatcher_opts: [ "allow_sync_calls" ]
  },
  use: [
    {
      service: "fuchsia.hardware.interrupt.Service",
      name: "irq000"
    },
    {
      service: "fuchsia.hardware.acpi.Service",
      name: "acpi",
      primary: true,
      bind: {
        banjo: "fuchsia.acpi.BIND_PROTOCOL.DEVICE",
        acpi: {
          hid: "PNP0C0A"
        }
      }
    }
  ]
}
```

### Board Manifest (`compile-board`)

Used by board drivers (e.g. `vim3.dml`, `test-board.dml`) to instantiate platform devices, route capabilities, and aggregate device properties into board-level metadata.

```json5
{
  name: "vim3",
  program: {
    driver_name: "vim3-dml",
    bind: {
      service: "fuchsia.hardware.platform.bus.Service",
      transport: "Driver",
      vid: "fuchsia.khadas.platform.BIND_PLATFORM_DEV_VID.KHADAS",
      pid: "fuchsia.khadas.platform.BIND_PLATFORM_DEV_PID.VIM3"
    }
  },
  include: [
    "buttons.shard.dml",
    "gpio.shard.dml"
  ],
  metadata_mappings: [
    {
      metadata_id: "fuchsia.hardware.gpio.Metadata",
      aggregations: [
        {
          service: "fuchsia.hardware.gpio.Service",
          field: "pins"
        }
      ]
    }
  ]
}
```

---

## Testing & Verification

Run the unit tests for `dmlc`:

```bash
fx test dmlc_bin_test
```

Format code:

```bash
fx format-code
```

---

## Further Reading

* [DML Language Reference][dml-reference] (`docs/development/drivers/dml_reference.md`)
* [DFv2 Overview][dfv2] (`docs/development/drivers/dfv2-overview.md`)
* [JSON Schema for DML](dml.schema.json)

[dfv2]: /docs/development/drivers/dfv2-overview.md
[dml-reference]: /docs/development/drivers/dml_reference.md
