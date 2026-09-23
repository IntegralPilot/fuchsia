# Identifying service instances

This guide will help you migrate away from the anti-pattern of identifying driver
instances using a fixed moniker.

Specifically, this guide is for non-driver component clients that do any of the
following:

 - Use topological paths such as `/dev/sys/platform/ram-nand/nand-ctl`
 - Use a hardcoded class path (eg. /dev/class/camera/000)
 - Connect to the `device_controller` for a given devfs entry and call
   `GetTopologicalPath()`.

In each of the above situations, the client needs to be migrated from a
situation where the provider of the service/fidl interface was (or appeared to
be) known a priori and where the service was always assumed to exist to an
aggregated service, where instances will be added and removed asynchronously,
and the instance name cannot be used for identification.

## Option 1: Connect to the first service instance available

This is not the generally preferred solution, as it ignores the dynamic nature
of the system, and will have errors if multiple service instances exist.
However, this can be sufficient for simpler tools and tests where only one provider of
a service is expected. The following code waits indefinitely for a service
instance to appear:

* {C++}

    ```cpp
    SyncServiceMemberWatcher<fidl_example::Service::Device> watcher;
    zx::result<ClientEnd<fidl_example::Echo>> result = watcher.GetNextInstance(false);
    ```

* {Rust}

    ```rust
    use fuchsia_component::client::Service;
    let client = Service::open(fuchsia_example::EchoServiceMarker)?
      .watch_for_any()
      .await?
      .connect_to_device()?;
    ```

## Option 2: Query Information from each service instance

Instead of identifying the instance by connecting to it using a unique identifier,
clients should watch for the service that they are interested in, and then query
each instance for identifying information.  This information could come from the
existing protocol provided by the service, or a new protocol could be added to the
service to assist in identification.

### Use the existing fidl protocol provided by the service

Many fidl protocols specify information about the device already (for example,
the `fuchsia.camera/Control` protocol provides a `GetInfo` call.) If the
protocol being served does not currently contain identifying information,
it could be modified to add such device-specific information.

```fidl
library fuchsia.hardware.something;

protocol Device {
    ...
    GetDeviceId() -> (struct {
        device_id uint32;
    }) error zx.Status;
}
service Service {
    device client_end:Device;
};
```

### Add an additional protocol to the service

Add an additional `GetInfo` protocol to the service, that can contain the
device identifying information. For example:

```fidl
library fuchsia.hardware.something;

Protocol Device { ... }
Protocol GetInfo {
    GetDeviceId() -> (struct {
        device_id uint32;
    }) error zx.Status;
}
service Service {
    device client_end:Device;
    info   client_end:GetInfo;
};
```

Separating the identification interface from the main communication interface
has several advantages:

- The identification interface can be mostly handled by a driver library,
as it provides a static set of information.
- The identification interface would preferably handle multiple
simultaneous connections to aid in querying, whereas most drivers cannot
handle multiple simultaneous connections to their other interfaces.
- Separating the identification logic makes it easier for future work to
integrate identification steps into the framework.

If the topological information is needed for a node, there is already a protocol
to access that information, see the next section.

## Accessing the topological information for a service instance.

For ease in converting interfaces off of using `dev-topological` or using the
`fuchsia_device::Controller` interface to call `GetTopologicalPath`, there is
a tool to access the topological path of a devfs or service instance.  This tool
is designed as a step in migrating to services which provide a bus topology token
using the `fuchsia.driver.token.NodeToken` protocol.

Note: It is possible to skip the first step and migrate directly to using Bus
Topology. The first step is provided to decouple migrating off of devfs and
migrating to Bus Topology.

### Convert clients to services using the `GetTopologicalPath` tool {:#convert-services .numbered}

Using `fdf_topology::GetTopologicalPath` will allow you to stop accessing topological
paths directly, and allow you to convert your drivers to services

Note: Please do not connect to the `fuchsia.device.fs.TopologicalPath` protocol
directly.  This tool only is meant to allow migration to using `BusTopology`

