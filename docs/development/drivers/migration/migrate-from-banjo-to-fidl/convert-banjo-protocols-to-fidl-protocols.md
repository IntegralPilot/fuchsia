# Convert Banjo protocols to FIDL protocols

This page provides instructions, best practices, and examples related to
converting Banjo protocols to FIDL protocols as part of DFv1-to-DFv2
driver migration.

## Update the DFv1 driver from Banjo to FIDL {:#update-the-dfv1-driver-from-banjo-to-fidl}

Note: There are two types of FIDL transport protocols that Fuchsia
drivers use: Driver transport and Zircon transport. Only Driver transport
is used to talk to other drivers co-located in the same driver host, and
importantly, **all Banjo protocols are expected to migrate to Driver
transport, not Zircon transport.**

Updating a driver's `.fidl` file is a good starting point for driver
migration because everything originates from the `.fidl` file. Fortunately,
Banjo and FIDL generate code from the same IDL (which is FIDL), so you
may not need to make significant changes to the existing `.fidl` file.

The example `.fidl` files below show the changes before and after the
Banjo-to-FIDL migration:

- **Before** (Banjo):
  [`//sdk/banjo/fuchsia.hardware.wlanphyimpl/wlanphy-impl.fidl`][wlanphy-impl-fidl]{:.external}
- **After** (FIDL): [`//sdk/fidl/fuchsia.wlan.phyimpl/phyimpl.fidl`][phyimpl-fidl]

