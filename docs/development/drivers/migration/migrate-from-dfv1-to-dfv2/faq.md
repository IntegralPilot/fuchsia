# Frequently asked questions for DFv1-to-DFv2 migration

Before you start working DFv1-to-DFv2 migration, the frequently asked
questions below can help you identify special conditions or edge cases
that may apply to your driver.

## What is the compatibility shim and when is it necessary?

DFv1 and DFv2 drivers exist under a single branch of the node topology
tree (that is, until all the drivers are migrated to DFv2) and they need
to be able to talk to each other. To help with the migration process,
Fuchsia's driver framework team created a compatibility shim to enable
DFv1 drivers to live in DFv2.

If your target driver talks to other DFv1 drivers that still use
Banjo and those drivers won't be migrated to DFv2 all at once,
you need to use this compatibility shim (by manually creating
`compat::DeviceServer`) for enabling the drivers in different framework
versions to talk to each other.

For more details on using the compatibility shim in a DFv2 driver,
see the
[Set up the compat device server in a DFv2 driver][set-up-compat-device-server]
guide.

## Can DFv2 drivers talk to Banjo protocols using the compatibility shim?

While it's strongly recommended that your DFv1 driver is migrated from
Banjo to FIDL, if it is necessary for a DFv2 driver to talk
to some existing Banjo protocols, the
[compatibility shim][set-up-compat-device-server] provides the
following features:

- `compat::BanjoServer` makes it easier to serve Banjo
  (see [`banjo_server.h`][banjo-server-h]).
- `compat::ConnectBanjo` makes it easier to connect to Banjo
  (see [`banjo_client.h`][banjo-client-h]).

For more details on these features, see the
[Serve Banjo protocols in a DFv2 driver][serve-banjo-protocols-in-a-dfv2-driver]
guide.

## Can DFv2 drivers use the compatibility shim for composite nodes?

The migration process for composite drivers are nearly identical to
normal drivers, but composite drivers have slightly different
ways for connecting to Banjo or FIDL protocols from parent nodes.

Because composite nodes have multiple parents, composite drivers need
to identify the parent’s name when connecting to it. For example,
below is a normal driver establishing a Banjo connection with its
parent:

```cpp
zx::result client_result =
  compat::ConnectBanjo<ddk::HidDeviceProtocolClient>(incoming());
```

The composite driver’s method is almost identical, except the parent
name needs to be added:

```cpp
zx::result client_result =
  compat::ConnectBanjo<ddk::HidDeviceProtocolClient>(incoming(), "gpio-int")
```

## What has changed in the new DFv2 driver interfaces?

One major change in DFv2 is that drivers take control of the life cycle
of the child [nodes][driver-node] (or devices) created by the drivers.
This is different from DFv1 where the driver framework manages the life
cycles of devices, such as [tearing down devices][device-lifecycle],
through the device tree.

In DFv1, devices are controlled by [`zx_protocol_device`][ddk-device-h-77]
while drivers are controlled by [`zx_driver_ops`][ddk-driver-h-29].
If `ddktl` is used, the interfaces in `zx_protocol_device` need to be
wrapped by `Ddk*()` functions in the mixin template class. In DFv2,
[those interfaces][update-driver-interfaces] have changed
significantly.

## How does service discovery work in DFv2?

In DFv2, using a FIDL service is required to establish a protocol
connection. The parent driver adds a FIDL service to the
`fdf::OutgoingDirectory` object and serves it to the child node,
which then enables the parent driver to offer the service to the
child node.

DFv1 and DFv2 drivers do this differently in the following ways:

- In DFv1, the driver sets and passes the offer from the
  `DeviceAddArgs::set_runtime_service_offers()` call. Then the driver
  creates an `fdf::OutgoingDirectory` object and passes the client
  end handle through the `DeviceAddArgs::set_outgoing_dir()` call.

- In DFv2, the driver sets and passes the offer from the
  `NodeAddArgs::offers` object. The driver adds the service to the
  outgoing directory wrapped by the `DriverBase` class (originally
  provided by the `Start()` function). When the child driver binds to
  the child node, the driver host passes the incoming namespace
  containing the service to the child driver's `Start()` function.

