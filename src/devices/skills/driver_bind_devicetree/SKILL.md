---
name: driver-bind-devicetree
description: >
  Write the .bind rules for a Fuchsia driver that binds to a devicetree node
  via fuchsia.COMPATIBLE matching the node's compatible string. Covers
  platform-device (pdev) bind properties for composite nodes. Use when
  authoring a driver for a devicetree-published device, or when binding fails
  on a compatible-string mismatch. Don't use for diagnosing why an existing
  driver won't bind (see debugging-driver-binding).
---

# Driver Devicetree Binding

## Write the Bind Rules

### 1. Define the Compatibility Bind Property
A driver designed to bind to a devicetree node matches the node's `compatible`
property.

Assert that the `fuchsia.COMPATIBLE` property matches your device's compatible
string.

```bind
fuchsia.COMPATIBLE == "foo,bar";
```

### 2. Handle Resource Bindings (Optional)
If the devicetree node is published as a composite platform device because it
contains interrupts, MMIOs, or other parent-provided dependencies, you should
include the standard platform node definitions to ensure it binds inside the
correct platform bus topology:

```bind
using fuchsia.platform;

fuchsia.BIND_PROTOCOL == fuchsia.platform.BIND_PROTOCOL.DEVICE;
```

## Connect to Platform Resources

Once the driver binds to the devicetree platform device, retrieve resources
(like MMIO ranges and interrupts) via Platform Device helper libraries.

To connect to the platform device in a C++ driver, see the [Using Platform
Device (pdev)](/src/devices/skills/driver_pdev/implementation/cpp/SKILL.md)
skill.

## Common Pitfalls

* **Mismatched Compatible Strings**: The `fuchsia.COMPATIBLE` string in your
  `.bind` rules must exactly match the string in the node's `compatible` list in
  the `.dts` file, including vendor prefixes and casing.
* **Forgetting `fuchsia.COMPATIBLE` on Raw Nodes**: Nodes without a `compatible`
  property in devicetree will not expose a `fuchsia.COMPATIBLE` bind property.
  These will fallback to generic platform device properties. Always include a
  compatible property for peripheral drivers.

## Further Reading

* [Devicetree Debugging](/src/devices/skills/devicetree_debugging/SKILL.md)
  - Guide for diagnosing golden mismatches and bind failures.
* [Devicetree Visitor Creation](/src/devices/skills/devicetree_visitor/SKILL.md)
  - Guide for parsing devicetree nodes into custom driver metadata.
* [Driver File Structure](/src/devices/skills/driver_file_structure/SKILL.md)
  - Standard Fuchsia driver component directory layout, file naming, and build
    target conventions.