To update your DFv1 driver from Banjo to FIDL, make the following changes
(mostly in the driver's `.fidl` file):

1. [Update attributes before protocol definitions](#update-attributes-before-protocol-definitions).
2. [Update function definitions to use FIDL error syntax](#update-function-definitions-to-use-fidl-error-sytntax).
3. [Update FIDL targets in BUILD.gn](#update-fidl-targets-in-buildgn).
4. [Move the FIDL file to the SDK/FIDL directory](#move-the-fidl-file-to-the-sdkfidl-directory).

### 1. Update attributes before protocol definitions {:#update-attributes-before-protocol-definitions}

To use Banjo, the `@transport("Banjo")` attribute is required in a `.fidl`
file. But this attribute is not necessary for FIDL (because FIDL is the
default). So you can just delete the `@transport("Banjo")` attribute from
your driver's `.fidl` file, for example:

- From using Banjo:

  ```{:.devsite-disable-click-to-copy}
  @transport("Banjo")
  @banjo_layout("ddk-protocol")
  protocol MyExampleProtocol {
   ...
  }
  ```

- To using FIDL:

  ```{:.devsite-disable-click-to-copy}
  @discoverable
  @transport("Driver")
  protocol MyExampleProtocol {
   ...
  }
  ```

  In the example above, the [`@discoverable`][discoverable] attribute
  is **required** for all FIDL protocols. This attribute allows the
  client to search this protocol using its generated name.

  However, the `@transport("Driver")` attribute (which indicates that
  this is a Driver transport protocol) is **optional** for
  drivers that use the [driver runtime FIDL](#update-the-dfv1-driver-to-use-the-driver-runtime).
  And driver runtime migration is only necessary if your driver talks
  to other drivers co-located in the same driver host.

  Note: To find out if driver runtime migration is needed, see the
  "How do I know if my driver talks to other drivers co-located in
  the same process?" question in the [Before you start](#before-you-start)
  section.

  For more examples on the Driver transport protocols, see this
  [Driver transport examples][driver-transport-example] directory.

### 2. Update function definitions to use FIDL error syntax {:#update-function-definitions-to-use-fidl-error-sytntax}

If some function definitions in the `.fidl` file include return status,
you need to update them to use FIDL error syntax instead of adding status
in the return structure, for example:

- From using a return structure:

  ```none {:.devsite-disable-click-to-copy}
  protocol MyExampleProtocol {
     MyExampleFunction() -> (struct {
         Error zx.status;
     });
  };
  ```

  (Source: [`wlanphy-impl.fidl`][wlanphy-impl-fidl]{:.external})

- To returning FIDL error syntax:

  ```none {:.devsite-disable-click-to-copy}
  protocol MyExampleProtocol {
     BMyExampleFunction() -> () error zx.status;
  };
  ```

  (Source: [`phyimpl.fidl`][phyimpl-fidl])

Using FIDL error syntax has the following benefits:

- The return structure can now focus on the data that needs to be sent
  to the server end.

- The error handling on the server end is cleaner because fetching the
  status does not require reading into the return structure.

### 3. Update FIDL targets in BUILD.gn {:#update-fidl-targets-in-buildgn}

Edit the `BUILD.gn` file (of this `.fidl` file) to add the following line
in the FIDL targets:

```none
contains_drivers = true
```

The example below shows the `contains_drivers = true` line in a FIDL target:

```{:.devsite-disable-click-to-copy}
import("//build/fidl/fidl.gni")

fidl("fuchsia.wlan.phyimpl") {
 sdk_category = "partner"
 sources = [ "phyimpl.fidl" ]
 public_deps = [
   "//sdk/fidl/fuchsia.wlan.common",
   "//sdk/fidl/fuchsia.wlan.ieee80211",
   "//zircon/vdso/zx",
 ]
 {{ '<strong>' }}contains_drivers = true{{ '</strong>' }}
 enable_banjo = true
}
```

(Source: [`BUILD.gn`][phyimpl-build-gn])

Without the `contains_drivers = true` line, the protocols with
`@transport("Driver")` attribute won't be generated into FIDL libraries
properly.

### 4. Move the FIDL file to the SDK/FIDL directory {:#move-the-fidl-file-to-the-sdkfidl-directory}

Move the updated `.fidl` file from the `sdk/banjo` directory to the
`sdk/fidl` directory.

The `sdk/fidl` directory is the default location for storing all files that
generate FIDL code. However, if Banjo structures or functions are still used
anywhere else (that is, outside of driver communication), moving this `.fidl`
file is **not allowed**. In that case, you can keep a copy of this `.fidl`
file in the `sdk/banjo` directory.

Note: To check whether your updated `.fidl` file can generate FIDL code
without any errors, open [fidlbolt][fidlbolt]{:.external} on a browser and
paste the content of the `.fidl` file.

## (Optional) Update the DFv1 driver to use the driver runtime {:#update-the-dfv1-driver-to-use-the-driver-runtime}

Important: Driver runtime migration is required **only if** your driver
talks to other drivers co-located in the same driver host. To find out
if the driver runtime migration is needed, see the "How do I know if
my driver talks to other drivers co-located in the same process?" question
in the [Before you start](#before-you-start) section.

This section requires that the updated `.fidl` file from the
[previous section](#update-the-dfv1-driver-from-banjo-to-fidl) can
successfully generate FIDL code. This FIDL code is used to establish
the driver runtime FIDL communication between drivers.

To update a DFv1 driver to use the driver runtime, the steps are:

1. [Update dependencies for the driver runtime](#update-dependencies-for-the-driver-runtime).
1. [Set up client and server objects for the driver runtime FIDL](#set-up-client-and-server-objects-for-the-driver-runtime-fidl).
1. [Update the driver to use the driver runtime FIDL](#update-the-driver-to-use-the-driver-runtime-fidl).
1. [Make FIDL requests](#make-fidl-requests).

### 1. Update dependencies for the driver runtime {:#update-dependencies-for-the-driver-runtime}

Update the server and client ends to include new dependencies for using
the driver runtime:

1. In the `BUILD.gn` file, update the dependencies fields to include the
   following lines:

   ```none
   //sdk/fidl/<YOUR_FIDL_LIB>_cpp_wire
   //sdk/fidl/<YOUR_FIDL_LIB>_cpp_driver
   //src/devices/lib/driver:driver_runtime
   ```

   Replace `YOUR_FIDL_LIB` with the name of your FIDL library, for example:

   ```none {:.devsite-disable-click-to-copy}
   public_deps = [
     ...
     "//sdk/fidl/fuchsia.factory.wlan:fuchsia.factory.wlan_cpp_wire",
     "//sdk/fidl/fuchsia.wlan.fullmac:fuchsia.wlan.fullmac_cpp_driver",
     "//sdk/fidl/fuchsia.wlan.phyimpl:fuchsia.wlan.phyimpl_cpp_driver",
     ...
     "//src/devices/lib/driver:driver_runtime",
     ...
   ]
   ```

   (Source: [`BUILD.gn`][brcmfmac-build-gn])

2. In the headers of source code, update the `include` lines, for example:

   ```cpp
   #include <fidl/<YOUR_FIDL_LIB>/cpp/driver/wire.h>
   #include <lib/fdf/cpp/arena.h>
   #include <lib/fdf/cpp/channel.h>
   #include <lib/fdf/cpp/channel_read.h>
   #include <lib/fdf/cpp/dispatcher.h>
   #include <lib/fidl/cpp/wire/connect_service.h>
   #include <lib/fidl/cpp/wire/vector_view.h>
   ...
   ```

   (Source: [`wlanphy-impl-device.h`][wlanphy-impl-device-h]{:.external})

### 2. Set up client and server objects for the driver runtime FIDL {:#set-up-client-and-server-objects-for-the-driver-runtime-fidl}

Update the server and client ends to use the driver runtime FIDL.

**On the client end**, do the following:

1. Declare a FIDL client object (`fdf::WireSharedClient<ProtocolName>` or
   `fdf::WireSyncClient<ProtocolName>`) in the device class.

   This FIDL client object enables you to make FIDL calls that send
   requests to the server end.

   The example code below shows a FIDL client object in a device class:

   ```cpp {:.devsite-disable-click-to-copy}
   class Device : public fidl::WireServer<fuchsia_wlan_device::Phy>,
                  public ::ddk::Device<Device, ::ddk::MessageableManual, ::ddk::Unbindable> {
    ...
    private:
     // Dispatcher for being a FIDL server listening MLME requests.
     async_dispatcher_t* server_dispatcher_;

     // The FIDL client to communicate with iwlwifi
     fdf::WireSharedClient<fuchsia_wlan_wlanphyimpl::WlanphyImpl> client_;
    ...
   ```

   (Source: [`device_dfv2.h`][wlanphy-device-dfv2-h]{:.external})

1. (**For asynchronous calls only**) Declare a dispatcher object
   (`fdf::Dispatcher`) in the device class.

   A dispatcher is needed to bind the FIDL client object from step 1.

   Important: Here you need to choose the driver's client type, which depends
   on whether the driver wants to make asynchronous or synchronous FIDL calls
   to the parent driver. Making synchronous calls **does not require**
   a dispatcher.

   The example code below shows the FIDL client and dispatcher objects
   in the device class:

   ```cpp {:.devsite-disable-click-to-copy}
   class Device : public fidl::WireServer<fuchsia_wlan_device::Phy>,
                  public ::ddk::Device<Device, ::ddk::MessageableManual, ::ddk::Unbindable> {
    ...
    private:
     // Dispatcher for being a FIDL server listening MLME requests.
     async_dispatcher_t* server_dispatcher_;

     // The FIDL client to communicate with iwlwifi
     fdf::WireSharedClient<fuchsia_wlan_wlanphyimpl::WlanphyImpl> client_;

     // Dispatcher for being a FIDL client firing requests to WlanphyImpl device.
     fdf::Dispatcher client_dispatcher_;
    ...
   ```

   (Source: [`device_dfv2.h`][wlanphy-device-dfv2-h]{:.external})

   You can retrieve the driver's default dispatcher using the
   `fdf::Dispatcher::GetCurrent()` method or create a new, non-default
   dispatcher (see
   [Update the DFv1 driver to use non-default dispatchers](#update-the-dfv1-driver-to-use-non-default-dispatchers)).

**On the server end**, do the following:

1. Inherit a FIDL server class (`fdf::WireServer<ProtocolName>`)
   from the device class.

1. Declare a dispatcher object (`fdf::Dispatcher`) that the FIDL server
   binds to.

   Unlike the client end, a dispatcher is always needed on the server end
   for binding the FIDL server object.

   The example code below shows the FIDL server and dispatcher objects
   in the device class:

   ```cpp {:.devsite-disable-click-to-copy}
   class Device : public fidl::WireServer<fuchsia_wlan_device::Phy>,
                  public ::ddk::Device<Device, ::ddk::MessageableManual, ::ddk::Unbindable> {
    ...
    private:
     // Dispatcher for being a FIDL server listening MLME requests.
     async_dispatcher_t* server_dispatcher_;
    ...
   ```

   (Source: [`device_dfv2.h`][wlanphy-device-dfv2-h])

   You can retrieve the driver's default dispatcher using the
   `fdf::Dispatcher::GetCurrent()` method or create a new, non-default
   dispatcher (see
   [Update the DFv1 driver to use non-default dispatchers](#update-the-dfv1-driver-to-use-non-default-dispatchers)).

**In the `.fidl` file**, do the following:

- Define a [driver service protocol][driver-service-protocol] for
  the client and server ends.

  The example code below shows a driver service protocol defined in
  a `.fidl` file:

  ```none {:.devsite-disable-click-to-copy}
  service Service {
      wlan_phy_impl client_end:WlanPhyImpl;
  };
  ```

  (Source: [`phyimpl.fidl`][phyimpl-fidl])

### 3. Update the driver to use the driver runtime FIDL {:#update-the-driver-to-use-the-driver-runtime-fidl}

With the changes in the [previous step](#set-up-client-and-server-objects-for-the-driver-runtime-fidl),
you can start updating your driver's implementation to use the driver
runtime FIDL.

Note: An assumption in this step is that your driver needs to establish
a FIDL connection with its parent driver. While it's less common, not
all drivers may not require FIDL connections with their parent drivers.

**On the client end**, do the following:

1. To connect to the protocol that the parent device driver added to
   its outgoing directory, call the
   [`DdkConnectRuntimeProtocol()`][ddktl-device-h] function, for example:

   ```cpp
   auto client_end = DdkConnectRuntimeProtocol<fuchsia_wlan_softmac::Service::WlanSoftmac>();
   ```

   (Source: [`device.cc`][wlanoftmac-device-cc])

   This function creates a pair of endpoints:

   - The function returns the `fdf::ClientEnd<ProtocolName>` object
     to the caller.
   - The `fdf::ServerEnd<ProtocolName>` object silently goes to the
     parent device driver.

1. When the caller gets the client end object, pass the object to
   the constructor of `fdf::WireSharedClient<ProtocolName>()`
   (or `fdf::WireSyncClient<ProtocolName>()`), for example:

   ```cpp
   client_ = fdf::WireSharedClient<fuchsia_wlan_phyimpl::WlanPhyImpl>(std::move(client), client_dispatcher_.get());
   ```

   (Source: [`device.cc`][wlanphy-device-cc])

**On the server end**, do the following:

1. Declare an outgoing directory object in the device class, for example:

   ```cpp {:.devsite-disable-click-to-copy}
   #include <lib/driver/outgoing/cpp/outgoing_directory.h>

   class Device : public DeviceType,
                  public fdf::WireServer<fuchsia_wlan_phyimpl::WlanPhyImpl>,
                  public DataPlaneIfc {
   ...

      fdf::OutgoingDirectory outgoing_dir_;
   ```

   (Source: [`device.h`][nxpfmac-device-h])

1. Call the `fdf::OutgoingDirectory::Create()` function so that the parent
   driver creates an outgoing directory object, for example:

   ```cpp {:.devsite-disable-click-to-copy}
   #include <lib/driver/outgoing/cpp/outgoing_directory.h>

   ...

   Device::Device(zx_device_t *parent)
       : DeviceType(parent),
         outgoing_dir_(
             fdf::OutgoingDirectory::Create(
                fdf::Dispatcher::GetCurrent()->get()))
   ```

   (Source: [`wlan_interface.cc`][wlan-interface-cc])

1. Add the service into the outgoing directory and serve it.

   The parent driver's service protocol is served to this outgoing directory
   so that the child driver can connect to it.

   In the parent driver's service callback function (which is a function
   that gets called when the child node is connected to the service), bind
   the `fdf::ServerEnd<ProtocolName>` object to itself using the
   `fdf::BindServer()` function, for example:

   ```cpp {:.devsite-disable-click-to-copy}
   zx_status_t Device::ServeWlanPhyImplProtocol(
           fidl::ServerEnd<fuchsia_io::Directory> server_end) {
     // This callback will be invoked when this service is being connected.
     auto protocol = [this](
         fdf::ServerEnd<fuchsia_wlan_phyimpl::WlanPhyImpl> server_end) mutable {
       fdf::BindServer(fidl_dispatcher_.get(), std::move(server_end), this);
       protocol_connected_.Signal();
     };

     // Register the callback to handler.
     fuchsia_wlan_phyimpl::Service::InstanceHandler handler(
          {.wlan_phy_impl = std::move(protocol)});

     // Add this service to the outgoing directory so that the child driver can
     // connect to by calling DdkConnectRuntimeProtocol().
     auto status =
          outgoing_dir_.AddService<fuchsia_wlan_phyimpl::Service>(
               std::move(handler));
     if (status.is_error()) {
       NXPF_ERR("%s(): Failed to add service to outgoing directory: %s\n",
            status.status_string());
       return status.error_value();
     }

     // Serve the outgoing directory to the entity that intends to open it, which
     // is DFv1 in this case.
     auto result = outgoing_dir_.Serve(std::move(server_end));
     if (result.is_error()) {
       NXPF_ERR("%s(): Failed to serve outgoing directory: %s\n",
            result.status_string());
       return result.error_value();
     }

     return ZX_OK;
   }
   ```

   (Source: [`device.cc`][nxpfmac-device-cc])

   Notice that the `fdf::BindServer()` function requires a
   [dispatcher][driver-dispatcher] as input. You can either use the default
   driver dispatcher provided by the driver host
   (`fdf::Dispatcher::GetCurrent()->get()`) or create a new, non-default
   dispatcher to handle FIDL requests separately (see
   [Update the DFv1 driver to use non-default dispatchers](#update-the-dfv1-driver-to-use-non-default-dispatchers)).

   Note: The return values of the creations of endpoints, dispatchers, and
   arenas are all wrapped by `zx::Status<>`, so we need to check the status
   and perform error handling first, then unwrap it with * before using it.

   At this point, the client and server ends are ready to talk to each other
   using the driver runtime FIDL.

### 4. Make FIDL requests {:#make-fidl-requests}

For making FIDL calls, use the proxy of the
`fdf::WireSharedClient<ProtocolName>()` object constructed on the client end
from the previous steps.

See the syntax below for making FIDL calls (where `CLIENT_` is the name of
the instance):

- Asynchronous FIDL call:

  ```cpp
  {{ '<strong>' }}CLIENT_{{ '</strong>' }}.buffer(*std::move(arena))->MyExampleFunction().ThenExactlyOnce([](fdf::WireUnownedResult<FidlFunctionName>& result) mutable {
    // Your result handler.
  });
  ```

  (Source: [`device.cc`][wlan-device-cc]{:.external})

- Synchronous FIDL call:

  ```cpp
  auto result = {{ '<strong>' }}CLIENT_{{ '</strong>' }}.sync().buffer(*std::move(arena))->MyExampleFunction();
  // Your result handler.
  ```

   (Source: [`device.cc`][wlan-device-cc-39]{:.external})

You may find the following practices helpful for making FIDL calls:

- Using [FIDL error syntax](#update-function-definitions-to-use-fidl-error-sytntax),
  you can call `result.is_error()` to check whether the call returns
  a domain error. Similarly, the `result.error_value()` call returns
  the exact error value.

- When making asynchronous two-way client FIDL calls, instead of
  passing a callback, you may use `.Then(callback)` or
  `.ThenExactlyOnce(callback)` to specify the desired cancellation
  semantics of pending callbacks.

- Synchronous FIDL calls will wait for the callback from the server end.
  Therefore, a callback definition is required for this call in the
  `.fidl` file. Without it, the call will just do "fire and forget" and
  return immediately.

  For callback definitions, see the following example in a `.fidl` file:

  ```none {:.devsite-disable-click-to-copy}
  protocol MyExampleProtocol {
  // You can only make async calls based on this function.
    FunctionWithoutCallback();

    // You can make both sync and async calls based on this function.
    FunctionWithCallback() -> ();
  }
  ```

  Once the callback definitions are created, a function (which will be
  invoked in both synchronous and asynchronous callbacks in the example
  above) needs to be implemented on the server end (see
  `MyExampleFunction()` below).

  When the server end device class inherits the `fdf::WireServer<ProtocolName>`
  object, virtual functions based on your protocol definition are generated,
  similar to `fidl::WireServer<ProtocolName>`. The following is the format of
  this function:

  ```cpp
  void MyExampleFunction(MyExampleFunctionRequestView request, fdf::Arena& arena, MyExampleFunctionCompleter::Sync& completer);
  ```

  This function takes three parameters:

  - `MyExampleFunctionRequestView` – Generated by FIDL, it contains the
    request structure you want to send from the client to the server.

  - `fdf::Arena` – It is the buffer of this FIDL message. This buffer is
    passed or moved from the client end. You can use it as the buffer to
    return a message or error syntax through the `completer` object. You
    can also reuse it to make FIDL calls to next level drivers.

  - `MyExampleFunctionCompleter` – Generated by FIDL, it is used to invoke
    the callback and return the result of this FIDL call to the client end.
    If FIDL error syntax is defined, you can use `completer.ReplySuccess()`
    or `completer.ReplyError()` to return a message and error status,
    if FIDL error syntax is not defined, you can only use `completer.Reply()`
    to return the message.

- You can either move the arena object into the FIDL call or simply pass it
  as `*arena`. However, moving the arena object may expose potential mistakes.
  Therefore, passing the arena object is recommended instead. Because the
  arena may be reused for the next level FIDL call, the arena object is not
  destroyed after being passed.

- If your driver sends messages formatted as self-defined types in the
  `.fidl` file, FIDL generates both natural type and wire type based on the
  definition. However, the wire type is recommended in this case because
  the wire type is a more stable choice.

- FIDL closes its channel if it notices that the endpoints exchanged invalid
  values, which is known as validation error:

  - A good example of a validation error is when passing 0 (for a non-flexible
    enum) while it only defines values 1 through 5. In case of a validation
    error, the channel is closed permanently and all subsequent messages
    result in failure. **This behavior is different from Banjo**. (For more
    information, see [Strict vs. Flexible][strict-vs-flexible].)

  - Invalid values also include kernel objects with the `zx_handle_t` value
    set to 0. For example, `zx::vmo(0)`.

## (Optional) Update the DFv1 driver to use non-default dispatchers {:#update-the-dfv1-driver-to-use-non-default-dispatchers}

Important: This section is relevant only if your DFv1 driver has migrated to
[use the driver runtime](#update-the-dfv1-driver-to-use-the-driver-runtime)
and the driver needs to use non-default dispatchers.

The `fdf::Dispatcher::GetCurrent()` method gives you the
[default dispatcher][default-dispatcher] that the driver is running on.
If possible, it's recommended to use this default dispatcher alone. However,
if you need to create a new, non-default dispatcher, the driver runtime
needs to understand which driver the dispatcher belongs to. This allows
the driver framework to set up proper attributes and shut down
the dispatcher correctly.

The sections below describe advanced use cases of allocating and managing
your own non-default dispatcher:

1. [Allocate a dispatcher](#allocate-a-dispatcher).
2. [Shut down a dispatcher](#shut-down-a-dispatcher).

### 1. Allocate a dispatcher {:#allocate-a-dispatcher}

To make sure that dispatchers are created and run in the threads managed
by the driver framework, the allocation of dispatchers must be done in the
functions invoked by the driver framework (such as `DdkInit()`), for example:

```cpp {:.devsite-disable-click-to-copy}
void Device::DdkInit(ddk::InitTxn txn) {
  bool fw_init_pending = false;
  const zx_status_t status = [&]() -> zx_status_t {
    auto dispatcher = fdf::SynchronizedDispatcher::Create(
        {}, "nxpfmac-sdio-wlanphy",
        [&](fdf_dispatcher_t *) { sync_completion_signal(&fidl_dispatcher_completion_); });
    if (dispatcher.is_error()) {
      NXPF_ERR("Failed to create fdf dispatcher: %s", dispatcher.status_string());
      return dispatcher.status_value();
    }
    fidl_dispatcher_ = std::move(*dispatcher);
  ...
```

(Source: [`device.cc`][nxpfmac-device-cc-71])

### 2. Shut down a dispatcher {:#shut-down-a-dispatcher}

Important: Unlike DFv1, DFv2 handles the dispatcher shutdown automatically.
When you start migrating your DFv1 driver to DFv2 in the
[next phase][migrate-from-dfv1-to-dfv2], you will need to remove your own
dispatcher shutdown implementation covered in this section.

Likewise, the dispatcher shutdown also needs to happen in the functions
invoked by the driver framework for the same reason. The `DdkUnbind()` or
`device_unbind()` method is a good candidate for this operation.

Note that dispatcher shutdown is asynchronous, which needs to be handled
appropriately. For example, if a dispatcher is shutting down in the
`DdkUnbind()` call, we need to use the `ddk::UnbindTxn` object (passed
from the `Unbind()` call previously) to invoke the `ddk::UnbindTxn::Reply()`
call in the shutdown callback of the dispatcher to ensure a clean shutdown.

The following code snippet examples demonstrate the shutdown process
described above:

1. Save the `ddk::UnbindTxn` object in `DdkUnbind()`:

   ```cpp
   void DdkUnbind(ddk::UnbindTxn txn) {
     // Move the txn here because it’s not copyable.
     unbind_txn_ = std::move(txn);
     ...
   }
   ```

1. As part of the bind or `DdkInit()` hook, create a dispatcher with
   a shutdown callback that invokes `ddk::UnbindTxn::Reply()`:

   ```cpp
     auto dispatcher = fdf::Dispatcher::Create(0, [&](fdf_dispatcher_t*) {
       if (unbind_txn_)
         unbind_txn_->Reply();
       unbind_txn_.reset();
     });
   ```

1. Call `ShutdownAsync()` from the dispatcher at the end of `DdkUnbind()`:

   ```cpp
   void DdkUnbind(ddk::UnbindTxn txn) {
     // Move the txn here because it’s not copyable.
     unbind_txn_ = std::move(txn);
     ...
     dispatcher.ShutDownAsync();
   }
   ```

However, if there are more than one dispatcher allocated in the driver,
because `ddk::UnbindTxn::Reply()` is called only once, you need to implement
a chain of shutdown operations. For instance, given dispatchers A and B
(which are interchangeable), you can:

1. Call `ShutdownAsync()` for B in the shutdown callback of A.
1. Call `ddk::UnbindTxn::Reply()` in the shutdown callback of B.

## (Optional) Update the DFv1 driver to use two-way communication {:#update-the-dfv1-driver-to-use-two-way-communication}

It is common that only the device on the client end needs to proactively
make requests to the server-end device. But in some cases, both devices
need to send messages across the channel without having to wait for the
other end to respond.

Important: Banjo and FIDL cannot be used simultaneously between a pair of
devices. This means that two-way communication between devices must be done
using FIDL.

For establishing two-way communication in your DFv1 driver, you have the
following three options:

- **Option 1** – Define events in the FIDL protocol
  (see [Implement a C++ FIDL server][implement-fidl-server]).

- **Option 2** – Implement a second FIDL protocol in the opposite direction
  so that devices on both ends are both server and client at the same time.

- **Option 3** – Use the "[hanging get"][hanging-get] pattern in FIDL (a
  [flow control][flow-control] design pattern recommended by the FIDL rubric).

Reasons to use events (**option 1**) include:

- Simplicity - A single protocol is simpler than two.

- Serialization - If you need two protocols, events and replies to requests are
  guaranteed to be serialized in the order they are written to the channel.

Reasons not to use events (**option 1**) include:

- You need to respond to messages when they are sent from the server to
  the client.

- You need to control the flow of messages.

The protocols below implement **option 2** where they are defined in the same
`.fidl` file for two different directions:

- [`WlanSoftmac` protocol][softmac-fidl-393]
- [`WlanSoftmacIfc` protocol][softmac-fidl-200]

For the WLAN driver migration, the team selected **option 2** since it wasn't
going to introduce additional FIDL syntax from unknown domains. But it should
be noted that the WLAN driver was the first instance of the driver runtime
migration, and FIDL events in driver transport were not supported at the time
of migration.

Depending on your needs, a "hanging get" call (**option 3**) is often a better
option than an event because it allows the client to specify that it is ready
to handle the event, which also provides flow control. (However, if necessary,
there are alternative ways to add flow control to events, which are described
in the [Throttle events using acknowledgements][throttle-events] section of
the FIDL Rubric.)

## Update the DFv1 driver's unit tests to use FIDL {:#update-the-dfv1-drivers-unit-tests-to-use-fidl}

If there exist unit tests based on the Banjo APIs for your driver, you need
to migrate the tests to provide a mock FIDL server instead of the Banjo server
(or a mock FIDL client).

In DFv1, to mock a FIDL server or client, you can use the
`MockDevice::FakeRootParent()` method, for example:

```cpp
std::shared_ptr<MockDevice> fake_parent_ = MockDevice::FakeRootParent();
```

(Source: [`ft_device_test.cc`][ft-device-test-cc])

The `MockDevice::FakeRootParent()` method is integrated with the
`DriverRuntime` testing library (which is the only supported testing library
for DFv1). The `MockDevice::FakeRootParent()` method creates
a `fdf_testing::DriverRuntime` instance for the user. The instance then starts
the driver runtime and creates a foreground driver dispatcher for the user.
However, background dispatchers can also be created through this object. You
can grab the instance using the `fdf_testing::DriverRuntime::GetInstance()`
method.

For an example, see this [unit test][aml-power-test-cc] that mocks
the PWM and vreg FIDL protocols for a DFv1 driver.

Also, the following library may be helpful for writing driver unit tests:

- [`//src/devices/bus/testing/fake-pdev/fake-pdev.h`][fake-pdev-h] – This
  helper library implements a fake version of the pdev FIDL protocol.

## Additional resources {:#additional-resources}

All the **Gerrit changes** mentioned in this section:

- [\[iwlwifi\]\[wlanphy\] Driver runtime Migration(wlanphy <--> iwlwifi)][gc-wlanphy-device-test-cc]{:.external}
- [\[iwlwifi\]\[wlansoftmac\] Driver runtime Migration(wlansoftmac <--> iwlwifi)][gc-wlan-device-cc]{:.external}

All the **source code files** mentioned in this section:

- [`//sdk/banjo/fuchsia.hardware.wlanphyimpl/wlanphy-impl.fidl`][wlanphy-impl-fidl]{:.external}
- [`//sdk/fidl/fuchsia.wlan.phyimpl/phyimpl.fidl`][phyimpl-fidl]
- [`//sdk/fidl/fuchsia.wlan.softmac/softmac.fidl`][softmac-fidl]
- [`//sdk/lib/driver/component/cpp/tests/driver_base_test.cc`][driver-base-test-cc]
- [`//sdk/lib/driver/testing/cpp`][driver-testing-cpp]
- [`//src/connectivity/wlan/drivers/third_party/nxp/nxpfmac/device.cc`][nxpfmac-device-cc]
- [`//src/connectivity/wlan/drivers/wlansoftmac/device.cc`][wlanoftmac-device-cc]
- [`//src/connectivity/wlan/drivers/wlansoftmac/meta/wlansoftmac.cml`][wlansofmac-cml]
- [`//examples/drivers/transport/driver/`][driver-transport-example]

All the **documentation pages** mentioned in this section:

- [FIDL][fidl]
- [RFC-0126: Driver Runtime][driver-runtime-rfc]
- [New C++ bindings tutorials][cpp-tutorial]
- [Driver dispatcher and threads][driver-dispatcher]
- [FIDL attributes][fidl-attributes]
- [Implement a C++ FIDL server][implement-fidl-server]
- [Define a driver service protocol][driver-service-protocol]
  (from _Expose the driver capabilities_)
- [Strict vs. Flexible][strict-vs-flexible]
  (from _FIDL language specification_)
- [Throttle events using acknowledgements][throttle-events]
  (from _FIDL API Rubric_)
- [Delay responses using hanging gets][hanging-get]
  (from _FIDL API Rubric_)

<!-- Reference links -->

[fidlbolt]: https://fidlbolt-6jq5tlqt6q-uc.a.run.app/
[fidl-guides]: /docs/development/languages/fidl/README.md
[fidl]: /docs/concepts/fidl/overview.md
[migrate-from-dfv1-to-dfv2]: /docs/development/drivers/migration/migrate-from-dfv1-to-dfv2/overview.md
[driver-runtime-rfc]: /docs/contribute/governance/rfcs/0126_driver_runtime.md
[cpp-tutorial]: /docs/development/languages/fidl/tutorials/cpp/README.md
[synchronized-dispatchers]: /docs/concepts/drivers/driver-dispatcher-and-threads.md#synchronous-operations
[wlanofmac-cml]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/connectivity/wlan/drivers/wlansoftmac/meta/wlansoftmac.cml
[driver-dispatcher]: /docs/concepts/drivers/driver-dispatcher-and-threads.md
[discoverable]: /docs/reference/fidl/language/attributes.md#discoverable
[wlanphy-impl-fidl]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/928355/6/sdk/banjo/fuchsia.hardware.wlanphyimpl/wlanphy-impl.fidl
[phyimpl-fidl]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.wlan.phyimpl/phyimpl.fidl
[phyimpl-build-gn]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.wlan.phyimpl/BUILD.gn
[brcmfmac-build-gn]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/BUILD.gn;l=170
[wlanphy-impl-device-h]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/655947/75/src/connectivity/wlan/drivers/third_party/intel/iwlwifi/platform/wlanphy-impl-device.h
[wlanphy-device-dfv2-h]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/655947/75/src/connectivity/wlan/drivers/wlanphy/device_dfv2.h
[driver-service-protocol]: /docs/development/drivers/tutorials/sdk_build_driver/driver-service.md#define_a_driver_service_protocol
[phyimpl-fidl]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.wlan.phyimpl/phyimpl.fidl;l=96
[ddktl-device-h]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/lib/ddktl/include/ddktl/device.h;l=610;drc=172f766331e1dd3509833ce5a9a86917d945b2d2?q=ddkconnectruntimeprotocol&ss=fuchsia
[wlanoftmac-device-cc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/connectivity/wlan/drivers/wlansoftmac/device.cc;l=312
[wlanphy-device-cc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/connectivity/wlan/drivers/wlanphy/device.cc;l=69
[nxpfmac-device-h]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/connectivity/wlan/drivers/third_party/nxp/nxpfmac/device.h
[wlan-interface-cc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/connectivity/wlan/drivers/third_party/nxp/nxpfmac/wlan_interface.cc
[nxpfmac-device-cc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/connectivity/wlan/drivers/third_party/nxp/nxpfmac/device.cc;l=175
[wlan-device-cc]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/655947/59/src/connectivity/wlan/drivers/wlanphy/device.cc#145
[wlan-device-cc-39]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/660030/39/src/connectivity/wlan/drivers/wlan/device.cc#146
[strict-vs-flexible]: /docs/reference/fidl/language/language.md#strict-vs-flexible
[nxpfmac-device-cc-71]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/connectivity/wlan/drivers/third_party/nxp/nxpfmac/device.cc;l=71
[implement-fidl-server]: /docs/development/languages/fidl/tutorials/cpp/basics/server.md
[hanging-get]: /docs/development/api/fidl.md#hanging-get
[flow-control]: /docs/development/api/fidl.md#flow_control
[softmac-fidl-393]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.wlan.softmac/softmac.fidl;l=393
[softmac-fidl-200]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.wlan.softmac/softmac.fidl;l=200
[throttle-events]: /docs/development/api/fidl.md#throttle-events-using-acknowledgements
[ft-device-test-cc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/ui/input/drivers/focaltech/ft_device_test.cc;l=277
[aml-power-test-cc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/power/drivers/aml-meson-power/aml-power-test.cc
[fake-pdev-h]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/bus/testing/fake-pdev/fake-pdev.h
[gc-wlan-device-cc]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/660030/109
[gc-wlanphy-device-test-cc]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/655947/75
[wlansofmac-cml]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/connectivity/wlan/drivers/wlansoftmac/meta/wlansoftmac.cml
[softmac-fidl]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.wlan.softmac/softmac.fidl
[driver-testing-cpp]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/testing/cpp/
[driver-base-test-cc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/component/cpp/tests/driver_base_test.cc
[nxpfmac-device-cc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/connectivity/wlan/drivers/third_party/nxp/nxpfmac/device.cc
[fidl-attributes]: /docs/reference/fidl/language/attributes.md
[default-dispatcher]: /docs/concepts/drivers/driver-dispatcher-and-threads.md#default-dispatcher
[driver-transport-example]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/transport/driver/

