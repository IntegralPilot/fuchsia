# Update DDK interfaces to DFv2

This page provides best practices and examples for migrating driver interfaces
from DFv1 to DFv2.

DFv1 drivers use the DDK (Driver Development Kit) library ([`//src/lib/ddk`][ddk]).
In DFv2, this library is replaced with the [driver component][driver-component]
library, which includes most of the essential utilities for DFv2 drivers.

## Update dependencies from DDK to DFv2 {:#update-dependencies-from-ddk-to-dfv2}

Remove all package dependencies under the DDK library in your DFv1 driver and
replace them with the new DFv2 library,

Note: After updating dependencies from DDK to DFv2, your driver won't
compile until you complete the next
[Update interfaces from DDK to DFv2](#update-interfaces-from-ddk-to-dfv2)
section.

### Source headers

Replace DDK dependencies in source header files:

* {DFv1}

  ```none
  //sdk/lib/driver/component/cpp:cpp
  ```

* {DFv2}

  ```cpp
  #include <lib/driver/component/cpp/driver_base2.h>
  ```

### Build dependencies

Replace DDK dependencies in the `BUILD.gn` files:

* {DFv1}

  ```gn
  "//src/lib/ddk",
  "//src/lib/ddktl",
  ```

* {DFv2}

  ```gn
  "//sdk/lib/driver/component/cpp:cpp",
  ```

## Update interfaces from DDK to DFv2 {:#update-interfaces-from-ddk-to-dfv2}

### ddk:Device mixin

`ddk::Device<>` is a mixin defined in [`ddktl/device.h`][device-h] that
simplifies writing DDK drivers in C++.

* {DFv1}

  The example of a DFv1 driver below uses the `ddk::Device<>` mixin:

  ```cpp
  #include <ddktl/device.h>

  class SimpleDriver;
  using DeviceType = ddk::Device<SimpleDriver, ddk::Initializable>;

  class SimpleDriver : public DeviceType {
   public:
    explicit SimpleDriver(zx_device_t* parent);
    ~SimpleDriver();

    static zx_status_t Bind(void* ctx, zx_device_t* dev);

    void DdkInit(ddk::InitTxn txn);
    void DdkRelease();
  };

  }  // namespace simple
  ```

* {DFv2}

  In DFv2, the [`DriverBase2`][driver-base] class is used to write drivers.
  DFv2 drivers need to inherit from the `DriverBase2` class instead of the
  `ddk::Device` mixin, for example:

  ```cpp
  #include <lib/driver/component/cpp/driver_base2.h>

  class SimpleDriver : public fdf::DriverBase2 {
   public:
    SimpleDriver();

    zx::result<> Start(fdf::DriverContext context) override;
  };
  ```

### Constructor, init hook, and bind hook

In DFv1, the driver is initialized through the constructor, the init hook,
and the bind hook.

The init hook is typically implemented as `DdkInit()`, for example:

```cpp
void SimpleDriver::DdkInit(ddk::InitTxn txn) {}
```

The bind hook is a static function that is passed to the `zx_driver_ops_t`
struct, for example:

```cpp
zx_status_t SimpleDriver::Bind(void* ctx, zx_device_t* dev) {}

static zx_driver_ops_t simple_driver_ops = [][5] -> zx_driver_ops_t {
  zx_driver_ops_t ops = {};
  ops.version = DRIVER_OPS_VERSION;
  ops.bind = SimpleDriver::Bind;
  return ops;
}();
```

In DFv2, all of the DFv1 logic needs to go into the `DriverBase2` class's
`Start()` function in the following order: Constructor, Init hook, and
Bind hook.

* {DFv1}

  ```cpp
  #include <ddktl/device.h>

  class SimpleDriver;
  using DeviceType = ddk::Device<SimpleDriver, ddk::Initializable>;

  class SimpleDriver : public DeviceType {
   public:
    explicit SimpleDriver(zx_device_t* parent) {
       zxlogf(INFO, "SimpleDriver constructor logic");
    }
    ~SimpleDriver() { delete this; }

    static zx_status_t Bind(void* ctx, zx_device_t* dev) {
       zxlogf(INFO, "SimpleDriver bind logic");
    }

    void DdkInit(ddk::InitTxn txn) {
       zxlogf(INFO, "SimpleDriver initialized");
    }
  };

  }  // namespace simple

  static zx_driver_ops_t simple_driver_ops = [][5] -> zx_driver_ops_t {
    zx_driver_ops_t ops = {};
    ops.version = DRIVER_OPS_VERSION;
    ops.bind = SimpleDriver::Bind;
    return ops;
  }();
  ```

* {DFv2}

  ```cpp
  class SimpleDriver : public fdf::DriverBase2 {
   public:
    SimpleDriver() : fdf::DriverBase2("simple_driver") {}

    zx::result<> Start(fdf::DriverContext context) override {
       fdf::info("SimpleDriver constructor logic");
       fdf::info("SimpleDriver initialized");
       fdf::info("SimpleDriver bind logic");
       return zx::ok();
    }
  };
  ```

### Destructor, DdkSuspend(), DdkUnbind(), and DdkRelease()

In DFv1, the destructor, `DdkSuspend()`, `DdkUnbind()`, and `DdkRelease()` are
used for tearing down objects and threads (for more information on these hooks,
see this [DFv1 simple driver example][dfv1-simple-driver]).

In DFv2, the teardown logic needs to be implemented in the `Stop()` method (for asynchronous cleanup) or the destructor (for synchronous cleanup) of the `DriverBase2` class, for example:

```cpp
class SimpleDriver : public fdf::DriverBase2 {
 public:
  SimpleDriver() : fdf::DriverBase2("simple_driver") {}
  ~SimpleDriver() override {
     // Perform synchronous teardowns here
  }

  void Stop(fdf::StopCompleter completer) override {
     // Called before the driver dispatchers are shutdown. Only implement this function
     // if you need to manually clean up objects asynchronously before dispatchers shutdown.
     completer(zx::ok());
  }
};
```

Also, when migrating to DFv2, exclude self-delete statements in the `DdkRelease()`
methods, for example:

```cpp
void DdkRelease() {
    delete this; // Do not migrate this statement to DFv2
}
```

### ZIRCON_DRIVER() macro

In DFv1, a driver is declared through the `ZIRCON_DRIVER()` macro. In DFv2, the
[`driver_export`][driver-export] library replaces the `ZIRCON_DRIVER()` macro.
The driver export needs to be declared in the driver implementation, typically
in the `.cc` file.

* {DFv1}

  An example of the `ZIRCON_DRIVER()` macro:

  ```cpp
  static zx_driver_ops_t simple_driver_ops = [][5] -> zx_driver_ops_t {
    zx_driver_ops_t ops = {};
    ops.version = DRIVER_OPS_VERSION;
    ops.bind = SimpleDriver::Bind;
    return ops;
  }();

  ZIRCON_DRIVER(SimpleDriver, simple_driver_ops, "zircon", "0.1");
  ```

* {DFv2}

  The `driver_export` example below migrates the DFv1 `ZIRCON_DRIVER()` macro
  example:

  ```cpp
  #include <lib/driver/component/cpp/driver_export2.h>
  ...
  FUCHSIA_DRIVER_EXPORT2(simple::SimpleDriver);
  ```

### ddk::Messageable mixin

In DFv1, the `ddk::Messageable<>` mixin is used to implement a FIDL server.
In DFv2, the `ddk:Messageable<>` mixin can be replaced by directly inheriting
from the FIDL server.

* {DFv1}

  ```cpp
  using I2cDeviceType = ddk::Device<I2cChild, ddk::Messageable<fuchsia_hardware_i2c::Device>::Mixin>;

  class I2cDriver : public I2cDeviceType {
    ...
    // Functions from the fuchsia.hardware.i2c Device protocol
    void Transfer(TransferRequestView request, TransferCompleter::Sync& completer) override;
    void GetName(GetNameCompleter::Sync& completer) override;
  }
  ```

* {DFv2}

  ```cpp
  class I2cDriver : public DriverBase2, public fidl::WireServer<fuchsia_hardware_i2c::Device> {
    ...
    // Functions from the fuchsia.hardware.i2c Device protocol
    void Transfer(TransferRequestView request, TransferCompleter::Sync& completer) override;
    void GetName(GetNameCompleter::Sync& completer) override;
  }
  ```

After implementing the FIDL server, you need to add the service to the outgoing
directory:

1. Add a `ServerBindingGroup` to your driver class.

1. Use `fidl::ServerBindingGroup` for Zircon transport and
   `fdf::ServerBindingGroup` for driver transport, for example:

   ```cpp
   class I2cDriver: public DriverBase2 {
     ...
     private:
       fidl::ServerBindingGroup<fidl_i2c::Device> bindings_;
   }
   ```

1. Add the bindings to the outgoing directory, for example:

   ```cpp
   auto serve_result = outgoing()->AddService<fuchsia_hardware_i2c::Service>(
         fuchsia_hardware_i2c::Service::InstanceHandler({
             .device = bindings_.CreateHandler(this, dispatcher()->async_dispatcher(),
                 fidl::kIgnoreBindingClosure),
         }));
   if (serve_result.is_error()) {
       fdf::error("Failed to add Device service: {}", serve_result);
       return serve_result.take_error();
   }
   ```

### DdkAdd() and device_add()

The `DdkAdd()` and `device_add()` methods are used to add child nodes,
for example:

```cpp
zx_status_t status = device->DdkAdd(ddk::DeviceAddArgs("i2c");
```

To add a child node in DFv2, see the [Add a child node][add-a-child-node]
section in the _Write a minimal DFv2 driver_ guide.

### DdkAddCompositeNodeSpec()

The `DdkAddCompositeNodeSpec()` method adds a composite node spec to
the system, for example:

```cpp
auto status = DdkAddCompositeNodeSpec("ft3x27_touch", spec);
```

For instructions on how to add a composite node spec in DFv2,
see the [Create a composite node][composite-nodes] guide.

### device_get_protocol()

The `device_get_protocol()` method is used to retrieve and connect to
a Banjo server. In DFv2, we replace the call with the
[`compat/cpp/banjo_client`][banjo-client] library.

* {DFv1}

  ```cpp
  misc_protocol_t misc;
  zx_status_t status = device_get_protocol(parent, ZX_PROTOCOL_MISC, &misc);
  if (status != ZX_OK) {
    return status;
  }
  ```

* {DFv2}

  ```cpp
  zx::result<ddk::MiscProtocolClient> client =
       compat::ConnectBanjo<ddk::MiscProtocolClient>(incoming);
  ```

### FIDL connect functions

In DFv1, DDK provides the following functions for connecting to FIDL protocols:

- [`DdkConnectFidlProtocol()`](#ddkconnectfidlprotocol)
- [`DdkConnectFragmentFidlProtocol()`](#ddkconnectfragmentfidlprotocol)
- [`DdkConnectRuntimeProtocol()`](#ddkconnectruntimeprotocol)
- [`DdkConnectFragmentRuntimeProtocol()`](#ddkconnectfragmentruntimeprotocol)

In DFv2, you use the [driver namespace][driver-namespace] library to connect
to the FIDL protocols.

#### DdkConnectFidlProtocol()

* {DFv1}

  ```cpp
  zx::result<fidl::ClientEnd<fuchsia_examples_gizmo::Device>> client_end =
    DdkConnectFidlProtocol<fuchsia_examples_gizmo::Service::Device>();
  if (client_end.is_error()) {
    zxlogf(ERROR, "Failed to connect fidl protocol");
    return client_end.status_value();
  }
  ```

  (Source: [DFv1 driver transport example][dfv1-driver-transport-zircon])

* {DFv2}

  ```cpp
  zx::result<fidl::ClientEnd<fuchsia_examples_gizmo::Device>> connect_result =
      context.incoming().Connect<fuchsia_examples_gizmo::Service::Device>();
  if (connect_result.is_error()) {
    fdf::error("Failed to connect gizmo device protocol: {}", connect_result);
    return connect_result.take_error();
  }
  ```

  (Source: [DFv2 driver transport example][dfv2-driver-transport-zircon])

#### DdkConnectFragmentFidlProtocol()

* {DFv1}

  ```cpp
  zx::result<fidl::ClientEnd<fuchsia_examples_gizmo::Device>> client_end =
    DdkConnectFragmentFidlProtocol<fuchsia_examples_gizmo::Service::Device>(parent, "gizmo");
  if (codec_client_end.is_error()) {
    zxlogf(ERROR, "Failed to connect to fidl protocol: %s",
        zx_status_get_string(codec_client_end.status_value()));
    return codec_client_end.status_value();
  }
  ```

* {DFv2}

  ```cpp
  zx::result<fidl::ClientEnd<fuchsia_examples_gizmo::Device>> connect_result =
      context.incoming().Connect<fuchsia_examples_gizmo::Service::Device>("gizmo");
  if (connect_result.is_error()) {
    fdf::error("Failed to connect gizmo device protocol: {}", connect_result);
    return connect_result.take_error();
  }
  ```

#### DdkConnectRuntimeProtocol()

* {DFv1}

  ```cpp
  zx::result<fidl::ClientEnd<fuchsia_examples_gizmo::Device>> client_end =
      DdkConnectRuntimeProtocol<fuchsia_examples_gizmo::Service::Device>();
  if (client_end.is_error()) {
    zxlogf(ERROR, "Failed to connect fidl protocol");
    return client_end.status_value();
  }
  ```

  (Source: [DFv1 driver transport example][dfv1-driver-transport-driver])

* {DFv2}

  ```cpp
  zx::result<fidl::ClientEnd<fuchsia_examples_gizmo::Device>> connect_result =
      context.incoming().Connect<fuchsia_examples_gizmo::Service::Device>();
  if (connect_result.is_error()) {
    fdf::error("Failed to connect gizmo device protocol: {}", connect_result);
    return connect_result.take_error();
  }
  ```

  (Source: [DFv2 driver transport example][dfv2-driver-transport-driver])

#### DdkConnectFragmentRuntimeProtocol()

* {DFv1}

  ```cpp
  zx::result<fidl::ClientEnd<fuchsia_examples_gizmo::Device>> client_end =
      DdkConnectFragmentRuntimeProtocol<fuchsia_examples_gizmo::Service::Device>("gizmo_parent");
  if (client_end.is_error()) {
    zxlogf(ERROR, "Failed to connect fidl protocol");
    return client_end.status_value();
  }
  ```

* {DFv2}

  ```cpp
  zx::result<fidl::ClientEnd<fuchsia_examples_gizmo::Device>> connect_result =
      context.incoming().Connect<fuchsia_examples_gizmo::Service::Device>("gizmo_parent");
  if (connect_result.is_error()) {
    fdf::error("Failed to connect gizmo device protocol: {}", connect_result);
    return connect_result.take_error();
  }
  ```

### Metadata

In DFv1, drivers can add and retrieve metadata using functions such as
`DdkAddMetadata()` and `ddk::GetEncodedMetadata<>()`. See the code examples
below.

To migrate these metadata functions to DFv2, do the following:

1. Complete the [Set up a compat device server][set-up-a-compat-device-server]
   guide.

1. Follow the instructions in the
   [Forward, add, and parse DFv1 metadata][forward-add-and-parse] section.

#### DdkAddMetadata()

* {DFv1}

  ```cpp
  zx_status_t status = dev->DdkAddMetadata(DEVICE_METADATA_PRIVATE,
      metadata.value().data(), sizeof(metadata.value());
  if (status != ZX_OK) {
    zxlogf(ERROR, "DdkAddMetadata failed: %s", zx_status_get_string(status));
  }
  ```

* {DFv2}

  ```cpp
  compat_server->inner().AddMetadata(DEVICE_METADATA_PRIVATE,
      metadata.value().data(), sizeof(metadata.value()));
  ```

#### ddk::GetEncodedMetadata<>()

* {DFv1}

  ```cpp
  auto decoded =
      ddk::GetEncodedMetadata<fuchsia_hardware_i2c_businfo::wire::I2CBusMetadata>(
         parent, DEVICE_METADATA_I2C_CHANNELS);
  ```

* {DFv2}

  ```cpp
  fidl::Arena arena;
  zx::result i2c_bus_metadata =
      compat::GetMetadata<fuchsia_hardware_i2c_businfo::wire::I2CBusMetadata>(
          incoming, arena, DEVICE_METADATA_I2C_CHANNELS);
  ```

## Migrate other interfaces {:#migrating-other-interfaces}

### TRACE_DURATION

The `TRACE_DURATION()` method is available for both DFv1 and DFv2.

* {DFv1}

  In DFv1, the `TRACE_DURATION` macro is imported from the following DDK
  library:

  ```cpp
  #include <lib/ddk/trace/event.h>
  ```

* {DFv2}

  To use the `TRACE_DURATION` macro in DFv2, change the `include` line to the
  following header:

  ```cpp
  #include <lib/trace/event.h>
  ```

  And add the following line to the build dependencies:

  ```cpp
  fuchsia_cc_driver("i2c-driver") {
    ...
    deps = [
      ...
      "//zircon/system/ulib/trace",
    ]
  }
  ```

### zxlogf()

The `zxlogf()` method is not available in DFv2 and needs to be migrated to
`fdf::info()`, `fdf::error()`, etc.

For instructions, see the [Add logs][add-logs] section in the
_Write a minimal DFv2 driver_ guide.

<!-- Reference links -->

[ddk]: /docs/development/drivers/concepts/driver_development/using-ddktl.md
[driver-component]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/component/cpp/
[device-h]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/lib/ddktl/include/ddktl/device.h
[driver-base]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/component/cpp/driver_base.h
[dfv1-simple-driver]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/simple/dfv1/simple_driver.cc
[driver-export]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/component/cpp/driver_export.h
[add-a-child-node]: /docs/development/drivers/developer_guide/write-a-minimal-dfv2-driver.md#add-a-child-node
[composite-nodes]: /docs/development/drivers/developer_guide/create-a-composite-node.md
[banjo-client]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/compat/cpp/include/lib/driver/compat/cpp/banjo_client.h
[driver-namespace]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/incoming/cpp/namespace.h
[dfv1-driver-transport-zircon]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/transport/zircon/v1/child-driver.cc
[dfv2-driver-transport-zircon]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/transport/zircon/v2/child-driver.cc
[dfv1-driver-transport-driver]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/transport/driver/v1/child-driver.cc
[dfv2-driver-transport-driver]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/transport/driver/v2/child-driver.cc
[set-up-a-compat-device-server]: /docs/development/drivers/migration/set-up-compat-device-server.md#set-up-the-compat-device-server
[forward-add-and-parse]: /docs/development/drivers/migration/set-up-compat-device-server.md#forward-add-and-parse-dfv1-metadata
[add-logs]: /docs/development/drivers/developer_guide/write-a-minimal-dfv2-driver.md#add-logs
