# Update other services to DFv2

This page provides instructions, best practices, and examples related to
updating various services (other than the DDK interfaces) in DFv1 drivers
to DFv2.

## Set up the compat device server {:#set-up-the-compat-device-server}

If your DFv1 driver talks to other DFv1 drivers that haven't yet migrated
to DFv2, you need to use the compatibility shim to enable your now-DFv2 driver to
talk to other DFv1 drivers in the system. For more information on setting up and
using this compatibility shim in a DFv2 driver, see the
[Set up the compat device server in a DFv2 driver][set-up-compat-device-server]
guide.

## Use the DFv2 service discovery {:#use-the-dfv2-service-discovery}

When working on driver migration, you will likely encounter one or more
of the following three scenarios in which two drivers establish a FIDL
connection (in `child driver -> parent driver` format):

- **Scenario 1**: DFv2 driver -> DFv2 driver
- **Scenario 2**: DFv1 driver -> DFv2 driver
- **Scenario 3**: DFv2 driver -> DFv1 driver

**Scenario 1** is the standard case for DFv2 drivers (this
[example][runtime-protocol-test] shows the new DFv2 syntax). To update
your driver under this scenario, see the
[DFv2 driver to DFv2 driver](#dfv2-driver-to-dfv2-driver) section below.

**Scenario 2 and 3** are more complicated because the DFv1 driver is
wrapped in the compatibility shim in the DFv2 world. However,
the differences are:

- In **scenario 2**, this [Gerrit change][gc-scenario-2]{:.external} shows
  a method that exposes a service from the DFv2 parent to the DFv1 child.

- In **scenario 3**, the driver is connected to the
  `fuchsia_driver_compat::Service::Device` protocol provided by the
  compatibility shim of the parent driver, and the driver calls the
  `ConnectFidl()` method through this protocol to connect to the real
  protocol (for an example, see this
  [Gerrit change][pcie-iwlwifi-driver-cc]{:.external}).

To update your driver under **scenario 2 or 3**, see the
[DFv1 driver to DFv2 driver (with compatibility shim)](#dfv1-driver-to-dfv2-driver)
section below.

### DFv2 driver to DFv2 driver {:#dfv2-driver-to-dfv2-driver}

To enable other DFv2 drivers to discover your driver's service,
do the following:

1. Update your driver's `.fidl` file.

   The protocol discovery in DFv2 requires adding `service` fields for
   the driver's protocols, for example:

   ```none
   library fuchsia.example;

   @discoverable
   @transport("Driver")
   protocol MyProtocol {
       MyMethod() -> (struct {
           ...
       });
   };

   {{ '<strong>' }}service Service {
       my_protocol client_end:MyProtocol;
   };{{ '</strong>' }}
   ```

1. Update the child driver.

   DFv2 drivers can connect to protocols in the same way as FIDL services,
   for example:

   ```cpp
   incoming()->Connect<fuchsia_example::Service::MyProtocol>();
   ```

   You also need to update the component manifest (`.cml`) file to use
   your driver runtime service, for example:

   ```none
   use: [
       { service: "fuchsia.example.Service" },
   ]
   ```

1. Update the parent driver.

   Your parent driver needs to use the `fdf::DriverBase`'s `outgoing()` function to get the
   `fdf::OutgoingDirectory` object. Note that you must use services rather than protocols.
   If your driver isn't using `fdf::DriverBase` you must create and serve an
   `fdf::OutgoingDirectory` on your own.

   Then you need to add the runtime service to your outgoing directory.
   The example below is a driver that inherits from the `fdf::DriverBase`
   class:

   ```cpp
   zx::status<> Start() override {
     auto protocol = [this](
         fdf::ServerEnd<fuchsia_example::MyProtocol> server_end) mutable {
       // bindings_ is a class field with type fdf::ServerBindingGroup<fuchsia_example::MyProtocol>
       bindings_.AddBinding(
         dispatcher()->get(), std::move(server_end), this, fidl::kIgnoreBindingClosure);
     };

     fuchsia_example::Service::InstanceHandler handler(
          {.my_protocol = std::move(protocol)});

     auto status =
           outgoing()->AddService<fuchsia_wlan_phyimpl::Service>(std::move(handler));
     if (status.is_error()) {
       return status.take_error();
     }

     return zx::ok();
   }
   ```

   Update the child node's [`NodeAddArgs`][nodeaddargs] to include an offer
   for your runtime service, for example:

   ```cpp
   {{ '<strong>' }}auto offers =
       std::vector{fdf::MakeOffer2<fuchsia_example::Service>(arena, name)};{{ '</strong>' }}

   fidl::WireSyncClient<fuchsia_driver_framework::Node> node(std::move(node()));
     auto args = fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena)
                       .name(arena, "example_node")
                       {{ '<strong>' }}.offers2(offers){{ '</strong>' }}
                       .Build();

     zx::result controller_endpoints =
          fidl::CreateEndpoints<fuchsia_driver_framework::NodeController>();
     ZX_ASSERT(controller_endpoints.is_ok());

     auto result = node_->AddChild(
         args, std::move(controller_endpoints->server), {});
   ```

   Similarly, update the parent driver's component manifest (`.cml`) file to
   offer your runtime service, for example:

   ```cpp
   capabilities: [
       { service: "fuchsia.example.Service" },
   ],

   expose: [
       {
           service: "fuchsia.example.Service",
           from: "self",
       },
   ],
   ```

### DFv1 driver to DFv2 driver (with compatibility shim) {:#dfv1-driver-to-dfv2-driver}

To enable other DFv1 drivers to discover your DFv2 driver's service,
do the following:

1. Update the DFv1 drivers.

   You need to update the component manifest (`.cml`) files of the DFv1
   drivers in the same way as mentioned in the
   [DFv2 driver to DFv2 driver](#dfv2-driver-to-dfv2-driver) section above,
   for example:

   - Child driver:

     ```none
     {
         include: [
             "//sdk/lib/driver_compat/compat.shard.cml",
             "inspect/client.shard.cml",
             "syslog/client.shard.cml",
         ],
         program: {
             runner: "driver",
             compat: "driver/child-driver-name.so",
             bind: "meta/bind/child-driver-name.bindbc",
             colocate: "true",
         },
         {{ '<strong>' }}use: [
             { service: "fuchsia.example.Service" },
         ],{{ '</strong>' }}
     }
     ```

   - Parent driver:

     ```none
     {
         include: [
             "//sdk/lib/driver_compat/compat.shard.cml",
             "inspect/client.shard.cml",
             "syslog/client.shard.cml",
         ],
         program: {
             runner: "driver",
             compat: "driver/parent-driver-name.so",
             bind: "meta/bind/parent-driver-name.bindbc",
         },
         {{ '<strong>' }}capabilities: [
             { service: "fuchsia.example.Service" },
         ],
         expose: [
             {
                 service: "fuchsia.example.Service",
                 from: "self",
             },
         ],{{ '</strong>' }}
     }
     ```

1. Update the DFv2 driver.

   The example below shows a method that exposes a service from the DFv2
   parent to the DFv1 child:

   ```cpp
     fit::result<fdf::NodeError> AddChild() {
       fidl::Arena arena;

       auto offer = fdf::MakeOffer2<ft::Service>(kChildName);

       // Set the properties of the node that a driver will bind to.
       auto property =
           fdf::MakeProperty(1 /*BIND_PROTOCOL */, bind_fuchsia_test::BIND_PROTOCOL_COMPAT_CHILD);

       auto args = fdf::NodeAddArgs{
         {
           .name = std::string(kChildName),
           .properties = std::vector{std::move(property)},
           .offers2 = std::vector{std::move(offer)},
         }
       };

       // Create endpoints of the `NodeController` for the node.
       auto endpoints = fidl::CreateEndpoints<fdf::NodeController>();
       if (endpoints.is_error()) {
         return fit::error(fdf::NodeError::kInternal);
       }

       auto add_result = node_.sync()->AddChild(fidl::ToWire(arena, std::move(args)),
                                                std::move(endpoints->server), {});
   ```

   (Source: [`root-driver.cc`][root-driver-cc])

## Update component manifests of other drivers {:#update-component-manifests-of-other-drivers}

To complete the migration of a DFv1 driver to DFv2, you not only need
to update the component manifest (`.cml`) file of your target driver,
but you may also need to update the component manifest files of some
other drivers that interact with your now-DFv2 driver.

Do the following:

1. Update the component manifests of leaf drivers (that is, without
   child drivers) with the changes below:

   - Remove `//sdk/lib/driver/compat/compat.shard.cml` from the
     `include` field.
   - Replace the `program.compat` field with `program.binary`.

2. Update the component manifests of other drivers that perform the
   following tasks:

   - Access kernel `args`.
   - Create composite devices.
   - Detect reboot, shutdown, or rebind calls.
   - Talk to other drivers using the Banjo protocol.
   - Access metadata from a parent driver or forward it.
   - Talk to a DFv1 driver that binds to a node added by your driver.

   For these drivers, update their component manifest with the changes
   below:

   - Copy some of the `use` capabilities from
     [`compat.shard.cml`][compat-shard-cml] to the component manifest,
     for example:

     ```none
     use: [
         {
             protocol: [
                 "fuchsia.boot.Items",
                 "fuchsia.driver.framework.CompositeNodeManager",
                 "fuchsia.system.state.SystemStateTransition",
             ],
         },
         { service: "fuchsia.driver.compat.Service" },
     ],
     ```

   - Set the `program.runner` field to `driver`, for example:

     ```none
     program: {
         runner: "driver",
         binary: "driver/compat.so",
     },
     ```

## Use dispatchers {:#use-dispatchers}

[Dispatchers][driver-dispatcher] fetch data from a channel between
a FIDL client-and-server pair. By default, FIDL calls in this channel
are asynchronous.

For introducing asynchronization to drivers in DFv2, see the following
suggestions:

- The `fdf::Dispatcher::GetCurrent()` method gives you the default
  dispatcher that the driver is running on (see this
  [`aml-ethernet`][aml-ethernet] driver example). If possible, it is
  recommended to use this default dispatcher alone.

- Consider using multiple dispatchers for the following reasons
  (but not limited to):

  - The driver requires parallelism for performance.

  - The driver wants to perform blocking operations (because it is
    either a legacy driver or a non-Fuchsia driver being ported to
    Fuchsia) and it needs to handle more work while blocked.

- If multiple dispatchers are needed, the `fdf::Dispatcher::Create()`
  method can create new dispatchers for your driver. However, you must
  call this method on the default dispatcher (for example, call it
  inside the `Start()` hook) so that the driver host is aware of the
  additional dispatchers that belong to your driver.

- In DFv2, you don't need to shut down the dispatchers manually. They
  will be shut down between the `PrepareStop()` and `Stop()` calls.

For more details on migrating a driver to use multiple dispatchers,
see the
[Update the DFv1 driver to use non-default dispatchers][use-non-default-dispatchers]
section (in the [Migrate from Banjo to FIDL][migrate-from-banjo-to-fidl]
phrase).

## Use the DFv2 inspect {:#use-the-dfv2-inspect}

To set up driver-maintained [inspect][driver-inspect] metrics in DFv2,
you may use the `ComponentInspector` provided by `fdf::DriverBase::inspector()`:

```cpp
inspect::Node& root = inspector().root();
```

If a custom Inspector should be used, call `fdf::DriverBase::InitInspectorExactlyOnce(inspector)`
before accessing the `inspector()` method.

DFv2 inspect does not require passing the VMO of `inspect::Inspector` to the driver framework.

DFv2 drivers' Inspect will show up attributed to the driver (since it is a "normal" component).
The monikers of DFv2 drivers are not stable, however, so when writing privacy selectors against
a driver you should use wildcards and name filters to refer to a specific driver. For example,

```
bootstrap/*-drivers*:[name=sysmem]root
```

To access a driver's Inspect during debugging, you can use all the normal tools, such as

```
ffx inspect show --name sysmem "bootstrap/*-drivers*:root"
```

or

```
ffx inspect show sysmem.cm
```

## (Optional) Implement your own load_firmware method {:#implement-your-own-load-firmware-method}

If your DFv1 driver calls the [`load_firmware()`][load-firmware]
function in the DDK library, you need to implement your own version
of this function because an equivalent function is not available in DFv2.

This function is expected to be simple to implement. You need to get
the backing VMO from the path manually. For an example, see this
[Gerrit change][gc-pcie-iwlwifi-driver]{:.external}.

## (Optional) Use the node properties generated from FIDL service offers {:#use-the-node-properties-generated-from-fidl-service-offers}

DFv2 nodes contain the node properties generated from the FIDL service
offers from their parents.

For instance, in the [Parent Driver (The Server)][parent-driver-the-server]
example, the parent driver adds a node called `"parent"` with a service
offer for `fidl.examples.echo.EchoService`. In DFv2, a driver that binds to this
node can have a bind rule for that FIDL service node property, for example:

```none
fuchsia.Service == "fidl.examples.echo.EchoService";
```


## Update unit tests to DFv2 {:#update-unit-tests-to-dfv2}

The [`mock_ddk`][mock-ddk] library (which is used in unit tests for testing
driver and device life cycle) is specific to DFv1. The new DFv2 test
framework (see this [Gerrit change][gc-driver-testing]{:.external}) makes
mocked FIDL servers available to DFv2 drivers through the `TestEnvironment`
class.

The following libraries are available for unit testing DFv2 drivers:

- [`//sdk/lib/driver/testing/cpp`][driver-testing-cpp]

  - `TestNode` - This class implements the `fuchsia_driver_framework::Node`
    protocol, which can be provided to a driver to create child nodes. This
    class is also used by tests to access the child nodes that the driver
    has created.

  - `TestEnvironment` - A wrapper over an `OutgoingDirectory` object that
    serves as the backing VFS (virtual file system) for the incoming
    namespace of the driver under test.

  - `DriverUnderTest` - This class is a RAII
    ([Resource Acquisition Is Initialization][raii]{:.external}) wrapper
    for the driver under test.

  - `DriverRuntime` - This class is a RAII wrapper over the managed driver
    runtime thread pool.

- [`//sdk/lib/driver/testing/cpp/driver_runtime.h`][driver-testing-runtime]

  - `TestSynchronizedDispatcher` - This class is a RAII wrapper over the
    driver dispatcher.

The following library may be helpful for writing driver unit tests:

- [`//src/devices/bus/testing/fake-pdev/fake-pdev.h`][fake-pdev-h] - This
  helper library implements a fake version of the `pdev` FIDL protocol.

Lastly, the following example unit tests cover different configurations and
test cases:

- [`//sdk/lib/driver/component/cpp/tests/driver_base_test.cc`][driver-base-test-cc] -
  This file contains examples of the different threading models that driver
  tests can have.

- [`//sdk/lib/driver/component/cpp/tests/driver_fidl_test.cc`][driver-fidl-test-cc] -
  This file demonstrates how to work with incoming and outgoing FIDL
  services for both driver transport and Zircon transport.

## Additional resources {:#additional-resources}

Some DFv2 drivers examples:

- [`MagmaDriverBase`][magma-driver-base]
- [`WlanTapDriver`][wlantap-driver]
- [`AdcButtonsDriver`][adc-button-driver]

All the **Gerrit changes** mentioned in this section:

- [\[iwlwifi\] Dfv2 migration for iwlwifi driver][gc-iwlwifi-driver]{:.external}
- [\[compat-runtime-test\] Migrate off usage of DeviceServer][gc-scenario-2]{:.external}
- [\[msd-arm-mali\] Add DFv2 version][gc-msd-arm-mali-top-level]{:.external}
- [\[sdk\]\[driver\]\[testing\] Add testing library][gc-driver-testing]{:.external}

All the **source code files** mentioned in this section:

- [`//examples/drivers/transport/zircon/v2/parent-driver.cc`][v2-parent-driver-cc]
- [`//sdk/fidl/fuchsia.driver.framework/topology.fidl`][topology-fidl-80]
- [`//sdk/lib/driver/component/cpp/driver_base.h`][driver-base-h-70]
- [`//sdk/lib/driver/component/cpp/tests/driver_base_test.cc`][driver-base-test-cc]
- [`//sdk/lib/driver/component/cpp/tests/driver_fidl_test.cc`][driver-fidl-test-cc]
- [`//sdk/lib/driver/compat/cpp/banjo_server.h`][banjo-server-h]
- [`//sdk/lib/driver/compat/cpp/banjo_client.h`][banjo-client-h]
- [`//sdk/lib/driver/compat/cpp/device_server.h`][device-server-h-23]
- [`//sdk/lib/driver/testing/cpp/driver_runtime.h`][driver-testing-runtime]
- [`//src/connectivity/wlan/testing/wlantap-driver/wlantap-driver.cc`][wlantap-driver]
- [`//src/devices/bus/testing/fake-pdev/fake-pdev.h`][fake-pdev-h]
- [`//src/devices/tests/v2/compat-runtime/root-driver.cc`][root-driver-cc]
- [`//src/lib/ddk/include/lib/ddk/device.h`][ddk-device-h-77]
- [`//src/lib/ddk/include/lib/ddk/driver.h`][load-firmware]

All the **documentation pages** mentioned in this section:

- [Drivers and nodes][driver-node]
- [Driver communication][driver-communication]
- [Drivers and nodes][driver-node]
- [Driver dispatcher and threads][driver-dispatcher]
- [Drivers][driver-concepts]
- [Create a composite node][composite-node]
- [Expose the driver capabilities][codelab-driver-service]
- [Fuchsia component inspection overview][driver-inspect]
- [Mock DDK Migration][mock-ddk]
- [An Example of the Tear-Down Sequence][device-lifecycle]
  (from _Device driver lifecycle_)
- [Parent Driver (The Server)][parent-driver-the-server]
  (from _FIDL tutorial_)

<!-- Reference links -->

[migrate-from-banjo-to-fidl]: /docs/development/drivers/migration/migrate-from-banjo-to-fidl/overview.md
[driver-dispatcher]: /docs/concepts/drivers/driver-dispatcher-and-threads.md
[driver-node]: /docs/concepts/drivers/drivers_and_nodes.md
[fake-pdev-h]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/bus/testing/fake-pdev/fake-pdev.h
[driver-testing-cpp]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/testing/cpp/
[driver-base-test-cc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/component/cpp/tests/driver_base_test.cc
[device-lifecycle]: /docs/development/drivers/concepts/device_driver_model/device-lifecycle.md#an_example_of_the_tear-down_sequence
[ddk-device-h-77]: https://source.corp.google.com/fuchsia/src/lib/ddk/include/lib/ddk/device.h;l=77
[ddk-driver-h-29]: https://source.corp.google.com/fuchsia/src/lib/ddk/include/lib/ddk/driver.h;l=29
[load-firmware]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/lib/ddk/include/lib/ddk/driver.h;l=416
[logging-h]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/compat/cpp/logging.h
[synchronized-dispatchers]: /docs/concepts/drivers/driver-dispatcher-and-threads.md#synchronized-and-unsynchronized
[gc-intel-wifi]:https://fuchsia-review.git.corp.google.com/c/fuchsia/+/692243
[pcie-iwlwifi-driver-cc]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/692243/47/src/connectivity/wlan/drivers/third_party/intel/iwlwifi/platform/pcie-iwlwifi-driver.cc
[driver-base]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/component/cpp/driver_base.h?q=sdk%2Flib%2Fdriver%2Fcomponent%2Fcpp%2Fdriver_base.h&ss=fuchsia%2Ffuchsia
[driver-base-99]: https://source.corp.google.com/h/turquoise-internal/turquoise/+/main:sdk/lib/driver/component/cpp/driver_base.h;l=99
[addchild]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.driver.framework/topology.fidl;drc=53130c6bb8b33ae921bb49a561966cbdbc2d6595;l=158
[nodecontroller]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.driver.framework/topology.fidl;l=101;drc=53130c6bb8b33ae921bb49a561966cbdbc2d6595
[runtime-protocol-test]: http://cs/fuchsia/src/devices/tests/v2/runtime-protocol/
[gc-scenario-2]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/920734
[pcie-iwlwifi-driver-cc]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/692243/47/src/connectivity/wlan/drivers/third_party/intel/iwlwifi/platform/pcie-iwlwifi-driver.cc#323
[nodeaddargs]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.driver.framework/topology.fidl;l=81
[root-driver-cc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/tests/v2/compat-runtime/root-driver.cc
[compat-shard-cml]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/compat/compat.shard.cml
[v2-parent-driver-cc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/transport/zircon/v2/parent-driver.cc;l=93
[codelab-driver-service]: /docs/development/drivers/tutorials/sdk_build_driver/driver-service.md
[use-non-default-dispatchers]: /docs/development/drivers/migration/migrate-from-banjo-to-fidl/convert-banjo-protocols-to-fidl-protocols.md#update-the-dfv1-driver-to-use-non-default-dispatchers
[aml-ethernet]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/connectivity/ethernet/drivers/aml-ethernet/aml-ethernet.cc;l=181
[driver-inspect]: /docs/development/drivers/diagnostics/inspect.md
[wlantap-driver]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/connectivity/wlan/testing/wlantap-driver/wlantap-driver.cc;l=61
[logger-h]: https://source.corp.google.com/h/turquoise-internal/turquoise/+/main:sdk/lib/driver/logging/cpp/logger.h;l=15
[load-firmware]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/lib/ddk/include/lib/ddk/driver.h;l=408
[gc-pcie-iwlwifi-driver]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/692243/47/src/connectivity/wlan/drivers/third_party/intel/iwlwifi/platform/pcie-iwlwifi-driver.cc#60
[parent-driver-the-server]: /docs/development/drivers/tutorials/fidl-tutorial.md#parent_driver_the_server
[mock-ddk]: /docs/contribute/open_projects/graduated/mock_ddk_migration.md
[raii]: https://en.wikipedia.org/wiki/Resource_acquisition_is_initialization
[driver-testing-runtime]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/testing/cpp/driver_runtime.h
[driver-fidl-test-cc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/component/cpp/tests/driver_fidl_test.cc
[magma-driver-base]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/graphics/magma/lib/magma_service/sys_driver/magma_driver_base.h
[wlantap-driver]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/connectivity/wlan/testing/wlantap-driver/wlantap-driver.cc
[adc-button-driver]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/ui/input/drivers/adc-buttons/adc-buttons.h
[driver-base-h-70]: https://source.corp.google.com/fuchsia/sdk/lib/driver/component/cpp/driver_base.h;l=70?q=driverbase&sq=package:fuchsia
[topology-fidl-80]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.driver.framework/topology.fidl;l=80
[root-driver-cc]: https://source.corp.google.com/fuchsia/src/devices/tests/v2/compat-runtime/root-driver.cc
[device-server-h-23]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/compat/cpp/device_server.h;l=23?q=deviceserver&sq=&ss=fuchsia%2Ffuchsia
[driver-concepts]: /docs/concepts/drivers/README.md
[composite-node]: /docs/development/drivers/developer_guide/create-a-composite-node.md
[composite-node-spec]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.driver.framework/composite_node_spec.fidl;l=68
[nodecontroller-remove]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.driver.framework/topology.fidl;drc=53130c6bb8b33ae921bb49a561966cbdbc2d6595;l=103
[gc-iwlwifi-driver]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/692243
[gc-msd-arm-mali]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/853637/5/src/graphics/drivers/msd-arm-mali/BUILD.gn
[gc-msd-arm-mali-top-level]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/853637
[gc-driver-testing]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/770412
[driver-communication]: /docs/concepts/drivers/driver_communication.md
[banjo-server-h]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/compat/cpp/banjo_server.h
[banjo-client-h]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/compat/cpp/banjo_client.h
[set-up-compat-device-server]: /docs/development/drivers/migration/set-up-compat-device-server.md
