---
name: driver-version-identification
description: >
  Identify whether a Fuchsia driver is DFv1 (legacy/compat) or DFv2. Distinguishes
  via the .cml program block (compat vs binary key), C++ headers/base class
  (ddk::Device vs fdf::DriverBase2), entry point (bind/create vs Start), logging
  (zxlogf vs FDF_LOG), build deps, and runtime `ffx component show` / `ffx
  driver show`. Use before migrating, debugging, or modifying a driver when its
  framework version is unclear. Note: all Rust drivers are DFv2.
---

# Identify Driver Version (DFv1 vs DFv2)

## 1. Check Component Manifest (.cml)

The most reliable indicator is the driver's component manifest file. Both DFv2
and DFv1 (running via compat) drivers use `runner: "driver"`, but they differ in
how they specify the driver library:

* **DFv2 Driver**: Uses the `binary` key.
  ```json5
  program: {
      runner: 'driver',
      binary: 'driver/my_driver.so',
      bind: 'meta/bind/my_driver.bindbc',
  },
  ```
* **DFv1 Driver (Compat Mode)**: Uses the `compat` key and typically includes
  the compat shard.
  ```json5
  include: [
      "//sdk/lib/driver/compat/compat.shard.cml",
  ],
  program: {
      runner: 'driver',
      compat: 'driver/my_driver.so',
      bind: 'meta/bind/my_driver.bindbc',
  },
  ```

## 2. Inspect Source Code

### **If** the driver is written in C++:

* **Headers**:
  * **DFv2**: Includes `<lib/driver/component/cpp/driver_base2.h>`.
  * **DFv1**: Includes `<lib/ddk/device.h>` or `<lib/ddk/driver.h>`.
* **Driver Class Inheritance**:
  * **DFv2**: Inherits from `fdf::DriverBase2` and uses
    `FUCHSIA_DRIVER_EXPORT2(MyDriver)`.
  * **DFv1**: Inherits from `ddk::Device` (or uses raw hooks) and uses the
    `ZIRCON_DRIVER(...)` macro.
* **Entry Point**:
  * **DFv2**: Implements `Start()` returning `zx::result<>`.
  * **DFv1**: Implements `bind()` or `create()` returning `zx_status_t`.
* **Logging**:
  * **DFv2**: Uses `FDF_LOG(...)` or modern `fdf::info`, `fdf::error`, etc.
  * **DFv1**: Uses `zxlogf(LEVEL, ...)`.

### **If** the driver is written in Rust:

* All drivers written in Rust are DFv2 drivers.

## 3. Check Build Files

* **Look for (GN)**: Dependencies on `//sdk/lib/driver/component/cpp` or
  `//sdk/lib/driver/component/rust`.
* **Look for (Bazel)**: Dependencies on `@fuchsia_sdk//pkg/driver_component_cpp`
  or `@fuchsia_sdk//pkg/fdf_component`.
* **Look for**: The use of templates that package the driver as a component with
  a `.cml` file.

## 4. Identify at Runtime

If the driver is already running on a device, use host tools to identify its
type:

* **Using `ffx component show`**: Run `ffx component show
  <driver_component_name>`. Look at the `program` block:
  * **DFv2 Driver**: Shows the `binary` key (e.g., `binary:
    'driver/my_driver.so'`).
  * **DFv1 Driver (Compat)**: Shows the `compat` key (e.g., `compat:
    'driver/my_driver.so'`).
* **Using `ffx driver show`**: Run `ffx driver show <driver_url>`. This command
  displays details about the driver. The presence of `compat` in the manifest
  details or arguments indicates a wrapped DFv1 driver.
* **Log Analysis**: DFv1 drivers running in the shim may produce logs from the
  compat runner or still use legacy `zxlogf` style output.

## Summary of Differences

| Feature | DFv1 (Legacy/Compat) | DFv2 (Modern) |
| :--- | :--- | :--- |
| **Manifest** | `.cml` with `compat` key | `.cml` with `binary` key |
| **Headers** | `<lib/ddk/device.h>` | `<lib/driver/component/cpp/driver_base2.h>` |
| **C++ Base** | `ddk::Device` or raw hooks | `fdf::DriverBase2` |
| **Logging** | `zxlogf` | `FDF_LOG` or `fdf::info/error` |
| **Entry Point** | `bind()` or `create()` | `Start()` |
| **Rust Support** | None | Full support via `fdf_component` |

## Common Pitfalls

* **Assuming `.cml` means DFv2**: Both DFv2 and DFv1 (compat) drivers use `.cml`
  files. Check the `program` block for `binary` vs `compat` to distinguish them.
* **Confusing `runner: "driver"`**: Both versions use this runner. It does not
  distinguish between them.
* **Assuming legacy logging means DFv1**: Transitional drivers might still use
  `zxlogf` while being otherwise migrated or using DFv2 features. Check headers
  and inheritance for a definitive answer.

## Further Reading

* For guidance on migrating a driver from DFv1 to DFv2, see the [Migrating DFv1
  to DFv2](/src/devices/skills/migrate_dfv1_to_dfv2/SKILL.md) skill.
