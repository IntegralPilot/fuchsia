# Set up the compat device server in a DFv2 driver

Important: Setting up the compat device server is **required** in all
DFv1-to-DFv2 driver migration.

This guide provides instructions on how to set up and use the compat device
server in a DFv2 driver for communicating with DFv1 drivers.

The compat device server helps DFv2 drivers maintain compatibility with
DFv1 drivers during the migration process. The compat device server offers the
`fuchsia_driver_compat::Device` interface (see [`compat.fidl`][compat-fidl]).
This interface allows DFv2 drivers to provide their resources to descendant
DFv1 drivers.

The key features of the compat device server are:

- **Resource sharing:** Provide resources from your DFv2 driver to
  descendant DFv1 drivers seamlessly.

- **Banjo services:** Offer Banjo protocols implemented in your DFv2 driver
  to DFv1 drivers.

- **Metadata handling:** Forward, add, and parse DFv1 metadata for
  communication between DFv2 and DFv1 drivers.

This guide provides step-by-step instructions and examples to help you with the
following tasks:

- [Set up the compat device server](#set-up-the-compat-device-server): Set up
  the compat device server in a DFv2 driver, including initialization
  (synchronous or asynchronous) and offering it to the child nodes.

- [Provide Banjo services to descendant DFv1 drivers](#provide-banjo-services-to-descendant-dfv1-drivers):
  Configure and share Banjo protocols with descendant DFv1 drivers.

- [Forward, add, and parse DFv1 metadata](#forward-add-and-parse-dfv1-metadata):
  Add, forward, and retrieve metadata, ensuring seamless information exchange
  between DFv2 and DFv1 drivers.

## Set up the compat device server {:#set-up-the-compat-device-server}

This section provides the compat device server setup instructions based
on the [DFv2 Simple driver example][dfv2-simple-driver-example]{:.external}.

The steps are:

1. [Specify the compat device server as dependencies](#specify-the-compat-device-server-as-dependencies).
2. [Initialize the compat device server synchronously or asynchronously](#initialize-the-compat-device-server-synchronously-or-asynchronously).
3. [Offer the compat device server to the target child node](#offer-the-compat-device-server-to-the-target-child-node).

### 1. Specify the compat device server as dependencies {:#specify-the-compat-device-server-as-dependencies}

To specify the compat device server as dependencies in your DFv2 driver,
do the following:

1. Add the following dependency to your `fuchsia_driver` target in
   [`BUILD.gn`][dfv2-simple-driver-build-gn]{:.external}:

   ```gn
   "//sdk/lib/driver/compat/cpp",
   ```

2. Include the [`device_server.h`][device-server-h] header in your driver's
   source file:

   ```cpp
   #include <lib/driver/compat/cpp/device_server.h>
   ```

3. Include the `driver_component/driver.shard.cml` shard in your driver's
   component manifest (`.cml`), for example:

   ```none {:.devsite-disable-click-to-copy}
    include: [
        "driver_component/driver.shard.cml",
        "inspect/client.shard.cml",
        "syslog/client.shard.cml",
    ],
   ```
   (Source: [`simple_driver.cml`][dfv2-simple-driver-cml])

   This shard defines the `use`, `capabilities`, and `expose` fields for
   `fuchsia.driver.compat.Service`.

With this setup, your DFv2 driver can now create a `DeviceServer` object for
a descendant node. Each `DeviceServer` object must have only one targeted node.

### 2. Initialize the compat device server synchronously or asynchronously {:#initialize-the-compat-device-server-synchronously-or-asynchronously}

Initialize a `DeviceServer` object synchronously or asynchronously. By default,
it is recommended to use synchronous over asynchronous initialization.

However, you may consider using
[asynchronous initialization](#asynchronous-initialization) if you meet one of
the following criteria:

- Synchronous or blocking calls on the current dispatcher are not allowed.

- Your DFv1 driver is already structured for asynchronous code and you need
  the performance gains from asynchronous behavior.

#### Synchronous initialization {:#synchronous-initialization}

For synchronous initialization, do the following:

1. Add a `SyncInitializedDeviceServer` object to the class, for example:

   ```cpp {:.devsite-disable-click-to-copy}
   class SimpleDriver : public fdf::DriverBase2 {
    ...
       compat::SyncInitializedDeviceServer compat_server_;
    ...
   }
   ```

2. In the driver implementation, call this `DeviceServer` object's
   `Initialize()` function:

   ```cpp {:.devsite-disable-click-to-copy}
   zx::result<> Initialize(const std::shared_ptr<fdf::Namespace>& incoming,
                             const std::shared_ptr<fdf::OutgoingDirectory>& outgoing,
                             const std::optional<std::string>& node_name,
                             std::string_view child_node_name,
                             const ForwardMetadata& forward_metadata = ForwardMetadata::None(),
                             std::optional<DeviceServer::BanjoConfig> banjo_config = std::nullopt,
                             const std::optional<std::string>& child_additional_path = std::nullopt);
   ```

   See below for the parameters of this function:

   - The value of `incoming` can be passed by taking and converting the incoming
     namespace from `DriverContext` during `Start`. The value of `outgoing` can
     be passed by invoking `DriverBase2`'s `outgoing()` accessor. The value of
     `node_name` can be passed by invoking `DriverContext`'s `node_name()` accessor.

   - `child_node_name` is the name of the targeted child node for this
     `DeviceServer` object.

     However, if there are any intermediary nodes between your DFv2 driver's node
     and the target child node, you need to set `child_additional_path` to the
     topological path between the nodes separated by `/`. For example, if there is
     `node-a` and then `node-b` before the target child node, then the value of
     `child_additional_path` needs to be `node-a/node-b/`.

   - `forward_metadata` contains information about the metadata to be forwarded
     from the parent nodes. (For more information, see
     [Forward metadata](#forward-metadata).)

   - `banjo_config` contains information for serving Banjo protocols to the target
     child node. However, if the device server is not serving a protocol, you can
     set the param to `std::nullopt`. (For more information, see
     [Provide Banjo services to descendant DFv1 drivers](#provide-banjo-services-to-descendant-dfv1-drivers).)

   The example below initializes a synchronous compat device server object:

   ```cpp {:.devsite-disable-click-to-copy}
   // Initialize our compat server.
     {
       zx::result<> result = compat_server_.Initialize(
          incoming(), outgoing(), node_name(), child_name);
       if (result.is_error()) {
         return result.take_error();
       }
     }
   ```

#### Asynchronous initialization {:#asynchronous-initialization}

For asynchronous initialization, do the following:

1. Add an `AsyncInitializedDeviceServer` object to the class,
   for example:

   ```cpp {:.devsite-disable-click-to-copy}
   class SimpleDriver : public fdf::DriverBase2 {
    ...
       compat::AsyncInitializedDeviceServer compat_server_;
    ...
   }
   ```

2. In the driver implementation, call this ` DeviceServer` object's
   `Begin()` function:

   ```cpp {:.devsite-disable-click-to-copy}
   void Begin(const std::shared_ptr<fdf::Namespace>& incoming,
                const std::shared_ptr<fdf::OutgoingDirectory>& outgoing,
                const std::optional<std::string>& node_name, std::string_view child_node_name,
                fit::callback<void(zx::result<>)> callback,
                const ForwardMetadata& forward_metadata = ForwardMetadata::None(),
                std::optional<DeviceServer::BanjoConfig> banjo_config = std::nullopt,
                const std::optional<std::string>& child_additional_path = std::nullopt);
   ```

   The parameters are nearly identical to the
   [synchronous `Initialize()` function](#synchronous-initialization) above, except
   the `callback` field.

   The callback function is invoked when the `DeviceServer` object finishes
   initializing and is ready to be accessed. It's recommended that you use this
   callback function to add the child nodes. The other parameter values are
   populated the same way as synchronous initialization.

   The example below initializes an asynchronous compat device server object:

   ```cpp {:.devsite-disable-click-to-copy}
   void SimpleDriver::OnDeviceServerInitialized(zx::result<> device_server_init_result) {
      // Add the child nodes here
      async_completer_.value()(zx::ok());
      async_completer_.reset();
   }

   void SimpleDriver::Start(fdf::DriverContext context, fdf::StartCompleter completer) {
      auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
      async_completer_.emplace(std::move(completer));
      device_server_.Begin(incoming, outgoing(), context.node_name(), child_name,
      fit::bind_member<&SimpleDriver::OnDeviceServerInitialized>(this),
      compat::ForwardMetadata::Some({DEVICE_METADATA_PRIVATE}));
   }
   ```

### 3. Offer the compat device server to the target child node {:#offer-the-compat-device-server-to-the-target-child-node}

Once the compat device server is initialized, pass its offers to the target
child node.

However, if the target node is not the direct child, you need to pass the offer
to the next closest child in the chain of descendants. For instance, the node
topology shows `A -> B -> C -> D` and if node A is your current node, and node D
is the target node, then you need to pass the offer to node B, which is the
closest child node in the chain.

Use your `DeviceServer` object's `CreateOffers2()` function to set the `offers2`
field in the `NodeAddArgs` struct, for example:

```cpp {:.devsite-disable-click-to-copy}
auto properties = std::vector{fdf::MakeProperty(BIND_PROTOCOL, ZX_PROTOCOL_SERIAL_IMPL_ASYNC)};
zx::result child_result = AddChild(child_name, properties, compat_server_.CreateOffers2());
if (child_result.is_error()) {
  return child_result.take_error();
}
```

If there are any additional offers, add them to the `DeviceServer` object's
offers before setting the `offers2` field in the `NodeAddArgs` struct, for
example:

```cpp {:.devsite-disable-click-to-copy}
auto offers = compat_server_.CreateOffers2();
offers.push_back(fdf::MakeOffer2<fuchsia_hardware_serialimpl::Service>(child_name));

auto properties = std::vector{fdf::MakeProperty(BIND_PROTOCOL, ZX_PROTOCOL_SERIAL_IMPL_ASYNC)};
zx::result child_result = AddChild(child_name, properties, std::move(offers));
if (child_result.is_error()) {
  return child_result.take_error();
}
```

If your compat device server is initialized synchronously, you need to perform
this operation after the `SyncInitializedDeviceServer::Initialize()` function is
called. Otherwise, you need to do this in the callback function passed to the
`AsyncInitializedDeviceServer::Begin()` call.

## Provide Banjo services to descendant DFv1 drivers {:#provide-banjo-services-to-descendant-dfv1-drivers}

If your DFv2 driver implements a Banjo protocol and wants to provide it to a
target child node, you need to add the protocol to the compat device server.

Let's say your driver implements the `Misc` Banjo protocol, for example:

```cpp {:.devsite-disable-click-to-copy}
class ParentBanjoTransportDriver : public fdf::DriverBase2,
                                   public ddk::MiscProtocol<ParentBanjoTransportDriver> {
...
```

(The examples in this section are based on the
[Banjo transport example][banjo-transport-example].)

To add the `Misc` Banjo protocol to the compat device server, do the following:

1. Include the following header in the driver's source file:

   ```cpp
   #include <lib/driver/compat/cpp/banjo_server.h>
   ```

1. Create a [`compat::BanjoServer`][banjo-server-h] object for the Banjo protocol:

   ```cpp
   compat::BanjoServer banjo_server_{ZX_PROTOCOL_<NAME>, this, &<name>_protocol_ops_};
   ```

   In the template above, replace `NAME` with the protocol's name in upper case and
   `name` in lower case. So for the `Misc` Banjo protocol, the object looks like below:

   ```cpp {:.devsite-disable-click-to-copy}
   class ParentBanjoTransportDriver : public fdf::DriverBase2,
                                      public ddk::MiscProtocol<ParentBanjoTransportDriver> {
    ...

    private:
     compat::BanjoServer banjo_server_{ZX_PROTOCOL_MISC, this, &misc_protocol_ops_};
    ...
   }
   ```

1. Create a `BanjoConfig` object and set the protocol callback to the Banjo
   server's callback, for example:

   ```cpp {:.devsite-disable-click-to-copy}
   compat::DeviceServer::BanjoConfig banjo_config;
   banjo_config.callbacks[ZX_PROTOCOL_MISC] = banjo_server_.callback();
   ```

   This setup passes the Banjo configuration information from the
   `BanjoServer` object to the compat device server.

1. When you initialize the compat device server, set the `Initialize()`
   function's `banjo_config` field to the `BanjoConfig` object, for example:

   ```cpp {:.devsite-disable-click-to-copy}
   // Initialize our compat server.
   {
       zx::result<> result = compat_server_.Initialize(
          incoming(), outgoing(), node_name(), child_name, ForwardMetadata::None(),
           std::move(banjo_config));
       if (result.is_error()) {
         return result.take_error();
       }
   }
   ```

For the remaining steps on providing this Banjo protocol to descendant
DFv1 drivers, see the
[Serve Banjo protocols in a DFv2 driver][serve-banjo-protocols-in-a-dfv2-driver]
guide.

## Forward, add, and parse DFv1 metadata {:#forward-add-and-parse-dfv1-metadata}

Many existing DFv1 drivers use metadata to relay information between parents and
their children. In DFv2 drivers, you can use the compat device server to perform
the following operations:

- [Add and send metadata](#add-and-send-metadata)
- [Forward metadata](#forward-metadata)
- [Retrieve metadata](#retrieve-metadata)

### Add and send metadata {:#add-and-send-metadata}

Metadata is passed to child nodes through a driver's compat device server. To
add and send metadata, the driver needs to create a compat device server and
call its `AddMetadata()` function:

```cpp
zx_status_t AddMetadata(MetadataKey type, const void* data, size_t size);
```

(Source: [`device_server.h`][device-server-h])

The example below adds metadata using the `AddMetadata()` function:

```cpp {:.devsite-disable-click-to-copy}
const uint64_t metadata = 0xAABBCCDDEEFF0011;
zx_status_t status = compat_device_server.AddMetadata(
    DEVICE_METADATA_PRIVATE, &metadata, sizeof(metadata));
```

However, if the metadata is FIDL type, you need to make it persist with the
`fidl::Persist()` call first adding it to the `AddMetadata()` function,
for example:

```cpp {:.devsite-disable-click-to-copy}
fuchsia_hardware_i2c_businfo::wire::I2CChannel local_channel(channel);
fit::result metadata = fidl::Persist(local_channel);
if (!metadata.is_ok()) {
  fdf::error("Failed to fidl-encode channel: {}", metadata.error_value().FormatDescription());
  return zx::error(metadata.error_value().status());
}
compat_server->AddMetadata(DEVICE_METADATA_PRIVATE, metadata.value().data(),
                           metadata.value().size());

```

### Forward metadata {:#forward-metadata}

If your DFv2 driver receives metadata from the parent node and needs to pass
some or all of metadata to its child nodes, you can forward the metadata using
the compat device server.

To forward metadata, set the `forward_metadata` parameter when you initialize
the driver's `DeviceServer` object:

- If you want to forward all metadata, set the parameter to
  `ForwardMetadata::All()`, for example:

  ```cpp
  // In Start(fdf::DriverContext context, fdf::StartCompleter completer):
  // auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  //
  zx::result<> result = compat_server_.Initialize(
         incoming, outgoing(), context.node_name(), child_name,
         compat::ForwardMetadata::All());
  ```

- If you want to forward only some metadata, create a `ForwardMetadata` object
  with `ForwardMetadata Some(std::unordered_set<MetadataKey> filter)` and pass
  this object to the parameter.

  The example below forwards metadata with the `DEVICE_METADATA_GPT_INFO` key only:

  ```cpp
  // In Start(fdf::DriverContext context, fdf::StartCompleter completer):
  // auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  //
  zx::result<> result = compat_server_.Initialize(
         incoming, outgoing(), context.node_name(), child_name,
         compat::ForwardMetadata::Some({DEVICE_METADATA_GPT_INFO}));
  ```

- If you don't want to forward metadata, set the parameter to
  `ForwardMetadata::None()`, for example:

  ```cpp
  // In Start(fdf::DriverContext context, fdf::StartCompleter completer):
  // auto incoming = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  //
  zx::result<> result = compat_server_.Initialize(
         incoming, outgoing(), context.node_name(), child_name,
                                compat::ForwardMetadata::None());
  ```

### Retrieve metadata {:#retrieve-metadata}

The compat device server's metadata library ([`metadata.h`][metadata-h]) provides
helper functions for retrieving metadata from drivers.

To use this metadata library, do the following:

1. Add the following dependency to your `fuchsia_driver` target in `BUILD.gn`:

   ```cpp
   "//sdk/lib/driver/compat/cpp",
   ```

1. Include the following header in the driver's source file:

   ```cpp
   #include <lib/driver/compat/cpp/metadata.h>
   ```

1. To retrieve metadata from the driver's compat device server, use the
   `compat::GetMetadata<T>()` method and replace `T` with the metadata type.

   The example below retrieves metadata with the `DEVICE_METADATA_VREG` key
   and the `fuchsia_hardware_vreg::wire::Metadata` type:

   ```cpp {:.devsite-disable-click-to-copy}
   fidl::Arena arena;
   zx::result<fuchsia_hardware_vreg::wire::Metadata> metadata = compat::GetMetadata<fuchsia_hardware_vreg::wire::Metadata>(
             incoming, arena, DEVICE_METADATA_VREG);
   if (metadata.is_error()) {
     fdf::error("Failed to get metadata: {}", metadata);
     return completer(metadata.take_error());
   }
   ```

   For the complete list of metadata keys and types, see
   [this file][ddk-metadata-h].

1. **(Optional)** If the driver is composite, you need to pass the parent's name
   to specify which parent node the metadata is from, for example:

   ```cpp {:.devsite-disable-click-to-copy}
   zx::result<fuchsia_hardware_vreg::wire::Metadata> metadata = compat::GetMetadata<fuchsia_hardware_vreg::wire::Metadata>(
        incoming, arena, DEVICE_METADATA_VREG, "pdev");
   ```

However, if the metadata type is a dynamically sized array, use
`compat::GetMetadataArray<T>()` and replace `T` with the array's type.

Let's say we need to retrieve metadata for
`DEVICE_METADATA_AML_VOLTAGE_TABLE`, which is an array of the
`aml_voltage_table_t` structs:

```cpp {:.devsite-disable-click-to-copy}
#define DEVICE_METADATA_AML_VOLTAGE_TABLE (0x41565400 | (DEVICE_METADATA_PRIVATE))  // AVTp
```

Then you need to replace `T` with `aml_voltage_table_t` and retrieve the
metadata as shown below:

```cpp {:.devsite-disable-click-to-copy}
zx::result<std::vector<aml_voltage_table_t>>  voltage_table =
     compat::GetMetadataArray<aml_voltage_table_t>(
      incoming, DEVICE_METADATA_AML_VOLTAGE_TABLE);
 if (voltage_table.is_error()) {
   fdf::error("Failed to get voltage table metadata: {}", voltage_table);
   return zx::error(voltage_table.take_error());
 }
```

Likewise, if the driver is composite, pass the parent's name to specify which
parent node the metadata is from, for example:

```cpp {:.devsite-disable-click-to-copy}
zx::result<std::vector<aml_voltage_table_t>> voltage_table =
     compat::GetMetadataArray<aml_voltage_table_t>(
      incoming, DEVICE_METADATA_AML_VOLTAGE_TABLE, "pdev");
```

<!-- Reference links -->

[compat-fidl]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.driver.compat/compat.fidl
[dfv2-simple-driver-example]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/simple/dfv2/
[dfv2-simple-driver-build-gn]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/simple/dfv2/BUILD.gn
[dfv2-simple-driver-cml]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/simple/dfv2/meta/simple_driver.cml
[device-server-h]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/compat/cpp/device_server.h
[banjo-transport-example]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/transport/banjo/v2/
[banjo-server-h]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/compat/cpp/banjo_server.h
[metadata-h]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/compat/cpp/metadata.h
[ddk-metadata-h]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/lib/ddk/include/lib/ddk/metadata.h
[serve-banjo-protocols-in-a-dfv2-driver]: serve-banjo-protocols.md