On the child driver side, DFv1 and DFv2 drivers also connect to the
protocol providing the service in different ways:

- A DFv1 driver calls the `DdkConnectRuntimeProtocol<ServiceInstanceName>()`
  method.
- A DFv2 driver calls `incoming()->Connect<ServiceInstanceName>()` if the
  `DriverBase` class is used.

  For more information, see
  [Use the DFv2 service discovery][use-service-discovery].

## How does my driver's node (or device) get exposed in the system in DFv2?

Drivers can [expose services][driver-communication] that can be routed to clients.
In addition, drivers, like all components, can be listed by using
`ffx component list`.

## What is not implemented in DFv2 that was available in DFv1?

If your DFv1 driver calls the [`load_firmware()`][load-firmware] function
in the DDK library, you need to implement your own since an equivalent
function is not available in DFv2. However, this is expected to be
[simple to implement][implement-firmware].

## What has changed in the bind rules in DFv2?

DFv2 nodes contain additional
[node properties generated from their FIDL service offers][use-node-properties].

However, it is unlikely that you will need to modify bind rules when
migrating an existing DFv1 driver to DFv2.

## What has changed in logging in DFv2?

DFv2 drivers cannot use the `zxlogf()` function or any debug library
that wraps or uses this function. `zxlogf()` is defined in
`//src/lib/ddk/include/lib/ddk/debug.h` and is removed from the
dependencies in DFv2. Drivers migrating to DFv2 need to
[stop using this library][use-dfv2-logger] and other libraries
that depend on it.

However, a new [compatibility library][logging-h], which is only
available in the Fuchsia source tree (`fuchsia.git`) environment, is
now added to allow DFv2 drivers to use DFv1-style logging.

## What has changed in inspect in DFv2?

DFv1 drivers use driver-specific inspect functions to create and update
driver-maintained metrics. For instance, in DFv1 the
`DeviceAddArgs::set_inspect_vmo()` function is called to indicate the
VMO that the driver uses for inspect. In DFv2, however, we can just
create an [`inspect::ComponentInspector`][use-dfv2-inspect] object.

## What do dispatchers do in DFv2?

A FIDL file generates templates and data types for a client-and-server
pair. Between these client and server ends is a channel, and the
dispatchers at each end fetch data from the channel. For more
information on dispatchers, see
[Driver dispatcher and threads][driver-dispatcher].

## What are some issues with the new threading model when migrating a DFv1 driver to DFv2?

FIDL calls in DFv2 are not on a single thread basis and are asynchronous
by design (although you can make them synchronous by adding `.sync()`
to FIDL calls or using `fdf::WireSyncClient`). Drivers are generally
discouraged from making synchronous calls because they can block other
tasks from running. (However, if necessary, a driver can create a
dispatcher with the `FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS` option,
which is only supported for
[synchronized dispatchers][synchronized-dispatchers].)

Given the differences in the threading models between Banjo (DFv1) and
FIDL (DFv2), you'll need to decide which kind of FIDL call (that is,
synchronous or asynchronous) you want to use while migrating. If your
original code is designed around the synchronous nature of Banjo and
is hard to unwind to make it all asynchronous, then you may want to
consider using the synchronous version of FIDL at first (which,
however, may result in performance degradation for the time being).
Later, you can revisit these calls and optimize them into using
synchronous calls.

## What has changed in testing drivers in DFv2?

The `mock_ddk` library, which is used in driver unit tests, is
specific to DFv1. [New testing libraries][update-unit-tests] are now
available for DFv2 drivers.

## Should I fork my driver into a DFv2 version while working on migration?

Forking an existing driver for migration depends on the complexity
of the driver. In general, it is recommended to avoid forking a
driver because it could end up creating more work. However,
for larger drivers, it may make sense to fork the driver into
a DFv2 version so that you can gradually land migration changes
in smaller patches.

You can fork a driver by adding a new driver component in the GN args
and use a flag to decide between the DFv1 or DFv2 version. This
[example CL][gc-msd-arm-mali]{:.external} demonstrates how a DFv2 fork
of the `msd-arm-mali` driver was added.

