# Connect and serve Banjo protocols in a DFv2 driver

This guide walks through the tasks of serving Banjo protocols in a DFv2 driver
and connecting to its Banjo server from a DFv1 child driver.

Banjo protocols, primarily used in DFv1 drivers, are defined in FIDL library
annotated with the `@transport("Banjo")` and `@banjo_layout("ddk-protocol")`
lines, for example:

```cpp {:.devsite-disable-click-to-copy}
/// The protocol provides access to functions of the driver.
{{"<strong>"}}@transport("Banjo"){{"</strong>"}}
{{"<strong>"}}@banjo_layout("ddk-protocol"){{"</strong>"}}
closed protocol Misc {
    /// Returns a unique identifier for this device.
    strict GetHardwareId() -> (struct {
        status zx.Status;
        response uint32;
    });

    /// Returns the current device firmware version
    strict GetFirmwareVersion() -> (struct {
        status zx.Status;
        major uint32;
        minor uint32;
    });
};
```

(Source: [`gizmo.test.fidl`][gizmo-test-fidl])

To enable a DFv2 driver to use Banjo protocols, see the tasks below:

- [Serve a Banjo protocol from a DFv2 driver](#serve-a-bano-protocol-from-a-dfv2-driver):
  Implement and serve Banjo protocols in a DFv2 driver, which sets up
  the driver to communicate with child drivers using Banjo protocols.

- [Connect to a Banjo server from a DFv2 driver](#connect-to-a-banjo-server-from-a-DFv2-driver):
  Connect to a parent driver's Banjo server from a DFv2 child driver
  and start using Banjo protocols.

## Serve a Banjo protocol from a DFv2 driver {:#serve-a-bano-protocol-from-a-dfv2-driver}

This section walks through implementing a Banjo protocol in a DFv2 driver
and serving the protocol to child drivers. This walkthrough is based
on the [Banjo Transport example][banjo-transport-example], which implements
the `Misc` Banjo protocol in the [`gizmo.test`][gizmo-test-fidl] FIDL library.

The steps are:

1. [Set up the Banjo protocol](#set-up-the-banjo-protocol).
2. [Implement the Banjo protocol](#implement-the-banjo-protocol).
3. [Serve the Banjo protocol](#serve-the-banjo-protocol).

### 1. Set up the Banjo protocol {:#set-up-the-banjo-protocol}

To set up the `Misc` Banjo protocol in a DFv2 driver, do the following:

1. In the `BUILD.gn` file, add the Banjo library as a dependency in the
  `fuchsia_driver` target, for example:

   ```gn {:.devsite-disable-click-to-copy}
   fuchsia_cc_driver("parent_driver") {
     output_name = "banjo_transport_parent"
     sources = [ "parent-driver.cc" ]
     deps = [
       "//examples/drivers/bind_library:gizmo.example_cpp",
       {{"<strong>"}}"//examples/drivers/transport/banjo:fuchsia.examples.gizmo_banjo_cpp",{{"</strong>"}}
        ...
     ]
   }
   ```

1. In the driver's C++ header file, include the Banjo library's C++ header, for
   example:

   ```cpp {:.devsite-disable-click-to-copy}
   {{"<strong>"}}#include <fuchsia/examples/gizmo/cpp/banjo.h>{{"</strong>"}}
   ...

   namespace banjo_transport {
   ...
   ```

   (Source: [`parent-driver.h`][parent-driver-h])

1. To inherit from the Banjo protocol bindings, update the driver class using
   the following format:

   ```cpp
   ddk::<PROTOCOL_NAME>Protocol<<YOUR_DRIVER_CLASS>>
   ```

   Replace `PROTOCOL_NAME` with the name of the Banjo protocol and
   `YOUR_DRIVER_CLASS` is the class of your driver, both in camel case,
   for example:

   ```cpp {:.devsite-disable-click-to-copy}
   class ParentBanjoTransportDriver : public fdf::DriverBase,
       {{"<strong>"}}public ddk::MiscProtocol<ParentBanjoTransportDriver>{{"</strong>"}}  {
       ...
   };
   ```

   (Source: [`parent-driver.h`][parent-driver-h])

### 2. Implement the Banjo protocol {:#implement-the-banjo-protocol}

In the driver class, define and implement each function in the Banjo
protocol.

For instance, the example below shows a Banjo protocol named `ProtocolName`:

```cpp {:.devsite-disable-click-to-copy}
@transport("Banjo")
@banjo_layout("ddk-protocol")
closed protocol ProtocolName {
   /// Returns a unique identifier for this device.
   strict FunctionName() -> (struct {
       status zx.Status;
       response_1 response_1_type;
       response_2 response_2_type;
   });
};
```

For this `ProtocolName` Banjo protocol, the C++ binding for the
`FunctionName` function looks like below:

```cpp {:.devsite-disable-click-to-copy}
zx_status_t ProtocolNameFunctionName(response_1_type* response_1, response_2_type* response_2);
```

And you can find the C++ bindings of the existing Banjo protocols
in the following path of the Fuchsia source checkout:

```none {:.devsite-disable-click-to-copy}
<OUT_DIRECTORY>/fidling/gen/<PATH_TO_THE_FIDL_LIBRARY>/banjo
```

For instance, if your `out` directory is `out/default` and the FIDL
library is located in the `examples/drivers/transport` directory,
then the C++ bindings are located in the following directory:

```none {:.devsite-disable-click-to-copy}
out/default/fidling/gen/examples/drivers/transport/banjo
```

See the following implementation in the [Banjo Transport example][banjo-transport-example]:

- The `Misc` protocol contains the functions below:

  ```cpp {:.devsite-disable-click-to-copy}
  /// Returns a unique identifier for this device.
  strict GetHardwareId() -> (struct {
   status zx.Status;
   response uint32;
  });

  /// Returns the current device firmware version
  strict GetFirmwareVersion() -> (struct {
   status zx.Status;
   major uint32;
   minor uint32;
  });
  ```

- The `ParentBanjoTransportDriver` class defines these functions as
  below:

  ```cpp {:.devsite-disable-click-to-copy}
  class ParentBanjoTransportDriver : public fdf::DriverBase,
                                    public ddk::MiscProtocol<ParentBanjoTransportDriver> {
  public:
  ...

   // MiscProtocol implementation.
   zx_status_t MiscGetHardwareId(uint32_t* out_response);
   zx_status_t MiscGetFirmwareVersion(uint32_t* out_major, uint32_t* out_minor);
  ...
  };
  ```

  (Source: [`parent-driver.h`][parent-driver-h])

- The functions are implemented as below:

  ```cpp {:.devsite-disable-click-to-copy}
  zx_status_t ParentBanjoTransportDriver::MiscGetHardwareId(uint32_t* out_response) {
   *out_response = 0x1234ABCD;
   return ZX_OK;
  }

  zx_status_t ParentBanjoTransportDriver::MiscGetFirmwareVersion(uint32_t* out_major,
                                                                uint32_t* out_minor) {
   *out_major = 0x0;
   *out_minor = 0x1;
   return ZX_OK;
  }
  ```

  (Source: [`parent-driver.cc`][parent-driver-cc])

### 3. Serve the Banjo protocol {:#serve-the-banjo-protocol}

Once the Banjo protocol is implemented in a DFv2 driver, you need to
serve the protocol to a DFv1 child node using a compat device server
configured with Banjo.

To do so, complete the following tasks in the
[Set up the compat device server in a DFv2 driver][set-up-the-compat-device-server-in-a-dfv2-driver]
guide:

1. [Set up the compat device server][set-up-the-compat-device-server].
2. [Provide Banjo services to descendant DFv1 drivers][provide-banjo-services].

## Connect to a Banjo server from a DFv2 driver {:#connect-to-a-banjo-server-from-a-DFv2-driver}

This section uses the [Banjo transport example][banjo-transport-example]
to walk through the task of connecting a DFv2 child driver to a parent
driver that serves a Banjo protocol.

The steps are:

1. [Connect the child driver to the parent driver](#connect-the-child-driver-to-the-parent-driver).
2. [Use the Banjo protocol](#use-the-banjo-protocol).

### 1. Connect the child driver to the parent driver {:#connect-the-child-driver-to-the-parent-driver}

To be able to connect a child driver to a parent driver for using Banjo
protocols, the child must be co-located in the same driver host as the parent
and both drivers need to use the compat [`banjo_client`][banjo-client] library.

To connect the child driver to the parent driver, do the following:

1. In the child driver's component manifest (`.cml`), set the `colocate`
   field to `true`, for example:

   ```cml {:.devsite-disable-click-to-copy}
   {
       include: [ 'syslog/client.shard.cml' ],
       program: {
           runner: 'driver',
           binary: 'driver/banjo_transport_child.so',
           bind: 'meta/bind/child-driver.bindbc',

           // Run in the same driver host as the parent driver
           {{"<strong>"}}colocate: 'true',{{"</strong>"}}
       },
       use: [
           { service: 'fuchsia.driver.compat.Service' },
       ],
   }
   ```

   (Source: [`child-driver.cml`][child-driver-cml])

1. In the child driver's `BUILD.gn` file, add the Banjo library as
   a dependency in the `fuchsia_driver` target, for example:

   ```gn {:.devsite-disable-click-to-copy}
   fuchsia_cc_driver("child_driver") {
     output_name = "banjo_transport_child"
     sources = [ "child-driver.cc" ]
     deps = [
       {{"<strong>"}}"//examples/drivers/transport/banjo:fuchsia.examples.gizmo_banjo_cpp",{{"</strong>"}}
       "//sdk/lib/driver/component/cpp:cpp",
       "//src/devices/lib/driver:driver_runtime",
     ]
   }
   ```

   (Source: [`BUILD.gn`][build-gn])

1. In the child driver's C++ header file, include the Banjo library's
   C++ header, for example:

   ```
   {{"<strong>"}}#include <fuchsia/examples/gizmo/cpp/banjo.h>{{"</strong>"}}
   ...

   namespace banjo_transport {
   ...
   ```

   (Source: [`child-driver.h`][child-driver-h])

1. In the child driver's `BUILD.gn` file, add the compat `banjo_client`
   library as a dependency in the `fuchsia_driver` target, for example:

   ```gn {:.devsite-disable-click-to-copy}
   fuchsia_cc_driver("child_driver") {
     output_name = "banjo_transport_child"
     sources = [ "child-driver.cc" ]
     deps = [
       "//examples/drivers/transport/banjo:fuchsia.examples.gizmo_banjo_cpp",
       {{"<strong>"}}"//sdk/lib/driver/compat/cpp",{{"</strong>"}}
       "//sdk/lib/driver/component/cpp:cpp",
       "//src/devices/lib/driver:driver_runtime",
     ]
   }
   ```

   (Source: [`BUILD.gn`][build-gn])

1. In the child driver's C++ source file, include the compat `banjo_client`
   library's C++ header, for example:

   ```cpp {:.devsite-disable-click-to-copy}
   #include "examples/drivers/transport/banjo/v2/child-driver.h"

   {{"<strong>"}}#include <lib/driver/compat/cpp/compat.h>{{"</strong>"}}
   #include <lib/driver/component/cpp/driver_export.h>
   #include <lib/driver/logging/cpp/structured_logger.h>

   namespace banjo_transport {

   zx::result<> ChildBanjoTransportDriver::Start() {
   ...
   ```

   (Source: [`child-driver.cc`][child-driver-cc])

1. In the child driver's C++ source file, set up a Banjo client with the
   `compat::ConnectBanjo()` function, for example:

   ```cpp
   zx::result<Client> ConnectBanjo(const std::shared_ptr<fdf::Namespace>& incoming,
                                   std::string_view parent_name = "default") {
   ```

   In the [Banjo Transport example][banjo-transport-example], the child
   driver does the following to connect to the `Misc` protocol:

   ```cpp {:.devsite-disable-click-to-copy}
   zx::result<ddk::MiscProtocolClient> client =
         compat::ConnectBanjo<ddk::MiscProtocolClient>(incoming());

   if (client.is_error()) {
     fdf::error("Failed to connect client: {}", client);
     return client.take_error();
   }
   ```

   (Source: [`child-driver.cc`][child-driver-cc])

### 2. Use the Banjo protocol {:#use-the-banjo-protocol}

In the child driver, use the protocol client to invoke the Banjo functions.

For instance, the example below shows a Banjo protocol named
`ProtocolName`:

```cpp {:.devsite-disable-click-to-copy}
@transport("Banjo")
@banjo_layout("ddk-protocol")
closed protocol ProtocolName {
   /// Returns a unique identifier for this device.
   strict FunctionName() -> (struct {
       status zx.Status;
       response_1 response_1_type;
       response_2 response_2_type;
   });
};
```

For this `ProtocolName` Banjo protocol, the C++ binding for the
`FunctionName` function looks like below:

```cpp {:.devsite-disable-click-to-copy}
zx_status_t ProtocolNameFunctionName(response_1_type* response_1, response_2_type* response_2);
```

In the [Banjo Transport example][banjo-transport-example],
the `GetHardwareId()` function is defined as below:

```cpp {:.devsite-disable-click-to-copy}
/// Returns a unique identifier for this device.
strict GetHardwareId() -> (struct {
 status zx.Status;
 response uint32;
});
```

(Source: [`gizmo.test.fidl`][gizmo-test-fidl])

With the Banjo client stored in the `client_` object
and the `hardware_id_` variable defined as
a `uint32_t`, you can call the `GetHardwareId()` function
in the following way:

```cpp {:.devsite-disable-click-to-copy}
zx_status_t status = client_.GetHardwareId(&hardware_id_);
if (status != ZX_OK) {
 return status;
}
```

(Source: [`child-driver.cc`][child-driver-cc])

<!-- Reference links -->

[banjo-transport-example]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/transport/banjo/v2/
[gizmo-test-fidl]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/transport/banjo/gizmo.test.fidl
[parent-driver-h]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/transport/banjo/v2/parent-driver.h
[parent-driver-cc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/transport/banjo/v2/parent-driver.cc
[set-up-the-compat-device-server-in-a-dfv2-driver]: /docs/development/drivers/migration/set-up-compat-device-server.md
[set-up-the-compat-device-server]: /docs/development/drivers/migration/set-up-compat-device-server.md#set-up-the-compat-device-server
[child-driver-cml]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/transport/banjo/v2/meta/child-driver.cml
[build-gn]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/transport/banjo/v2/BUILD.gn
[child-driver-h]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/transport/banjo/v2/child-driver.h
[child-driver-cc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/transport/banjo/v2/child-driver.cc
[provide-banjo-services]: /docs/development/drivers/migration/set-up-compat-device-server.md#provide-banjo-services-to-descendant-dfv1-drivers
[banjo-client]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/compat/cpp/banjo_client.h
