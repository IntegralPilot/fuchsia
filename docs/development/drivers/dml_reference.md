# Driver Manifest Language (DML) Reference

This document provides a comprehensive reference for the **Driver Manifest Language (DML)** in
Fuchsia's [Driver Framework v2 (DFv2)][dfv2].

---

## 1. Overview

In Driver Framework v2 (DFv2), drivers are components executed by the driver runner.
Historically, authoring a DFv2 driver required maintaining multiple disconnected configuration files:
* A **Component Manifest (`.cml`)**: Declaring capabilities, used services, runner configuration, and binary locations.
* A **Bind Rules File (`.bind`)**: Declaring driver matching rules, hardware identifiers, and composite parent specifications.
* **Board Configurations (`.fidl`)**: Board drivers required board topology manifests, capability route mappings, and static metadata payloads.
* **Metadata Deserialization Boilerplate**: Manual deserialization of structured FIDL dictionaries or raw metadata bytes.

Maintaining these files separately introduced duplication: service names, parent node names, and
hardware protocol requirements were repeated across both `.cml` and `.bind` files, creating a risk
of drift and configuration bugs.

**DML (Driver Manifest Language)** solves this by providing a single, unified declarative JSON5 manifest
(`.dml`). The DML compiler (`dmlc`) processes `.dml` files and automatically produces:
1. **Component Manifests (`.cml`)**: Cleaned and validated component manifests ready for compilation with `cmc`.
2. **Bind Rules (`.bind`)**: Bind program source code compiled with `bindc` into driver bytecode (`.bindbc`).
3. **FIDL Board Configurations**: Board topology binaries (`BoardConfig`) for platform bus drivers.
4. **C++ and Rust Metadata Parsers**: Type-safe parser libraries automatically generated from embedded JSON Schemas.

The official schema for DML manifests is checked in at
[`//src/devices/tools/dmlc/dml.schema.json`](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/tools/dmlc/dml.schema.json).

---

## 2. Driver Manifest vs Board Manifest

DML supports two categories of manifests:

| Feature | Driver Manifest (`compile-driver`) | Board Manifest (`compile-board`) |
| :--- | :--- | :--- |
| **Target** | Standalone or composite leaf/intermediate device drivers | System board drivers (e.g. Vim3, QEMU, Astro) |
| **Outputs** | `.cml`, `.bind`, optional C++/Rust metadata parsers | `.cml`, `.bind`, `.fidl` board configuration |
| **Top-Level Sections** | `name`, `program`, `use`, `capabilities`, `expose`, `include`, `config` | `name`, `program`, `children`, `offer`, `metadata_mappings`, `use`, `include`, `capabilities`, `expose` |
| **Binding Style** | Non-composite (`program.bind`) or composite (`use[].bind`) | Platform bus device binding (`program.bind`) |
| **Hardware Nodes** | Consumes parent nodes | Declares child nodes and capability routes (`offer`) |

---

## 3. Syntax Reference

### 3.1 Top-Level Properties

```json5
{
  // Name of the driver or board component (required).
  name: "sample_driver",

  // Optional composite name override. Defaults to 'name' with '-' replaced by '_'.
  composite_name: "custom_composite_name",

  // Shards and manifests to include.
  include: [
    "//sdk/lib/driver/compat/compat.shard.cml",
    "subsystem.shard.dml"
  ],

  // Execution parameters and standalone bind rules.
  program: { ... },

  // Capabilities consumed, and parent node definitions for composite drivers.
  use: [ ... ],

  // Capabilities provided by this driver, including metadata schemas.
  capabilities: [ ... ],

  // Capabilities exposed to the framework or parent component.
  expose: [ ... ],

  // Optional structured component configuration schema.
  config: { ... },

  // Board manifest only: child device node definitions.
  children: [ ... ],

  // Board manifest only: capability offers and constraint routing.
  offer: [ ... ],

  // Board manifest only: rules for aggregating metadata across children.
  metadata_mappings: [ ... ]
}
```

---

### 3.2 The `program` Section

The `program` section defines driver execution parameters and bind rules for standalone (non-composite) drivers.

