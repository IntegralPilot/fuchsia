---
name: migrate-dfv1-to-dfv2
description: >
  Migrate a C++ driver from the legacy Driver Framework v1 (DFv1/DDK) to
  Driver Framework v2 (DFv2). Use when a driver still inherits from
  ddk::Device, uses ZIRCON_DRIVER, device_get_protocol,
  DdkInit/DdkBind/DdkUnbind/DdkSuspend, or links //src/lib/ddk and
  //src/lib/ddktl, and must move to fdf::DriverBase2, Start(DriverContext),
  FUCHSIA_DRIVER_EXPORT2, and a native driver .cml. Covers headers,
  BUILD.gn/Bazel deps, Banjo-to-FIDL, logging, pdev/MMIO, and DMA hand-offs.
  Stays in C++ -- don't use for a C++-to-Rust rewrite (migrate-cpp-to-rust-
  driver). For a driver already on the older fdf::DriverBase, use migrate-
  driver-base-to-driver-base2 instead.
---

# Driver Migration (DFv1 to DFv2)

## Mandatory Workflow Checklist

Before declaring a migration complete, you MUST complete all steps in this
checklist:
- [ ] Code changes implemented (DriverBase, Start, FIDL, etc.)
- [ ] Build dependencies updated in `BUILD.gn`
- [ ] Component manifest updated (`.cml`)
- [ ] Driver added to build graph (e.g., `build_only_labels` or `fx set`)
- [ ] **Run `fx format-code`** to ensure code style compliance.
- [ ] `fx build` completes successfully.
- [ ] Tests pass (if applicable).
- [ ] Commit changes after logical chunks of work.

## Identify DFv1 Drivers

Before starting a migration, confirm that the driver is indeed a DFv1 driver.

For a comprehensive guide on distinguishing DFv1 from DFv2 drivers (including
codebase indicators and runtime checks), see the [Driver Version Identification
Skill](/src/devices/skills/driver_version_identification/SKILL.md).

## Update Build Dependencies

In the driver's build file, remove dependencies on `//src/lib/ddk` and
`//src/lib/ddktl`, and add the DFv2 driver component library.

**GN:**
```diff
 deps = [
-  "//src/lib/ddk",
-  "//src/lib/ddktl",
+  "//sdk/lib/driver/component/cpp",
 ]
```

**Bazel:**
```diff
 deps = [
-  "//src/lib/ddk",
-  "//src/lib/ddktl",
+  "@fuchsia_sdk//pkg/driver_component_cpp",
 ]
```

## Update Source Code (C++)

### A. Update Headers

Replace DDK headers with DFv2 headers:

```diff
-#include <ddktl/device.h>
+#include <lib/driver/component/cpp/driver_base2.h>
+#include <lib/driver/component/cpp/driver_export2.h>
```

### B. Update Driver Class

Migrate the driver class to inherit from `fdf::DriverBase2`. Here is the diff
showing the transformation:

```diff
-class MyDriver : public ddk::Device<MyDriver, ...> {
- public:
-  MyDriver(zx_device_t* parent);
+class MyDriver : public fdf::DriverBase2 {
+ public:
+  MyDriver() : fdf::DriverBase2("my-driver") {}
+
+  zx::result<> Start(fdf::DriverContext context) override;
 };
```

### C. Implement Start Method

In DFv1, initialization happens in the constructor, `Bind` static method, and
`DdkInit`. In DFv2, all this logic should be moved to the
`Start(fdf::DriverContext context)` method.

* Return `zx::ok()` on success.
* Use `context.incoming()` to access incoming services (replacing
  `device_get_protocol`). For details on how to connect to parent FIDL
  connections in C++, see the [Driver FIDL Usage Implementation Skill
  (C++)](/src/devices/skills/driver_fidl/client/implementation/cpp/SKILL.md).

### D. Update Macros

Replace `ZIRCON_DRIVER` with `FUCHSIA_DRIVER_EXPORT2`. Here is the diff showing
the change:

```diff
-ZIRCON_DRIVER(my_driver, driver_ops, "zircon", "0.1");
+FUCHSIA_DRIVER_EXPORT2(MyDriver);
```
(Note: `FUCHSIA_DRIVER_EXPORT2` is usually placed in the `.cc` file).

### E. Update Logging

Migrate `zxlogf` to `fdf::info`, `fdf::error`, etc. Refer to the
[migrate_logging Skill](/src/devices/skills/migrate_logging/SKILL.md) for
detailed instructions on logging migration, including `std::format` syntax.

### F. Connect to Banjo or FIDL

If the driver uses `device_get_protocol` to get a Banjo protocol, use
`compat::ConnectBanjo` or migrate to FIDL if available.

Example for Banjo:
```cpp
zx::result<ddk::MiscProtocolClient> client =
     compat::ConnectBanjo<ddk::MiscProtocolClient>(incoming());
```

### G. Migrate DMA Buffers

If the driver uses `ddk::IoBuffer` for DMA operations, migrate it to the
`dma-buffer` library.

For detailed steps on migrating DMA buffers, see the [DMA Migration
Skill](/src/devices/skills/migrate_dfv1_to_dfv2/dma/SKILL.md).

### H. Stop and Suspend the Driver

