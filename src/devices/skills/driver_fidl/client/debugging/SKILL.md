---
name: driver-fidl-debugging
description: >
  Debug a DFv2 driver that fails to connect to a FIDL protocol or service from
  its incoming namespace (client/consumer side). Use when a Connect() call
  returns ZX_ERR_NOT_FOUND, a capability is missing from /ns/svc, or routing
  fails -- diagnose with ffx component explore and ffx component doctor,
  checking use declarations, protocol-vs-service mismatch, and named-instance
  mismatches. For the provider/outgoing-namespace side, use the server FIDL
  debugging skill instead.
---

# Driver FIDL Usage Debugging (DFv2)

## Verification Steps

### 1. Verify the Service in the Receiver's Namespace

The most direct way to see if a capability is reaching the receiver driver is to
explore its incoming namespace.

Run the following command to enter the component's sandbox:
```bash
ffx component explore <receiver-driver-moniker>
```

Once inside the shell, check the `/ns/svc` directory for the fully qualified
FIDL protocol or service name:
```bash
ls /ns/svc
```
* **Expected**: You should see a service or protocol name matching your FIDL
  type (e.g., `fuchsia.hardware.example.MyProtocol`).

If the capability is not listed, proceed to the troubleshooting section below.

### 2. Diagnose Routing with `ffx component doctor`

To check if all capabilities used by your driver are correctly routed and
available, use `ffx component doctor`:

```bash
ffx component doctor <receiver-driver-moniker>
```
This command will check all `use` declarations for the component and report any
routing failures. It is a quick way to identify missing capabilities without
manually tracing manifests.

## Common Issues and Troubleshooting

### Connection fails with `ZX_ERR_NOT_FOUND`

* **Missing Capability Routing in Manifest**:
  * Check that the parent driver (or component exposing the capability) has an
    `expose` entry in its `.cml`.
  * Check that the receiver driver has a `use` entry in its `.cml` for the
    protocol or service.
* **Instance Name Mismatch**:
  * If the parent exposes a named instance (e.g., via composite bind rules
    resulting in an instance name like "pdev"), the receiver must specify that
    instance name when connecting.
  * If the receiver attempts to connect to the "default" instance but the parent
    offered it under a specific name, the connection will fail with
    `ZX_ERR_NOT_FOUND`.
  * To learn how to find the correct instance name by looking at bind rules, see
    the [Finding Instance Names from Bind
    Rules](/src/devices/skills/driver_bind_find_instance_name/SKILL.md) skill.
* **Protocol vs Service Confusion in Manifest**:
  * Ensure you are using the correct keyword in your `.cml`. If the capability
    is a FIDL `service`, you must use `service: "..."`. If it is a direct
    `protocol`, you must use `protocol: "..."`. Mixing these up will cause
    runtime routing failures.

## Further Reading
* [Using FIDL in
  C++](/src/devices/skills/driver_fidl/client/implementation/cpp/SKILL.md)
* [Using FIDL in
  Rust](/src/devices/skills/driver_fidl/client/implementation/rust/SKILL.md)
* [Server FIDL
  Debugging](/src/devices/skills/driver_fidl/server/debugging/SKILL.md)
