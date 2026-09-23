---
name: debugging-driver-binding
description: >
  Debugs why a Fuchsia driver fails to bind to a node. Identifies if the
  driver is in the build, analyzes bind rules (regular vs composite), finds
  missing parent drivers (FIDL, legacy protocols, platform bus), and falls
  back to serial if needed. Use when a driver is not visible in `ffx driver
  list`, when `ffx driver doctor` fails, or when a device topology node is
  missing a parent.
---

# Debugging Driver Binding

Follow this workflow to determine why a driver failed to bind.

## Phase 1: Verify Driver Inclusion

* Run `ffx driver list` (or `driver list` over serial).
* **Check**: If the driver package is listed, it's on the device.
* **If missing**: Report to user. Do not proceed until included in build.

---

## Phase 2: Determine Driver Type

Check the `.bind` file (usually in `meta/`) or run `ffx driver show
{driver_url}`.

* **Composite**: Starts with `composite` or specifies multiple parents/nodes.
* **Regular (Non-Composite)**: Top-level rules only.

---

## Phase 3: Debug Regular Drivers

Goal: Find a node in the topology where properties match bind rules.

### Case A: Node Matches Bind Rules (Startup Failure)
The error is likely in driver startup.

* Check logs: `ffx log` (or `log` over serial) for warnings/errors.

### Case B: No Node Matches (Missing or Misaligned)
The node was either not added or properties don't align.

1.  Run `ffx driver doctor --driver {driver_url}` (finds misalignments).
2.  Investigate the **parent driver** (see below).

#### Finding the Parent Driver

Identify how the node is added by checking the code. Try these three common
patterns:

##### 1. FIDL Capability (Service)
Check if bind rules specify a Service capability (e.g. `fuchsia.Service ==
"fuchsia.hardware.display.engine.Service"`).

* Search `.cml` for the service offer (`capabilities` + `expose`).
* Verify parent code creates and provides the offer.
* Check if the parent is bound (recurse).

##### 2. Legacy Protocol (`fuchsia.BIND_PROTOCOL`)
Check if bind rules specify `fuchsia.BIND_PROTOCOL`.

* Find the `uint` value in `.bind` (search for `extend uint
  fuchsia.BIND_PROTOCOL`).
* Map to `src/lib/ddk/include/lib/ddk/protodefs.h` (e.g. `0x43` = 67 ->
  `ZX_PROTOCOL_CAMERA_SENSOR2`).
* Check how added:
    * **DFv1**: `proto_id` in `device_add_args`, or `ddk::base_protocol` mixin.
    * **DFv2**: Manual bind rules using `bind_fuchsia::PROTOCOL` constants.
* Check if parent is bound (recurse).

##### 3. Platform Device (Platform Bus)
Added by platform bus driver.

* Check PID/VID/DID, or `fuchsia.COMPATIBLE` for devicetree devices.
* For devicetree, find the visitor (inheritor of `fdf_devicetree::Visitor`).

### No Criteria Matches
If you cannot locate the node or parent:

* Inform the user of the properties the driver needs.
* Ask the user to locate the parent driver.
* If provided, recurse using this workflow for that parent.

---

## Phase 4: Debug Composite Drivers

Composite drivers bind to a completed composite node parented by multiple nodes
(managed by a composite parent spec / node group).

1.  Run `ffx driver doctor --driver {driver_url}`.
2.  If **No Matched Specs**: The board driver didn't add the spec or
    misconfigured parents.
3.  If Unbound parents exist: Run command from output (e.g. `ffx driver doctor
    --composite-node-spec {spec_name}`).
4.  Find missing parents' bind rules using `ffx driver composite show
    {spec_name}`.
5.  Recurse using the **Regular Driver** missing node workflow (skipping build
    checks for the missing node).

---

## Helpful Commands

* `ffx driver composite list -v`: List specs and matches.
* `ffx driver node list`: List device topology nodes.
* `ffx driver node show {node_name}`: Show node properties.

## Serial Fallback

If `ffx` fails or times out, use `run_on_serial` skill.
* Drop `ffx` prefix (e.g., `ffx driver list` -> `driver list`).

## References

* `docs/contribute/governance/rfcs/0197_node_groups.md`
* `docs/development/drivers/developer_guide/create-a-composite-node.md`