## What are some recommended readings?

The [DFv2 concept docs][driver-concepts] on fuchsia.dev and this
[Gerrit change][gc-intel-wifi]{:.external} from the previous DFv1
Intel WiFi driver migration (the
[`pcie-iwlwifi-driver.cc`][pcie-iwlwifi-driver-cc]{:.external} file
contains most of the new APIs).

<!-- Reference links -->

[migrate-from-banjo-to-fidl]: /docs/development/drivers/migration/migrate-from-banjo-to-fidl.md
[driver-dispatcher]: /docs/concepts/drivers/driver-dispatcher-and-threads.md
[driver-communication]: /docs/concepts/drivers/driver_communication.md
[driver-node]: /docs/concepts/drivers/drivers_and_nodes.md
[device-lifecycle]: /docs/development/drivers/concepts/device_driver_model/device-lifecycle.md#an_example_of_the_tear-down_sequence
[ddk-device-h-77]: https://source.corp.google.com/fuchsia/src/lib/ddk/include/lib/ddk/device.h;l=77
[ddk-driver-h-29]: https://source.corp.google.com/fuchsia/src/lib/ddk/include/lib/ddk/driver.h;l=29
[load-firmware]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/lib/ddk/include/lib/ddk/driver.h;l=416
[logging-h]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/compat/cpp/logging.h
[synchronized-dispatchers]: /docs/concepts/drivers/driver-dispatcher-and-threads.md#synchronized-and-unsynchronized
[gc-intel-wifi]:https://fuchsia-review.git.corp.google.com/c/fuchsia/+/692243
[pcie-iwlwifi-driver-cc]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/692243/47/src/connectivity/wlan/drivers/third_party/intel/iwlwifi/platform/pcie-iwlwifi-driver.cc
[codelab-driver-service]: /docs/get-started/sdk/learn/driver/driver-service.md
[logger-h]: https://source.corp.google.com/h/turquoise-internal/turquoise/+/main:sdk/lib/driver/logging/cpp/logger.h;l=15
[load-firmware]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/lib/ddk/include/lib/ddk/driver.h;l=408
[driver-concepts]: /docs/concepts/drivers/README.md
[gc-msd-arm-mali]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/853637/5/src/graphics/drivers/msd-arm-mali/BUILD.gn
[banjo-server-h]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/compat/cpp/banjo_server.h
[banjo-client-h]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/compat/cpp/banjo_client.h
[set-up-compat-device-server]: /docs/development/drivers/migration/set-up-compat-device-server.md
[driver-interfaces]: update-ddk-interfaces-to-dfv2.md
[update-dependencies]: update-ddk-interfaces-to-dfv2.md#update-dependencies-from-ddk-to-dfv2
[update-dep-for-compat-shim]: update-other-services-to-dfv2.md#update-dependencies-for-the-compatibility-shim
[update-driver-interfaces]: update-ddk-interfaces-to-dfv2.md#update-interfaces-from-ddk-to-dfv2
[use-service-discovery]: update-other-services-to-dfv2.md#use-the-dfv2-service-discovery
[update-component-manifests]: update-other-services-to-dfv2.md#update-component-manifests-of-other-drivers
[use-dispatchers]: update-other-services-to-dfv2.md#use-dispatchers
[use-dfv2-inspect]: update-other-services-to-dfv2.md#use-the-dfv2-inspect
[use-dfv2-logger]: update-other-services-to-dfv2.md#use-the-dfv2-logger
[implement-firmware]: update-other-services-to-dfv2.md#implement-your-own-load-firmware-method
[use-node-properties]: update-other-services-to-dfv2.md#use-the-node-properties-generated-from-fidl-service-offers
[update-unit-tests]: update-other-services-to-dfv2.md#update-unit-tests-to-dfv2
[additional-resources]: update-other-services-to-dfv2.md#additional-resources
[serve-banjo-protocols-in-a-dfv2-driver]: /docs/development/drivers/migration/serve-banjo-protocols.md