Add your class name to the [allowlist][class-names]:

```cpp
  const std::list<std::string> kClassesThatAllowTopologicalPath({
    "block",
    "example_echo",
    "devfs_service_test",
  });
```

Add the device_topology dependency:

```gn
deps = [
  "//src/devices/lib/client:device_topology",
],
```

And call `GetTopologicalPath` using the service directory and instance name:

```cpp
  // Open the service directory
  zx::result dir = component::OpenDirectory("/svc/fuchsia.examples.EchoService");
  ZX_ASSERT(dir.status_value() == ZX_OK);
  // For simplicity in this example, just get the first instance in the service directory:
  auto watch_result = device_watcher::WatchDirectoryForItems<std::string>(
      *dir, [](std::string_view name) -> std::optional<std::string> { return std::string(name); });
  ZX_ASSERT(watch_result.status_value() == ZX_OK);
  std::string instance_name = std::move(watch_result.value());
  // Now check the topological path of that instance:
  zx::result<std::string> path_result = fdf_topology::GetTopologicalPath(*dir, instance_name);
  ZX_ASSERT(path_result.is_ok());
  if (path_result.value() == kSomePathYouWant) {
    //Open the instance
  }
```

Undergoing this step of the migration should be relatively mechanical, and will
allow your clients to stop using devfs.  The next two steps will require more
detailed knowledge of why your client is using topological paths.

### Add `NodeToken` to your protocol or service {:#add-nodetoken .numbered}

The recommended way to get information about bus topology is to use a `NodeToken`:


```fidl
library fuchsia.example;
using fuchsia.driver.token;

Protocol Device {
    compose fuchsia.driver.token.NodeToken;
}
service Service {
    device client_end:Device;
};
```

See [Bus Topology][bus-topology]  for more information.

### Migrate to using bus topology instead of a topological path {:#use-bus-topology .numbered}

The implementation here depends on why you need to use topological information.
For example, if you needed to connect to a specific device address on a bus, you
could match the bus type and address number.

Here is an example using ServiceMemberWatcher to filter for a specific topology:

```cpp
// Define a callback function to be called when the correct device is found
void OnInstanceFound(ClientEnd<fuchsia_examples::Echo> client_end) {}

// Define a filter for choosing the correct device:
void FilterOnBusTopology(ClientEnd<fuchsia_examples::Echo> client_end) {
    fidl::WireResult result = fidl::WireCall(client_end)->Get();
    ZX_ASSERT(result.ok());
    zx::result topo_client = component::Connect<fuchsia.driver.token.NodeBusTopology>();
    ZX_ASSERT(topo_client.is_ok());
    fidl::WireResult topo_result = fidl::WireCall(topo_client.value())->Get(result.value().value()->token.get());
    ZX_ASSERT(topo_result.ok());
    // Check topology:
    std::vector<fuchsia.driver.framework.BusInfo> bus_info
                = topo_result.value().value()->path.get();
    if (bus_info[0].bus() == fuchsia_driver_framework::BusType::I2C &&
        bus_info[0].address() == 3) {
            OnInstanceFound(std::move(client_end));
        }
}
// Optionally define an idle function, which will be called when all
// existing instances have been enumerated:
void AllExistingEnumerated() {...}
// Create the ServiceMemberWatcher:
ServiceMemberWatcher<fuchsia_examples::EchoService::MyDevice> watcher;
watcher.Begin(get_default_dispatcher(), &FilterOnBusTopology, &AllExistingEnumerated);
// If you want to stop watching for new service entries:
watcher.Cancel()
```

You will also need to route the `fuchsia.driver.token.NodeBusTopology` protocol
to your client from the Driver Manager.

Reach out for questions or for status updates:

*   <garratt@google.com>
*   <surajmalhotra@google.com>
*   <fuchsia-drivers-discuss@google.com>

<!-- Code links -->
[class-names]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/bin/driver_manager/devfs/class_names.h;l=48
[bus-topology]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.driver.token/node_bus_topology.fidl