# Troubleshoot common issues in DFv2 driver development

This troubleshooting guide provides debugging workflows for resolving
some common errors in Fuchsia driver development.

* [Debugging workflows](#debugging-workflows) - A list of debugging workflows
  for diagnosing and fixing known issues.
* [Error messages](#error-messages) - A list of common error messages
  related to driver development and how to fix them.

## Debugging workflows {:#debugging-workflows}

Debugging workflows for common issues:

- [Driver fails to bring up](#driver-fails-to-bring-up)
- [Driver fails to communicate with a FIDL service (PEER_CLOSED error)](#driver-fails-to-communicate-with-a-fidl-service)
- [Capability routing error](#capability-routing-error)

### Driver fails to bring up {:#driver-fails-to-bring-up}

A driver fails to "bring up" if it fails to load, bind, or start as
expected.

To debug this issue, try the following steps:

1. [Verify that the driver is loaded successfully](#verify-that-the-driver-is-loaded-successfully).
2. [Verify that the driver attempted to bind](#verify-that-the-driver-attempted-to-bind).
3. [Verify that the driver is started successfully](#verify-that-the-driver-is-started-successfully).

#### 1. Verify that the driver is loaded successfully {:#verify-that-the-driver-is-loaded-successfully}

If the driver is loaded successfully, you see a message like below in the logs:

```none {:.devsite-disable-click-to-copy}
Found boot driver: fuchsia-boot:///#meta/ahci.cm
```

Or you can dump the list of all drivers using the following command:

```posix-terminal
ffx driver list
```

This command prints output similar to the following:

```none {:.devsite-disable-click-to-copy}
fuchsia-boot:///adc#meta/adc.cm
fuchsia-boot:///ahci#meta/ahci.cm
fuchsia-boot:///alc5514#meta/alc5514.cm
fuchsia-boot:///alc5663#meta/alc5663.cm
```

If the driver is missing from the list, then it must have failed to load.

Here are some reasons to consider:

- The driver is not included in the build packages.
- There are issues with the driver's component manifest (`.cml`) file.

Common error messages related to component manifest files:

- [`Could not load driver: <...>: Failed to open bind file`](#could-not-load-driver-failed-to-open-bind-file)
- [`Could not load driver: <...>: Missing bind path`](#could-not-load-driver-failed-to-open-bind-file)
- [`Failed to start driver, missing 'binary' argument: ZX_ERR_NOT_FOUND`](#could-not-start-driver-missing-binary)

#### 2. Verify that the driver attempted to bind {:#verify-that-the-driver-attempted-to-bind}

If the driver attempted to bind to a node, you see a message like below in the
logs:

```none {:.devsite-disable-click-to-copy}
Binding driver fuchsia-boot:///#meta/focaltech.cm
```

If you don't see this log message, there may be a mismatch between the driver's
bind rules and the node properties. To debug a mismatch, examine the driver's
bind rules and compare them to the node properties.

To view all node properties, run the following command:

```posix-terminal
ffx driver list-devices -v
```

This command prints output similar to the following:

```none {:.devsite-disable-click-to-copy}
Name     : pt
Moniker  : acpi._SB_.PCI0.ISA_.RTC_.pt
Driver   : unbound
5 Properties
[ 1/  5] : Key fuchsia.BIND_ACPI_ID           Value 0x000004
[ 2/  5] : Key "fuchsia.acpi.HID"             Value "PNP0B00"
[ 3/  5] : Key fuchsia.BIND_PROTOCOL          Value 0x00001e
[ 4/  5] : Key "fuchsia.Service"              Value "fuchsia.driver.compat.Service"
[ 5/  5] : Key "fuchsia.Service"              Value "fuchsia.hardware.acpi.Service"
2 Offers
Service: fuchsia.driver.compat.Service
  Source: board
  Instances: default
Service: fuchsia.hardware.acpi.Service
  Source: board
  Instances: default
```

**However, if the driver is a composite driver**, you need to verify
that the [composite node spec][composite-node-spec] matches the
composite driver's bind rules and the node properties from the parent
nodes.

To view the composite node spec, run the following command:

```posix-terminal
ffx driver list-composite-node-specs -v
```

This command prints output similar to the following:

```none {:.devsite-disable-click-to-copy}
Name      : COM1-composite-spec
Driver    : fuchsia-boot:///uart16550#meta/uart16550.cm
Nodes     : 3
Node 0    : "acpi" (Primary)
  2 Bind Rules
  [ 1/ 2] : Accept "fuchsia.BIND_PROTOCOL" { 0x00001e }
  [ 2/ 2] : Accept fuchsia.BIND_ACPI_ID { 0x000007 }
  3 Properties
  [ 1/ 3] : Key "fuchsia.BIND_PROTOCOL"        Value 0x00001e
  [ 2/ 3] : Key fuchsia.BIND_ACPI_ID           Value 0x000007
  [ 3/ 3] : Key "fuchsia.acpi.HID"             Value "PNP0501"
Node 1    : "sysmem"
  1 Bind Rules
  [ 1/ 1] : Accept "fuchsia.Service" { "fuchsia.hardware.sysmem.Service" }
  1 Properties
  [ 1/ 1] : Key "fuchsia.Service"              Value "fuchsia.hardware.sysmem.Service"
Node 2    : "irq000"
  3 Bind Rules
  [ 1/ 3] : Accept "fuchsia.BIND_ACPI_ID" { 0x000007 }
  [ 2/ 3] : Accept "fuchsia.BIND_PLATFORM_DEV_INTERRUPT_ID" { 0x000001 }
  [ 3/ 3] : Accept "fuchsia.Service" { "fuchsia.hardware.interrupt.Service" }
  3 Properties
  [ 1/ 3] : Key "fuchsia.BIND_ACPI_ID"         Value 0x000007
  [ 2/ 3] : Key "fuchsia.BIND_PLATFORM_DEV_INTERRUPT_ID" Value 0x000001
  [ 3/ 3] : Key "fuchsia.Service"              Value "fuchsia.hardware.interrupt.Service"
```

The composite driver is not matched to the spec, the `Driver` field will say
`None`.

If there exist inconsistencies between the bind rules and node properties, you
need to adjust them until they match.

Additionally, if the bind rules use a FIDL-based node property, you need to add
an offer to the child so that the FIDL-based property is included.

For example, let's say the bind rules specify the following condition:

```none {:.devsite-disable-click-to-copy}
fuchsia.Service == "fuchsia.examples.gizmo.Service";
```

Then the parent driver of the target node need to add the following offer to
the node:

```none {:.devsite-disable-click-to-copy}
zx::result child_result =
    AddChild("zircon_transport_child", {}, {fdf::MakeOffer2<fuchsia_examples_gizmo::Service>()});
```

#### 3. Verify that the driver is started successfully {:#verify-that-the-driver-is-started-successfully}

If the driver is loaded successfully, you see a message like below in the logs:

```none {:.devsite-disable-click-to-copy}
Started driver url=fuchsia-boot:///ramdisk#meta/ramdisk.cm
```

But if this log message is missing, then the driver must have failed to start.

To debug this issue, first check if the driver's `Start()` function is called.
For this task, it's recommended to update the `Start()`function to print some
log messages.

If the `Start()` function is never even called, here are some possible reasons:

- [`__fuchsia_driver_registration__ symbol not available`](#fuchsia-driver-registration-symbol-not-available)
- [`Driver-Loader: libdriver.so: Not in allowlist`](#driver-loader-libdriver-so-not-in-allowlist)

However, if the `Start()` function is called but the driver still fails to
start, it may fail in one of the following ways:

- The driver's `Start()` function replies or returns an error.
- The driver overrides `void Start(StartCompleter completer)` and does not
  reply to the completer.

If the driver's `Start()` function replies or returns an error, you see
messages similar to the following in the logs:

```none {:.devsite-disable-click-to-copy}
[driver_host,driver] ERROR: [src/devices/bin/driver_host/driver_host.cc(134)] Failed to start driver url=fuchsia-pkg://fuchsia.com/simple_driver#meta/simple.cm status_str=ZX_ERR_INTERNAL
[driver_manager.cm] ERROR: [src/devices/bin/driver_manager/driver_host.cc(152)] Failed to start driver 'driver/simple.so' in driver host: ZX_ERR_INTERNAL
```

At this point, start debugging the `Start()` function to identify
the cause of the error. Here are some possible reasons:

- [Driver fails to communicate with a FIDL service](#driver-fails-to-communicate-with-a-fidl-service)
- [`Required service <...> was not available for target component`](#required-service-was-not-available-for-target-component)

### Driver fails to communicate with a FIDL service (PEER_CLOSED error) {:#driver-fails-to-communicate-with-a-fidl-service}

If a FIDL service is not set up properly in the driver, you may get a
`PEER_CLOSED` error when the driver tries to send a request to the
FIDL service.

To debug a `PEER_CLOSED` error, try the following steps:

1. [Verify the outgoing directory](#verify-the-outgoing-directory).
2. [Check the instance name](#check-the-instance-name).
3. [Check for capability routing issues](#check-for-capability-routing-issues).

Plus, to see proper FIDL service setups in DFv2 drivers, check out
these [transport examples][transport-examples].

#### 1. Verify the outgoing directory {:#verify-the-outgoing-directory}

Examine the setup where the parent driver adds the FIDL service to its
outgoing directory, for example:

```none {:.devsite-disable-click-to-copy}
zx::result result = outgoing()->AddService<fuchsia_examples_gizmo::Service>(std::move(handler));
if (result.is_error()) {
  fdf::error("Failed to add service: {}", result);
  return result.take_error();
}
```

#### 2. Check the instance name {:#check-the-instance-name}

If the target driver is a [composite driver][composite-drivers], it needs to pass
the parent node's name to the FIDL service instance.

For example, let's say a composite driver has the following bind rules:

```none {:.devsite-disable-click-to-copy}
composite mali;

using fuchsia.arm.platform;
using fuchsia.platform;

primary parent "mali" {
  fuchsia.Service == "fuchsia.hardware.gpu.mali.Service";
}

parent "pdev" {
  fuchsia.BIND_PROTOCOL == fuchsia.platform.BIND_PROTOCOL.DEVICE;
  fuchsia.BIND_PLATFORM_DEV_VID == fuchsia.arm.platform.BIND_PLATFORM_DEV_VID.ARM;
  fuchsia.BIND_PLATFORM_DEV_PID == fuchsia.platform.BIND_PLATFORM_DEV_PID.GENERIC;
  fuchsia.BIND_PLATFORM_DEV_DID == fuchsia.arm.platform.BIND_PLATFORM_DEV_DID.MAGMA_MALI;
}
```

If we want to connect to the `fuchsia.hardware.platform.device` service from the
parent node named `pdev`, the driver needs to include the following code:

```none {:.devsite-disable-click-to-copy}
auto platform_device = incoming->Connect<fuchsia_hardware_platform_device::Service::Device>("pdev");
```

#### 3. Check for capability routing issues {:#check-for-capability-routing-issues}

See the [Capability routing error](#capability-routing-error) section below.

### Capability routing error {:#capability-routing-error}

To use a service in Fuchsia, the service's [capability][capabilities] must be
routed correctly to the driver at runtime. And to route the capability to
a child driver, the parent driver needs to expose the capability and offer
it to the child.

The following command can help diagnose capability routing issues:

```posix-terminal
ffx component doctor <DRIVER_URL>
```

Note: When you run this command, the driver must be running. This may require
you to modify the driver so that it still runs after capability routing fails.

This command prints output similar to the following:

```none {:.devsite-disable-click-to-copy}
$ ffx component doctor fuchsia-pkg://fuchsia.com/driver_transport#meta/driver_transport_child.cm
Moniker: bootstrap/full-drivers:dev.driver_transport_parent.driver_transport_child
      Used Capability                 Result
 [✓]  fuchsia.logger.LogSink          Success
 [✗]  fuchsia.examples.gizmo.Service  `fuchsia.examples.gizmo.Service` was not offered to `bootstrap/full-pkg-
                                      drivers:dev.driver_transport_parent.driver_transport_child`
                                      by parent.

For further diagnosis, try:

  $ ffx component route bootstrap/full-drivers:dev.driver_transport_parent.driver_transport_child fuchsia.examples.gizmo.Service
```

To resolve issues related to capability routing, try the following steps:

1. [Expose the capability](#expose-the-capability).
2. [Offer the capability](#offer-the-capability).
3. [Use the capability](#use-the-capability).

Plus, to see how capabilities are routed in DFv2 drivers,
check out these [transport examples][transport-examples].

#### 1. Expose the capability {:#expose-the-capability}

If the parent driver does not expose the capability correctly, you may see a
message like below in the logs:

```none {:.devsite-disable-click-to-copy}
[full-drivers:dev.driver_transport_parent.driver_transport_child] WARN: Required service `fuchsia.examples.gizmo.Service` was not available for target component `bootstrap/full-drivers:dev.driver_transport_parent.driver_transport_child`: could not find capability: `fuchsia.examples.gizmo.Service` was not exposed to `bootstrap` from child `#full-drivers:dev.driver_transport_parent`. For more, run `ffx component doctor bootstrap`.
To learn more, see https://fuchsia.dev/go/components/connect-errors
```

To fix this issue, examine how the capability is exposed in the parent
driver's component manifest (`.cml`) file, for example:

```none {:.devsite-disable-click-to-copy}
{
    include: [ 'syslog/client.shard.cml' ],
    program: {
        runner: 'driver',
        binary: 'driver/driver_transport_parent.so',
        bind: 'meta/bind/parent-driver.bindbc',
    },
    capabilities: [
        { service: 'fuchsia.examples.gizmo.Service' },
    ],
    expose: [
        {
            service: 'fuchsia.examples.gizmo.Service',
            from: 'self',
        },
    ],
}
```

#### 2. Offer the capability {:#offer-the-capability}

To check if the offer of the service exists in the child node, run the
following command (you may also use the `ffx component doctor` command):

```posix-terminal
ffx driver node list <child_node_name>
```

This command prints the details of all the services offered in
each node, for example:

```none {:.devsite-disable-click-to-copy}
          Name:  intel-hda-000
       Moniker:  PCI0.bus.00_01_0.00_01_0.intel-hda-000
         Owner:  parent
    Node State:  Bound
     Host Koid:
  Parent Count:  1
   Child Count:  1

  Bus Topology:  Bus Type  Stability  Address
                 Pci       Stable     0, 1, 0

  Node Properties:  Key                            Value
                    fuchsia.BIND_PROTOCOL          76
                    fuchsia.Service                fuchsia.driver.compat.Service

  Node Offers:  Service                        Source                    Instances
                fuchsia.driver.compat.Service  PCI0.bus.00_01_0.00_01_0  default
```

Also, if the offer is not found in the child node, you may see
a message like below in the logs:

```none {:.devsite-disable-click-to-copy}
[00008.035682][full-drivers:dev.driver_transport_parent.driver_transport_child] WARN: Required service `fuchsia.examples.gizmo.Service` was not available for target component `bootstrap/full-drivers:dev.driver_transport_parent.driver_transport_child`: could not find capability: `fuchsia.examples.gizmo.Service` was not offered to `bootstrap/full-drivers:dev.driver_transport_parent.driver_transport_child` by parent.
To learn more, see https://fuchsia.dev/go/components/connect-errors
```

To fix this issue, examine the setup where the parent driver includes
the offer to the child node, for example:

```none {:.devsite-disable-click-to-copy}
// Add a child with a `fuchsia.examples.gizmo.Service` offer.
zx::result child_result =
    AddChild("driver_transport_child", {}, {fdf::MakeOffer2<fuchsia_examples_gizmo::Service>()});
if (child_result.is_error()) {
  return child_result.take_error();
}
```

#### 3. Use the capability {:#use-the-capability}

To use a capability, the driver needs to declare that it wants to use the
capability.

If this is not declared correctly, you may see a message like below
in the logs:

```none {:.devsite-disable-click-to-copy}
[00008.631682][full-drivers:dev.driver_transport_parent.driver_transport_child] WARN: No capability available at path /svc/fuchsia.examples.gizmo.Service/default/device for component bootstrap/full-drivers:dev.driver_transport_parent.driver_transport_child, verify the component has the proper `use` declaration.
```

To fix this issue, examine how the capability's use is declared in the
driver's component manifest (`.cml`) file, for example:

```none {:.devsite-disable-click-to-copy}
{
    include: [ 'syslog/client.shard.cml' ],
    program: {
        runner: 'driver',
        binary: 'driver/driver_transport_child.so',
        bind: 'meta/bind/child-driver.bindbc',
        colocate: 'true',
    },
    use: [
        { service: 'fuchsia.examples.gizmo.Service' },
    ],
}
```

## Error messages {:#error-messages}

Common error messages in Fuchsia driver development:

- [`Failed to load driver <...> driver note not found`](#failed-to-load-driver)
- [`Required service <...> was not available for target component`](#required-service-was-not-available-for-target-component)
- [`Driver-Loader: libdriver.so: Not in allowlist`](#driver-loader-libdriver-so-not-in-allowlist)
- [`__fuchsia_driver_registration__ symbol not available`](#fuchsia-driver-registration-symbol-not-available)
- [`Could not load driver: <...>: Failed to open bind file (or Missing bind path)`](#could-not-load-driver-failed-to-open-bind-file)
- [`no member named 'destroy' in 'fdf_internal::DriverServer<...>`](#no-member-named-destroy)
- [`Failed to start driver, missing 'binary' argument: ZX_ERR_NOT_FOUND`](#could-not-start-driver-missing-binary)

### Failed to load driver <...> driver note not found {:#failed-to-load-driver}

When migrating a DFv1 driver to DFv2, you may run into the following error message
in the logs:

```none {:.devsite-disable-click-to-copy}
Failed to load driver <...> driver not found
```

When this error occurs, examine the driver's component manifest (`.cml`) file
for correctness. See [Driver fails to bring up](#driver-fails-to-bring-up).

### Required service <...> was not available for target component {:#required-service-was-not-available-for-target-component}

This error occurs if the driver does not have a service capability correctly exposed
and routed to it from the parent driver.

To fix this issue, try the [Capability routing error](#capability-routing-error)
workflow.

**However, if the required service is `fuchsia.driver.compat`, follow the
instructions below**:

The error occurs if the driver does not have a `fuchsia.driver.compat` service
routed to it from a DFv2 driver. To route this service, the DFv2 driver needs to
[set up a compat device server][compat-device-server] for each child.

To fix this issue, go through each parent driver starting from the affected driver.
For example, if the node topology shows `A->B->C->D` and `D` is the affected driver,
then go through `C`, `B`, and `A`. For each parent driver in the chain, if it's
written in DFv2, verify that its compat device server is set up properly.

Here are some common issues related to setting up a compat device server:

- The compat device server must be initialized with the correct child node name.
- The child must be [added with an offer][offer-compat-device-server] from the
  compat device server.

### Driver-Loader: libdriver.so: Not in allowlist {:#driver-loader-libdriver-so-not-in-allowlist}

This error occurs if `libdriver` is in one of a DFv2 driver's dependencies, which
can be transitive.

To fix the issue, search and remove `//src/devices/lib/driver` from the dependencies.

To find the dependency, use the following command:

```posix-terminal
fx gn path <out_directory> //path/to/driver //path/to/libdriver
```

### \__fuchsia_driver_registration\_\_ symbol not available {:#fuchsia-driver-registration-symbol-not-available}

When migrating a DFv1 driver to DFv2, you may run into the following error message
in the logs:

```none {:.devsite-disable-click-to-copy}
[driver_host,driver] ERROR: [src/devices/bin/driver_host/driver.cc(120)] __fuchsia_driver_registration__ symbol not available_.
[driver_host,driver] ERROR: [src/devices/bin/driver_host/driver.cc(316)] Failed to start driver 'fuchsia-pkg://fuchsia.com/fake-battery#meta/fake_battery.cm', could not Load driver: ZX_ERR_NOT_FOUND
```

This error occurs when a DFv2 driver doesn't include the `FUCHSIA_DRIVER_EXPORT` macro.

To fix this issue, see [Add the driver export macro][driver-export-macro].

Also, make sure that the macro is defined in `.cc` files, not in header (`.h`) files.

### Could not load driver: <...>: Failed to open bind file (or Missing bind path) {:#could-not-load-driver-failed-to-open-bind-file}

When migrating a DFv1 driver to DFv2, you may run into the following error message
in the logs:

```none {:.devsite-disable-click-to-copy}
Could not load driver: fuchsia-pkg://fuchsia.com/fake-battery#meta/fake_battery.cm: Failed to open bind file 'meta/bind/fake-battery-driver_2.bindbc'
```

Or

```none {:.devsite-disable-click-to-copy}
Could not load driver: fuchsia-pkg://fuchsia.com/fake-battery#meta/fake_battery.cm: Missing bind path
```

This error occurs when the `bind` field  is either missing or incorrect in
a DFv2 driver's component manifest (`.cml`) file, for example:

```none {:.devsite-disable-click-to-copy}
{
    include: [
        "syslog/client.shard.cml",
    ],
    program: {
        runner: "driver",
        binary: "driver/fake_battery.so",
        bind: <incorrect bind file path>,
    },
}
```

The `bind` field value needs to follow the format `meta/bind/<bind_output>` where
`bind_output` is a field with the same name in the `driver_bind_rules` target
in the build file, for example:

```none {:.devsite-disable-click-to-copy}
driver_bind_rules("fake_battery_bind") {
  rules = "meta/fake-battery-driver.bind"
  bind_output = "fake-battery-driver.bindbc"
  deps = [ "//src/devices/bind/fuchsia.test" ]
  deps += [ "//src/devices/bind/fuchsia.platform" ]
}
```

For this bind rule target example, the correct `bind` field in the component
manifest file looks like below:

```none {:.devsite-disable-click-to-copy}
{
    include: [
        "syslog/client.shard.cml",
    ],
    program: {
        runner: "driver",
        binary: "driver/fake_battery.so",
        bind: "meta/bind/fake-battery-driver.bindbc",
    },
}
```

However, if `bind_output` is not explicitly defined in a bind rule target,
the default value is the target name with `.bindbc` appended to it, which would
be `fake_battery_bind.bindbc` in the example above.

### Failed to start driver, missing 'binary' argument: ZX_ERR_NOT_FOUND {:#could-not-start-driver-missing-binary}

When writing a DFv2 driver, you may run into the following error message
in the logs:

```none {:.devsite-disable-click-to-copy}
Failed to start driver, missing 'binary' argument: ZX_ERR_NOT_FOUND
```

The `binary` field value needs to follow the format `driver/<driver_output>.so` where
`driver_output` is a field with the same name in the `fuchsia_driver` target in the
build file, for example:

```
fuchsia_cc_driver("driver") {
  output_name = "simple"
  sources = [ "simple_driver.cc" ]
  deps = [
    "//sdk/fidl/fuchsia.driver.compat:fuchsia.driver.compat_cpp",
    "//sdk/lib/driver/compat/cpp",
    "//sdk/lib/driver/component/cpp",
    "//src/devices/bind/fuchsia.test:fuchsia.test_cpp",
    "//src/devices/lib/driver:driver_runtime",
  ]
}
```

For this bind rule target example, the correct `binary` field in the component manifest file looks
like below:

```
{
    include: [
        "driver_component/driver.shard.cml",
        "inspect/client.shard.cml",
        "syslog/client.shard.cml",
    ],
    program: {
        runner: "driver",
        binary: "driver/simple.so",
        bind: "meta/bind/simple_driver.bindbc",
    },
}
```

### no member named 'destroy' in 'fdf_internal::DriverServer<...>' {:#no-member-named-destroy}

If a DFv2 driver implements an open FIDL protocol (that is,
`open protocol <PROTOCOL_NAME>`), the driver needs to override and implement
the `handle_unknown_method()` function.

For example, let's say a driver implements the following protocol:

```none {:.devsite-disable-click-to-copy}
open protocol VirtualController {
    ...
};
```

Then this driver needs to include the following `handle_unknown_method()` function:

```none {:.devsite-disable-click-to-copy}
void handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_bluetooth::VirtualController> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) override;
```

## Other resources {:#other-resources}

* [View driver information][view-driver-information] - A reference page including how to
  use `ffx driver` for debugging.

<!-- Reference links -->

[composite-drivers]: /docs/development/drivers/developer_guide/create-a-composite-node.md
[composite-node-spec]: /docs/development/drivers/developer_guide/create-a-composite-node.md#what-is-a-composite-node-specification
[dfv2-driver-transport]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/transport/driver/v2/
[dfv2-zircon-transport]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/transport/zircon/v2/
[transport-examples]: /docs/development/drivers/developer_guide/driver-examples.md#transport_examples
[capabilities]: /docs/concepts/components/v2/capabilities/README.md
[compat-device-server]: /docs/development/drivers/migration/set-up-compat-device-server.md#set-up-the-compat-device-server
[offer-compat-device-server]: /docs/development/drivers/migration/set-up-compat-device-server.md#offer-the-compat-device-server-to-the-target-child-node
[driver-export-macro]: /docs/development/drivers/developer_guide/write-a-minimal-dfv2-driver.md#add-the-driver-export-macro
[view-driver-information]: /docs/development/tools/ffx/workflows/view-driver-information.md
