keywords: category:FuchsiaHardwareAndDrivers,category:FuchsiaTestAndDebug,docType:ApiReference,category:FuchsiaFidl
keywords_public: Banjo, FIDL, Migration, Driver runtime, DFv1, Driver host, Dispatchers, Threading model, Unit tests, Fuchsia
description: This document provides answers to frequently asked questions regarding the migration of Fuchsia drivers from the Banjo interface definition language to FIDL, covering topics such as the driver runtime, threading models, and unit testing.
<!-- These keywords are for search widget on fuchsia.dev. Do not remove. -->

# Frequently asked questions for Banjo-to-FIDL migration

Before you start working on Banjo-to-FIDL migration, the frequently asked
questions below can help you identify special conditions or edge cases
that may apply to your driver.

## What is the difference between Banjo and FIDL?

Banjo is a "transpiler" (similar to `fidlc` in FIDL). It converts
an interface definition language (IDL) into target language specific files.
[FIDL][fidl] is the inter-process communication (IPC) system for Fuchsia.

## What is new in the driver runtime?

Drivers may talk to entities like the driver framework, other drivers and,
non-driver components. Among the drivers in the same driver host,
communication can occur using the FIDL bindings backed by the driver runtime
transport primitives (that is, arena, channel and dispatcher). This new
flavor of FIDL is called _driver runtime FIDL_. The driver runtime FIDL
enables the drivers to realize the advantages that the new driver runtime
provides, which includes stable ABI, thread safety, performance, driver
author ergonomics, security and resilience. (For more information, see
[this RFC][driver-runtime-rfc].)

## Do I need to migrate my DFv1 driver to use the driver runtime?

When migrating a DFv1 driver from Banjo to FIDL, **the driver runtime
migration is needed only if** your driver talks to other drivers
co-located in the same driver host. (see _How do I know if my driver
talks to other drivers co-located in the same process?_ below).

One major advantage of migrating a driver to use the new driver runtime
is that it changes the way that the driver communicates with co-located
drivers, which is done by using the driver runtime FIDL. However, before
you can start migrating a driver to use the driver runtime, if your driver
is using Banjo or is already using FIDL but it's based on the original
transport (Zircon primitives), you first need to make changes so that all
communications in the driver take place using FIDL.

The good news is that the syntax of the driver runtime FIDL is similar to
FIDL [C++ wire bindings][cpp-bindings]. The only difference is that there are
some additional parameters in the function calls. And the namespace of some
classes or primitives it uses is `fdf` instead of the original one (for example,
`fdf::WireServer`), but FIDL wire binding types are still used in data
transactions (for example, `fidl::VectorView`).

## How do I know if my driver talks to other drivers co-located in the same process?