In DFv1, drivers handle teardown in `DdkUnbind` and power state changes in
`DdkSuspend`. In DFv2 (`DriverBase2`), teardown is handled by the framework
invoking `Stop(StopCompleter)`. Power state transitions are coordinated through
the Power Broker or specific FIDL protocols.

#### Unbind

#### **If** the driver implements `DdkUnbind` solely to call `txn.Reply()`:

This hook can be removed completely in DFv2. The framework handles destruction
automatically.

#### **Otherwise** (If the driver performs cleanup in `DdkUnbind`):

Move that logic to the class destructor for synchronous cleanup, or override the
`Stop(StopCompleter)` method in `fdf::DriverBase2` for asynchronous cleanup.

#### Suspend

* In DFv2, suspend and power management are handled differently (often via the
  Power Broker or specific FIDL protocols). That said, any code that was in
  `DdkSuspend` or hardware power-down logic should be moved into
  `Stop(StopCompleter)`.

### I. Use Platform Device (pdev)

Many drivers need to connect to a Platform Device to access MMIOs, interrupts,
or BTIs. In DFv2, you use the `fdf::PDev` helper class.

For details on how to use `fdf::PDev` to acquire resources, see the [PDev Usage
Skill](/src/devices/skills/driver_pdev/implementation/cpp/SKILL.md).

## Update Component Manifest (.cml)

Migrating to DFv2 requires creating or updating the driver's component manifest
file (`.cml`). A native DFv2 driver uses `runner: "driver"` and the `binary` key
in the `program` block instead of the `compat` key.

If the driver was previously running in compatibility mode, you must **remove**
the include for `"//sdk/lib/driver/compat/compat.shard.cml"`, replace it with
`"driver_component/driver.shard.cml"`, and change the `compat` key to `binary`.

Here is a diff showing the transformation:

```diff
 {
     include: [
-        "//sdk/lib/driver/compat/compat.shard.cml",
+        "driver_component/driver.shard.cml",
         "inspect/client.shard.cml",
         "syslog/client.shard.cml",
     ],
     program: {
         runner: "driver",
-        compat: "driver/my_driver.so",
+        binary: "driver/my_driver.so",
         bind: "meta/bind/my_driver.bindbc",
     },
 }
```

## Verify the Migration

After migration, you must verify that the driver builds and passes tests:

### Include in Build Graph
Many drivers may not be in the default build graph. To ensure the driver source
code is compiled during `fx build`:
1.  Add the driver target (e.g.,
    `//src/devices/rtc/drivers/pl031-rtc:pl031-rtc`) to the build graph.
2.  You can use `fx set ... --with //path/to/driver:target` or manually add it
    to `build_only_labels` in `out/default/args.gn` to force compilation.

### Build Driver
Run `fx build` to ensure the driver compiles successfully.

### Test Driver

Migrating tests from DFv1 to DFv2 can be challenging. If tests exist, update
them to use DFv2 testing libraries (e.g., `sdk/lib/driver/testing/cpp`) and run
them with `fx test`.

A helpful guide for DFv2 unit testing can be found at [Driver Unit Testing Quick
Start](/docs/development/sdk/driver-testing/driver-unit-testing-quick-start.md).

## 6. Clean Up and Commit

### Clean Up Code
Before committing, run `fx format-code` to ensure the migrated code adheres to
Fuchsia style guidelines.

### Commit Changes
Make a local commit after a logical chunk of work. You do not necessarily need a
commit for every individual driver if you are migrating multiple similar
drivers, but each commit should represent a coherent state. Follow the Git
commit message guidelines in `GEMINI.md` or the project style guide.

## Common Pitfalls

* **Non-Default Constructible Members**: Members like `fdf::MmioBuffer` do not
  have default constructors. If you need to initialize them in `Start()` rather
  than the constructor, wrap them in `std::optional`.
* **Missing `fdf_` Symbols at Link Time**: If the linker fails with undefined
  symbols like `fdf_arena_create`, add `//src/devices/lib/driver:driver_runtime`
  to `deps` in `BUILD.gn`.
* **Ambient DDK Dependencies**: If the driver uses a library that has not been
  migrated to DFv2 and still includes DDK headers (e.g., `<lib/ddk/driver.h>`),
  you may need to keep `//src/lib/ddk` in `deps` temporarily.
* **Dispatcher Usage**: When passing a dispatcher to FIDL bindings,
  `dispatcher()` (which returns `fdf::UnownedSynchronizedDispatcher`) can often
  be used directly or converted as needed.

## Further Reading

* [Overview](/docs/development/drivers/migration/migrate-from-dfv1-to-dfv2/overview.md)
* [Update DDK
  Interfaces](/docs/development/drivers/migration/migrate-from-dfv1-to-dfv2/update-ddk-interfaces-to-dfv2.md)
* [FAQ](/docs/development/drivers/migration/migrate-from-dfv1-to-dfv2/faq.md)
* [Update Other
  Services](/docs/development/drivers/migration/migrate-from-dfv1-to-dfv2/update-other-services-to-dfv2.md)
* [Driver Unit Testing Quick
  Start](/docs/development/sdk/driver-testing/driver-unit-testing-quick-start.md)