| Property | Type | Description |
| :--- | :--- | :--- |
| `runner` | string | Component runner name. Defaults to `"driver"`. |
| `binary` | string | Path to driver shared library (e.g. `"driver/sample.so"`). Defaults to `"driver/<name>.so"`. |
| `compat` | string | Path to DFv1 compatibility driver shared library (e.g. `"driver/sample_compat.so"`). |
| `colocate` | string or boolean | Whether to colocate the driver in its parent driver host (`"true"` or `true`). |
| `default_dispatcher_opts` | array of string | Options for the driver's default dispatcher (e.g. `["allow_sync_calls"]`). |
| `bind` / `requirements` | object | Structured bind block for standalone drivers (see [Hardware Bus Blocks](#4-hardware-bus-blocks)). Note: string bind paths are not allowed in DML. |
| `driver_name` | string | Driver name override (primarily used in board manifests). |

#### Example: Standalone Driver `program` Block

```json5
program: {
  binary: "driver/usb_mass_storage.so",
  colocate: "true",
  default_dispatcher_opts: [ "allow_sync_calls" ],
  bind: {
    protocol: "fuchsia.usb.BIND_PROTOCOL.INTERFACE",
    usb: {
      class: "fuchsia.usb.BIND_USB_CLASS.MASS_STORAGE",
      subclass: "fuchsia.usb.massstorage.BIND_USB_SUBCLASS.SCSI",
      protocol: "fuchsia.usb.massstorage.BIND_USB_PROTOCOL.BULK_ONLY"
    }
  }
}
```

---

### 3.3 The `use` Section

The `use` section serves a dual purpose:
1. **Component Capability Routing**: Passing standard CML capabilities (`service`, `protocol`, `directory`).
2. **Composite Parent Node Specification**: Defining the parent nodes that a composite driver binds to.

| Property | Type | Description |
| :--- | :--- | :--- |
| `service` | string | FIDL service name (e.g. `"fuchsia.hardware.gpio.Service"`). Emits `fuchsia.Service == "<service>";` in parent bind rules. |
| `protocol` | string | FIDL protocol name. |
| `banjo` | string | Banjo protocol identifier (e.g. `"fuchsia.gpio.BIND_PROTOCOL.DEVICE"`). Emits `fuchsia.BIND_PROTOCOL == <banjo>;` in bind rules and is excluded from CML. |
| `name` / `instance_name` | string | Name of the parent node in the composite device specification. |
| `primary` | boolean | Set to `true` on exactly one `use` entry to designate it as the primary parent node. |
| `availability` | string | `"required"` (default) or `"optional"`. Generates `optional parent "<name>"` in bind rules. |
| `transport` | string | Service transport: `"Zircon"`, `"Driver"`, or `"Banjo"`. |
| `generate_bind_rule` | boolean | Defaults to `true`. If set to `false`, the capability is consumed via CML without generating a composite parent bind node. |
| `bind` / `requirements` | object | Inline bind rules for this parent node (see [Hardware Bus Blocks](#4-hardware-bus-blocks)). |

#### Special Handling for Initialization Steps

Services representing platform init steps (such as `fuchsia.gpio.Init` and `fuchsia.pwm.Init`) are automatically detected:
* They generate the appropriate bind rule (e.g. `fuchsia.BIND_INIT_STEP == fuchsia.gpio.BIND_INIT_STEP.GPIO;`).
* They are automatically filtered out of runtime CML `use` entries because init steps are transient bind milestones rather than connectable runtime FIDL services.

---

### 3.4 The `capabilities` and `expose` Sections

#### Capabilities

Declares capabilities provided by the driver. In addition to standard CML capability types, DML supports **schema-driven metadata definitions**:

```json5
capabilities: [
  {
    service: "fuchsia.hardware.buttons.Service"
  },
  {
    metadata: {
      id: "fuchsia.hardware.buttons.Metadata",
      schema: {
        title: "ButtonsMetadata",
        type: "object",
        definitions: {
          ButtonItem: {
            type: "object",
            properties: {
              type: { type: "integer", fuchsia_type: "uint8" },
              gpio: { type: "integer", fuchsia_type: "uint32" }
            },
            required: [ "type", "gpio" ]
          }
        },
        properties: {
          buttons: {
            type: "array",
            items: {
              "$ref": "#/definitions/ButtonItem"
            }
          }
        },
        required: [ "buttons" ]
      }
    }
  }
]
```

When `dmlc compile-driver` is executed with `--h-output`, `--cc-output`, or `--rs-output`, it generates complete C++ and Rust parser code that parses and validates incoming metadata against the schema.

#### Expose

Standard DFv2 capability exposure:

```json5
expose: [
  {
    service: "fuchsia.hardware.buttons.Service",
    from: "self"
  }
]
```

---

### 3.5 Board Manifest Top-Level Sections

Board manifests (compiled with `dmlc compile-board`) configure the platform bus and topology:

#### `children`

Declares devices published to the platform bus:

```json5
children: [
  {
    name: "adc-buttons",
    url: "fuchsia-pkg://fuchsia.com/adc-buttons#meta/adc-buttons.cm",
    compatible: "fuchsia,adc-buttons",
    metadata: [
      {
        id: "fuchsia.hardware.adc.Metadata",
        data: [ 1, 0, 0, 0 ]
      }
    ]
  }
]
```

#### `offer`

Routes capabilities between parent controllers and child drivers with hardware constraints:

```json5
offer: [
  {
    service: "fuchsia.hardware.gpio.Service",
    name: "power",
    from: "#gpio-controller-ff634400",
    to: "#gpio-buttons",
    constraints: {
      pin: 92,
      name: "power"
    }
  }
]
```

#### `metadata_mappings`

Aggregates child device constraints into unified FIDL metadata:

```json5
metadata_mappings: [
  {
    metadata_id: "fuchsia.hardware.pinimpl.Metadata",
    aggregations: [
      {
        service: "fuchsia.hardware.gpio.Service",
        field: "pins"
      },
      {
        service: "fuchsia.hardware.pin.PinStatesService",
        field: "device_pin_states",
        use_node_name: true
      }
    ]
  }
]
```

---

## 4. Hardware Bus Blocks

DML provides structured blocks for major hardware interconnects and discovery protocols.

### 4.1 Platform Bus & Devicetree

Platform devices match on Vendor ID (VID), Product ID (PID), Device ID (DID), or Devicetree `compatible` strings:

```json5
bind: {
  vid: "fuchsia.khadas.platform.BIND_PLATFORM_DEV_VID.KHADAS",
  pid: "fuchsia.khadas.platform.BIND_PLATFORM_DEV_PID.VIM3",
  did: "fuchsia.platform.BIND_PLATFORM_DEV_DID.GPIO",
  compat: "fuchsia,gpio-buttons"
}
```

Generated bind rules:
```bind
fuchsia.BIND_PLATFORM_DEV_VID == fuchsia.khadas.platform.BIND_PLATFORM_DEV_VID.KHADAS;
fuchsia.BIND_PLATFORM_DEV_PID == fuchsia.khadas.platform.BIND_PLATFORM_DEV_PID.VIM3;
fuchsia.BIND_PLATFORM_DEV_DID == fuchsia.platform.BIND_PLATFORM_DEV_DID.GPIO;
fuchsia.COMPATIBLE == "fuchsia,gpio-buttons";
```

### 4.2 PCI Bus

PCI devices match on PCI vendor, device, class, subclass, interface, revision, or topology:

Note: Legacy flat PCI fields (`pci_class`, `pci_subclass`, `pci_interface`) are rejected by `dmlc`. Always use the structured `pci: { ... }` block.

```json5
bind: {
  service: "fuchsia.hardware.pci.Service",
  pci: {
    vid: "fuchsia.pci.BIND_PCI_VID.INTEL",
    did: "0x1234",
    class: "fuchsia.pci.BIND_PCI_CLASS.GENERIC_SYSTEM_PERIPHERAL",
    subclass: "0x05",
    interface: "0x01",
    revision: "0x04",
    topo: "0x05"
  }
}
```

Generated bind rules:
```bind
fuchsia.Service == "fuchsia.hardware.pci.Service";
fuchsia.BIND_PCI_VID == fuchsia.pci.BIND_PCI_VID.INTEL;
fuchsia.BIND_PCI_DID == 0x1234;
fuchsia.BIND_PCI_CLASS == fuchsia.pci.BIND_PCI_CLASS.GENERIC_SYSTEM_PERIPHERAL;
fuchsia.BIND_PCI_SUBCLASS == 0x05;
fuchsia.BIND_PCI_INTERFACE == 0x01;
fuchsia.BIND_PCI_REVISION == 0x04;
fuchsia.BIND_PCI_TOPO == 0x05;
```

### 4.3 USB Bus

USB interfaces and devices match on USB vendor, product, class, subclass, protocol, and interface number:

```json5
bind: {
  usb: {
    vid: "fuchsia.usb.BIND_USB_VID.GOOGLE",
    pid: "0x1234",
    class: "fuchsia.usb.BIND_USB_CLASS.MASS_STORAGE",
    subclass: "0x02",
    protocol: 0,
    interface_number: 1,
    bind_protocol: "fuchsia.usb.BIND_PROTOCOL.INTERFACE"
  }
}
```

Generated bind rules:
```bind
fuchsia.BIND_PROTOCOL == fuchsia.usb.BIND_PROTOCOL.INTERFACE;
fuchsia.BIND_USB_VID == fuchsia.usb.BIND_USB_VID.GOOGLE;
fuchsia.BIND_USB_PID == 0x1234;
fuchsia.BIND_USB_CLASS == fuchsia.usb.BIND_USB_CLASS.MASS_STORAGE;
fuchsia.BIND_USB_SUBCLASS == 0x02;
fuchsia.BIND_USB_PROTOCOL == 0;
fuchsia.BIND_USB_INTERFACE_NUMBER == 1;
```

### 4.4 ACPI Bus

ACPI devices match on Hardware ID (`hid`), Compatible ID (`first_cid`), or ACPI Bus Type:

```json5
bind: {
  acpi: {
    hid: "PNP0C0A",
    first_cid: "PNP0C0B",
    bus_type: "fuchsia.acpi.BIND_ACPI_BUS_TYPE.PCI"
  }
}
```

Generated bind rules:
```bind
fuchsia.acpi.HID == "PNP0C0A";
fuchsia.acpi.FIRST_CID == "PNP0C0B";
fuchsia.BIND_ACPI_BUS_TYPE == fuchsia.acpi.BIND_ACPI_BUS_TYPE.PCI;
```

---

## 5. Composite Binding Rules & Parent Matching

Composite drivers require binding to multiple parent devices (e.g. a platform device, plus GPIO pins, plus I2C buses).

### 5.1 Primary Parent Node

Designate the primary parent with `primary: true` in its `use` entry:

```json5
use: [
  {
    service: "fuchsia.hardware.platform.device.Service",
    name: "pdev",
    primary: true,
    bind: {
      compat: "sample,buttons"
    }
  },
  {
    service: "fuchsia.hardware.gpio.Service",
    name: "mic-mute",
    availability: "optional"
  }
]
```

Generated bind rules:
```bind
composite sample_composite;

using fuchsia;

primary parent "pdev" {
  fuchsia.COMPATIBLE == "sample,buttons";
}

optional parent "mic-mute" {
  fuchsia.Service == "fuchsia.hardware.gpio.Service";
}
```

Note: If a driver is a composite driver, specifying bind rules under `program.bind` is forbidden. Place all bind rules in the corresponding `use` entries instead.

### 5.2 Parent Node Matching (`match_name: true`)

When a driver connects to multiple parents of the same service type (e.g. multiple GPIO pins or ADCs), specify `match_name: true` inside the parent's `bind` block. `dmlc` will automatically emit a rule matching the parent's topological node name (`fuchsia.NAME`):

```json5
use: [
  {
    service: "fuchsia.hardware.gpio.Service",
    name: "volume-up",
    bind: {
      match_name: true
    }
  }
]
```

Generated bind rules:
```bind
parent "volume-up" {
  fuchsia.Service == "fuchsia.hardware.gpio.Service";
  fuchsia.NAME == "volume-up";
}
```

### 5.3 Parent Grouping

If multiple `use` entries reference the same parent `name` (for example, one for a Banjo interface and another for a FIDL service or init step), `dmlc` automatically groups them into a single parent specification:

```json5
use: [
  {
    service: "fuchsia.gpio.Init",
    name: "gpio-init"
  },
  {
    service: "fuchsia.hardware.gpio.Service",
    name: "gpio-init",
    availability: "optional"
  }
]
```

Generated bind rules:
```bind
parent "gpio-init" {
  fuchsia.BIND_INIT_STEP == fuchsia.gpio.BIND_INIT_STEP.GPIO;
  fuchsia.Service == "fuchsia.hardware.gpio.Service";
}
```

---

## 6. Conditional Branching and Array Acceptance

### 6.1 Array Acceptance (`accept`)

To match against any one of multiple acceptable values, pass an array instead of a single scalar. `dmlc` will generate an `accept <property> { ... }` block in bind rules.

```json5
bind: {
  pci: {
    vid: "fuchsia.pci.BIND_PCI_VID.INTEL",
    did: [ "0x1234", "0x5678" ]
  }
}
```

Generated bind rules:
```bind
fuchsia.BIND_PCI_VID == fuchsia.pci.BIND_PCI_VID.INTEL;
accept fuchsia.BIND_PCI_DID {
  0x1234,
  0x5678,
}
```

Works for `compat`, `vid`, `pid`, `did`, PCI properties, USB properties, ACPI `hid`, and custom rules.

### 6.2 Conditional Branching (`one_of`)

When matching logic requires branching across different buses or device generations, use `one_of`. `dmlc` inspects each branch's trigger conditions and compiles them into `if ... else if ... else ...` statements.

```json5
bind: {
  one_of: [
    {
      pci: {
        vid: "fuchsia.pci.BIND_PCI_VID.INTEL",
        did: "0x1234"
      }
    },
    {
      pci: {
        class: "0x02",
        subclass: "0x00"
      }
    }
  ]
}
```

Generated bind rules:
```bind
if fuchsia.BIND_PCI_VID == fuchsia.pci.BIND_PCI_VID.INTEL {
    fuchsia.BIND_PCI_DID == 0x1234;
} else if fuchsia.BIND_PCI_CLASS == 0x02 {
    fuchsia.BIND_PCI_SUBCLASS == 0x00;
} else {
    false;
}
```

Supported trigger properties in `one_of` include:
* `node_name` / `fuchsia.NAME`
* `acpi.hid` and `acpi.bus_type`
* `compat` (`fuchsia.COMPATIBLE`)
* `protocol` / `banjo` (`fuchsia.BIND_PROTOCOL`)
* `service` (`fuchsia.Service`)
* `vid`, `pid`, `did`
* `pci.vid`, `pci.did`, `pci.class`
* `usb.vid`, `usb.pid`, `usb.class`
* Custom `rules` keys (e.g. `fuchsia.BIND_AUTOBIND`)

### 6.3 Inequality Constraints

To require that a property does *not* equal a value, specify `{ "neq": <value> }` in `rules`:

```json5
bind: {
  rules: {
    "fuchsia.BIND_COMPOSITE": { neq: 1 }
  }
}
```

Generated bind rule:
```bind
fuchsia.BIND_COMPOSITE != 1;
```

---

## 7. Real-World Before / After Examples

### Example 1: Standalone USB Mass Storage Driver

#### Before: Separate Files

`meta/usb_mass_storage.cml`:
```json5
{
  include: [
    "inspect/client.shard.cml",
    "syslog/client.shard.cml",
  ],
  program: {
    runner: "driver",
    binary: "driver/usb_mass_storage.so",
    bind: "meta/bind/usb_mass_storage.bindbc",
    colocate: "true",
    default_dispatcher_opts: [ "allow_sync_calls" ],
  },
  capabilities: [
    { service: "fuchsia.hardware.block.volume.Service" }
  ],
  expose: [
    {
      service: "fuchsia.hardware.block.volume.Service",
      from: "self"
    }
  ]
}
```

`meta/usb_mass_storage.bind`:
```bind
using fuchsia.usb;
using fuchsia.usb.massstorage;

fuchsia.BIND_PROTOCOL == fuchsia.usb.BIND_PROTOCOL.INTERFACE;
fuchsia.BIND_USB_CLASS == fuchsia.usb.BIND_USB_CLASS.MASS_STORAGE;
fuchsia.BIND_USB_SUBCLASS == fuchsia.usb.massstorage.BIND_USB_SUBCLASS.SCSI;
fuchsia.BIND_USB_PROTOCOL == fuchsia.usb.massstorage.BIND_USB_PROTOCOL.BULK_ONLY;
```

#### After: Unified `meta/usb_mass_storage.dml`

```json5
{
  name: "usb-mass-storage",
  program: {
    colocate: "true",
    default_dispatcher_opts: [ "allow_sync_calls" ],
    bind: {
      protocol: "fuchsia.usb.BIND_PROTOCOL.INTERFACE",
      usb: {
        class: "fuchsia.usb.BIND_USB_CLASS.MASS_STORAGE",
        subclass: "fuchsia.usb.massstorage.BIND_USB_SUBCLASS.SCSI",
        protocol: "fuchsia.usb.massstorage.BIND_USB_PROTOCOL.BULK_ONLY"
      }
    }
  },
  capabilities: [
    { service: "fuchsia.hardware.block.volume.Service" }
  ],
  expose: [
    {
      service: "fuchsia.hardware.block.volume.Service",
      from: "self"
    }
  ]
}
```

---

### Example 2: Composite Buttons Driver

#### Before: Separate Files

`meta/buttons.cml`:
```json5
{
  include: [
    "inspect/client.shard.cml",
    "syslog/client.shard.cml",
  ],
  program: {
    runner: "driver",
    binary: "driver/buttons.so",
    bind: "meta/bind/buttons.bindbc",
  },
  use: [
    { service: "fuchsia.hardware.platform.device.Service" },
    { service: "fuchsia.hardware.gpio.Service" },
  ],
  capabilities: [
    { service: "fuchsia.input.report.Service" }
  ],
  expose: [
    {
      service: "fuchsia.input.report.Service",
      from: "self"
    }
  ]
}
```

`meta/buttons.bind`:
```bind
composite buttons;

using fuchsia;
using fuchsia.gpio;

primary parent "pdev" {
  fuchsia.COMPATIBLE == "fuchsia,gpio-buttons";
}

optional parent "gpio-init" {
  fuchsia.BIND_INIT_STEP == fuchsia.gpio.BIND_INIT_STEP.GPIO;
}

optional parent "volume-up" {
  fuchsia.Service == "fuchsia.hardware.gpio.Service";
  fuchsia.NAME == "volume-up";
}
```

#### After: Unified `meta/buttons.dml`

```json5
{
  name: "buttons",
  use: [
    {
      service: "fuchsia.hardware.platform.device.Service",
      name: "pdev",
      primary: true,
      bind: {
        compat: "fuchsia,gpio-buttons"
      }
    },
    {
      service: "fuchsia.gpio.Init",
      name: "gpio-init",
      availability: "optional"
    },
    {
      service: "fuchsia.hardware.gpio.Service",
      name: "volume-up",
      availability: "optional",
      bind: {
        match_name: true
      }
    }
  ],
  capabilities: [
    { service: "fuchsia.input.report.Service" }
  ],
  expose: [
    {
      service: "fuchsia.input.report.Service",
      from: "self"
    }
  ]
}
```

Notice that:
* `pdev` is explicitly the primary parent.
* `fuchsia.gpio.Init` is converted into an init step bind rule and automatically filtered out of the generated CML.
* `volume-up` with `match_name: true` automatically matches `fuchsia.NAME == "volume-up"`.

---

[dfv2]: /docs/development/drivers/dfv2-overview.md