To find out whether your driver talks to other drivers co-located in the
same process (in which case you need to [migrate the driver to use the
driver runtime](#update-the-dfv1-driver-to-use-the-driver-runtime)), check
the component manifest file (`.cml`) of the driver and look for the
`colocate` field, for example:

```none {:.devsite-disable-click-to-copy}
program: {
  runner: "driver",
  compat: "driver/wlansoftmac.so",
  bind: "meta/bind/wlansoftmac.bindbc",
  {{ '<strong>' }}colocate: "true",{{ '</strong>' }}
  default_dispatcher_opts: [ "allow_sync_calls" ],
},
use: [
  { service: "fuchsia.wlan.softmac.Service" },
],
```
(Source: [`wlansoftmac.cml`][wlanofmac-cml])

If the `colocate` field is `true`, this driver talks to other drivers
co-located in the same process.

To check which drivers are co-located together, you can run the
`ffx driver list-hosts` command, for example:

```none {:.devsite-disable-click-to-copy}
$ ffx driver list-hosts
...
Driver Host: 11040
  fuchsia-boot:///#meta/virtio_netdevice.cm
  fuchsia-boot:///network-device#meta/network-device.cm

Driver Host: 11177
  fuchsia-boot:///#meta/virtio_input.cm
  fuchsia-boot:///hid#meta/hid.cm
  fuchsia-boot:///hid-input-report#meta/hid-input-report.cm

Driver Host: 11352
  fuchsia-boot:///#meta/ahci.cm

...
```

Co-located drivers share the same driver host. In this example, the
`virtio_netdevice.cm` and `network-device.cm` drivers are co-located.

## When do I need to use dispatchers?

Creating threads in drivers is not encouraged. Instead, drivers need to
use [dispatchers][driver-dispatcher]. Dispatchers are virtual threads that
get scheduled on the driver runtime thread pool. A FIDL file generates
client and server templates and data types, and in the middle of these
client and server ends is a channel where dispatchers at each end fetch
data from the channel.

Dispatchers are specific to the driver runtime, which is independent of
DFv1 and DFv2. Dispatchers are primarily used for FIDL communication,
although they can have other uses such as waiting on
interrupts from the kernel.

## What are some issues with the new threading model when migrating a DFv1 driver from Banjo to FIDL?

FIDL calls are not on a single thread basis and are asynchronous by design
(although you can make them synchronous by adding `.sync()` to FIDL calls
or using `fdf::WireSyncClient`). Drivers are generally discouraged from
making synchronous calls because they can block other tasks from running.
(However, if necessary, a driver can create a dispatcher with the
`FDF_DISPATCHER_OPTION_ALLOW_SYNC_CALLS` option, which is only supported
for [synchronized dispatchers][synchronized-dispatchers].)

While migrating from Banjo to FIDL, the first problem you'll likely
encounter is that, unlike FIDL, Banjo translates IDL (Interface
Definition Language) into structures consisting of function pointers or
data types. So in essence, bridging drivers with Banjo means bridging
drivers with synchronous function calls.

Given the differences in the threading models between Banjo and FIDL,
you'll need to decide which kind of FIDL call (that is, synchronous or
asynchronous) you want to use while migrating. If your original code is
designed around the synchronous nature of Banjo and is hard to unwind to
make it all asynchronous, then you may want to consider using the
synchronous version of FIDL at first (which, however, may result in
performance degradation for the time being). Later, you can revisit these
calls and optimize them into using asynchronous calls.

## What changes do I need to make in my driver's unit tests after migrating from Banjo to FIDL?

If there are unit tests based on the Banjo APIs for your driver, you'll
need to create a mock FIDL client (or server depending on whether you're
testing server or client) in the test class. For more information, see
[Update the DFv1 driver's unit tests to use FIDL](#update-the-dfv1-drivers-unit-tests-to-use-fidl).

<!-- Reference links -->

[fidl]: /docs/concepts/fidl/overview.md
[migrate-from-dfv1-to-dfv2]: /docs/development/drivers/migration/migrate-from-dfv1-to-dfv2.md
[driver-runtime-rfc]: /docs/contribute/governance/rfcs/0126_driver_runtime.md
[cpp-bindings]: /docs/reference/fidl/bindings/cpp-bindings.md
[synchronized-dispatchers]: /docs/concepts/drivers/driver-dispatcher-and-threads.md#synchronous-operations
[wlanofmac-cml]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/connectivity/wlan/drivers/wlansoftmac/meta/wlansoftmac.cml
[driver-dispatcher]: /docs/concepts/drivers/driver-dispatcher-and-threads.md
[update-banjo-to-fidl]: convert-banjo-protocols-to-fidl-protocols.md#update-the-dfv1-driver-from-banjo-to-fidl
[update-driver-runtime]: convert-banjo-protocols-to-fidl-protocols.md#update-the-dfv1-driver-to-use-the-driver-runtime
[update-non-default-dispatchers]: convert-banjo-protocols-to-fidl-protocols.md#update-the-dfv1-driver-to-use-non-default-dispatchers
[update-two-way-communication]: convert-banjo-protocols-to-fidl-protocols.md#update-the-dfv1-driver-to-use-two-way-communication
[update-unit-tests]: convert-banjo-protocols-to-fidl-protocols.md#update-the-dfv1-drivers-unit-tests-to-use-fidl
[additional-resources]: convert-banjo-protocols-to-fidl-protocols.md#additional-resources
