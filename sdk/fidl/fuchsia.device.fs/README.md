# Fuchsia DevFS

DevFS (Device File System) provides a mechanism for non-driver components and tools
 to communicate with drivers.  There are two mechanisms for interacting with a driver
 through DevFS, the Controller interface, defined in
 [controller.fidl](/sdk/fidl/fuchsia.device.fs/controller.fidl), and the device protocol,
 which is an interface the device chooses to provide through devfs.


Devfs presents as a topological graph, where devices can be accessed either through the `/dev/sys` folder,
 which is a topological graph of drivers, or through the `/dev/class` folder,
 which organizes drivers by the protocol each device has chosen to make available to devfs.
whichever way the device is accessed, it presents as a directory with two children:
“device_protocol” and “device_controller”.  These names are defined in names.fidl.
Opening either file will provide a handle to the named interface.
The directory itself can also be opened as an interface to the device protocol.

## Restrictions and Future Work

An effort will begin in 2024 to deprecate devfs.  The deprecation order will be as follows:
 1) Migrate away from using the fuchsia.device/Controller interface
 2) Remove the use of topological paths
 3) Migrate clients from using /dev/class to the aggregated service connection
As part of this migration, usage of the fuchsia.device/Controller interface is being tracked
 and restricted.  Unfortunately, because the devfs capability is broadly offered to components,
 devfs access is restricted by device, but changes to access are mostly on the client end.

There are two restrictions that are in place to track and limit usage of the fuchsia.device/Controller interface.
### Controller Support
To allow clients to use any part of the fuchsia.device/Controller interface, a DFv2 device must be added with the DevfsAddArgs:
```
auto devfs = fuchsia_driver_framework::wire::DevfsAddArgs::Builder(arena)
    .connector(std::move(connector.value()))
    .connector_supports(fuchsia_device_fs::ConnectionType::kController)
    .class_name(CLASS_NAME);
```
where `CLASS_NAME` is a string that can be found in [protodefs.h](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/lib/ddk/include/lib/ddk/protodefs.h)
and will allow the interface served by the connector to show up in the `/dev/class/CLASS_NAME` directory.

The `DevfsAddArgs` are then added to the `NodeAddArgs` when adding children.
If you are migrating a driver from the legacy driver framework, omitting the `connector_supports` argument will cause the following error:
```
Connection to <DEVICE_NAME> controller interface failed, as that node did not include controller support in its DevAddArgs
```
if a client attempts to connect to the controller interface. **Please do not add controller support unless a client is failing with this error.**

### Per-Function Restrictions
Each function of the fuchsia.device/Controller interface also has an allowlist.  The allowlist, as well as accompanying logic is located here:
[controller_allowlist_passthrough.cc](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/bin/driver_manager/controller_allowlist_passthrough.cc)

Migrating a driver from the legacy framework should not require changing these allowlists,
unless the name of the added device changes as part of the migration.  If you get the following error:
```
Undeclared DEVFS_USAGE detected: <FUNCTION_NAME> is using <DRIVER_STRING>.
This error is thrown when a fuchsia.device/Controller client
attempts to access a device and function not registered at
src/devices/bin/driver_manager/controller_allowlist_passthrough.cc.
If you need to add to the allowlist, add a bug as a child under https://fxbug.dev/331409420.
```
you can add the <DRIVER_STRING> provided in the assert to the appropriate allowlist. Please contact the
Driver Framework team if you are doing so.


