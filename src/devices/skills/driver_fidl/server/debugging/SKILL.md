---
name: driver-fidl-server-debugging
description: >
  Debug a DFv2 driver that fails to advertise a FIDL service from its outgoing
  namespace (provider/server side). Use when a service is missing from
  /out/svc, outgoing() AddService / serve_outgoing seems to fail, or clients
  cannot connect -- diagnose with ffx component explore and ffx component
  doctor, checking the capabilities and expose declarations, dispatcher
  liveness, and instance-name mismatches. For the consumer/incoming-namespace
  side, use the client FIDL debugging skill instead.
---

# Driver FIDL Server Debugging (DFv2)

## Verification Steps

### 1. Verify Service in Outgoing Namespace

To check if a driver is successfully advertising a service, you can explore its
outgoing namespace.

Run the following command to enter the component's sandbox:
```bash
ffx component explore <provider-driver-moniker>
```

Once inside the shell, check the `/out/svc` directory for the fully qualified
service name:
```bash
ls /out/svc
```
* **Expected**: You should see a service name matching your FIDL service (e.g.,
  `fuchsia.hardware.example.Service`).

If the service is not listed, check that the driver successfully called
[`outgoing()->AddService()`](/src/devices/skills/driver_fidl/server/implementation/cpp/SKILL.md)
(C++) or
[`context.serve_outgoing`](/src/devices/skills/driver_fidl/server/implementation/rust/SKILL.md)
(Rust) and that the call did not return an error.

### 2. Diagnose Routing with `ffx component doctor`

To check if the service exposed by your driver is correctly routed and available
to other components, use `ffx component doctor`:

```bash
ffx component doctor <provider-driver-moniker>
```
This command will check all `expose` declarations and report any routing
failures or broken capabilities.

## Common Issues and Troubleshooting

### Service not visible in `/out/svc`

* **Forgetting to serve outgoing directory**:
  * In C++, ensure you call `outgoing()->AddService(...)` and that it returns
    `zx::ok()`.
  * In Rust, ensure you call `context.serve_outgoing(&mut fs)` and spawn the
    collect task: `scope.spawn(fs.collect());`.
* **Dispatcher issues**:
  * Ensure the handler passed to `AddService` uses a valid and running
    dispatcher. If the dispatcher is shut down or not processing events, the
    service will not appear or respond.

### Clients cannot connect despite service being visible

* **Missing `capabilities` in Manifest**:
  * The service must be declared in the `capabilities` section before it can be
    exposed:
    ```json5
    capabilities: [
        {
            service: "fuchsia.hardware.example.Service",
        },
    ],
    ```
* **Missing `expose` in Manifest**:
  * Even if the service is in `/out/svc`, other components cannot see it unless
    the provider's `.cml` has an `expose` entry:
    ```json5
    expose: [
        {
            service: "fuchsia.hardware.example.Service",
            from: "self",
        },
    ],
    ```
* **Mismatched instance names**:
  * If the driver adds the service with a specific instance name (e.g.
    `AddService(..., "my-instance")`), the client must connect to that specific
    instance.

## Further Reading

* [Serving FIDL in
  C++](/src/devices/skills/driver_fidl/server/implementation/cpp/SKILL.md)
* [Serving FIDL in
  Rust](/src/devices/skills/driver_fidl/server/implementation/rust/SKILL.md)
* [Client FIDL
  Debugging](/src/devices/skills/driver_fidl/client/debugging/SKILL.md)
