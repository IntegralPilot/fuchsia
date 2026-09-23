---
name: driver-bind-find-instance-name
description: >
  Find the instance/parent node name to pass when opening a FIDL connection to a
  driver's parent capability, by reading the .bind rules. Non-composite drivers
  use "default"; composite drivers use the named parent (e.g. "pdev"). Use when
  a driver needs the right instance name for a Connect call, or when a parent
  connection fails because the instance name doesn't match the composite bind
  rule node name. Don't use for diagnosing overall bind failures (see
  debugging-driver-binding).
---

# Finding Instance Names from Bind Rules

## Steps

### 1. Open Bind Rules File

Open the driver's bind rules file (usually with a `.bind` extension).

### 2. Determine Instance Name

The method for finding the instance name depends on whether your driver is a
composite driver:

* **Non-Composite Drivers**: The instance name is implicitly `"default"`. You do
  not need to find a name in the bind rules.
* **Composite Drivers**:
  1.  **Look for Parent Definitions**: Look for the `parent` definitions in the
      bind rules.
  2.  **Identify Parent Name**: Identify the name assigned to the parent you
      need to connect to.

### Example

For a composite driver, if your bind rules file contains:
```bind
primary parent "pdev" {
  fuchsia.BIND_PROTOCOL == fuchsia.platform.BIND_PROTOCOL.DEVICE;
}
```
The name assigned to this parent is `"pdev"`.

Non-composite drivers do not have a "parent" property in their bind rules.

To see how to use this instance name to connect to the parent capability in your
driver code, see the corresponding implementation skills:

* [Using FIDL in
  C++](/src/devices/skills/driver_fidl/client/implementation/cpp/SKILL.md)
* [Using FIDL in
  Rust](/src/devices/skills/driver_fidl/client/implementation/rust/SKILL.md)

## Common Pitfalls

* **Assuming "default" in Composite Drivers**:
  * In composite drivers, you must use the specific node name defined in the
    bind rules, not `"default"`.
* **Mismatched Names**:
  * Ensure the instance name used in `Connect` exactly matches the parent node
    name in the `.bind` file (including case).

## Further Reading

* [Debugging Driver Binding](/src/devices/skills/debug_driver_binding/SKILL.md)
  - Workflow for determining why a driver failed to bind or find its parent
    capabilities.
