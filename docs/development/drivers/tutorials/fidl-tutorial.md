# FIDL tutorial

Caution: This page may contain information that is specific to the legacy
version of the driver framework (DFv1).

This guide explains how to go about adding exporting a FIDL protocol from a
driver and utilize it in another driver. This guide assumes familiarity with the
following concepts:

*   [FIDL](/docs/development/languages/fidl/README.md)
*   [Driver Binding](/docs/development/drivers/concepts/device_driver_model/driver-binding.md)
*   [DDKTL](/docs/development/drivers/concepts/driver_development/using-ddktl.md)
*   [New C++ FIDL Bindings](/docs/development/languages/fidl/tutorials/cpp/README.md)

## FIDL Protocol Definition

The following snippets will utilize this FIDL protocol:

```
library fidl.examples.echo;

const MAX_STRING_LENGTH uint64 = 32;

// The discoverable annotation is required, otherwise the protocol bindings
// will not have a name string generated.
@discoverable
protocol Echo {
    /// Returns the input.
    EchoString(struct {
        value string:<MAX_STRING_LENGTH, optional>;
    }) -> (struct {
        response string:<MAX_STRING_LENGTH, optional>;
    });
};

service EchoService {
    echo client_end:Echo;
};
```

## Parent Driver (The Server)

We approximate here how a parent driver which implements the protocol being
called into would be written. Although not shown, we assume this class is
utilizing the DDKTL.

```
// This class implement the fuchsia.examples.echo/Echo FIDL protocol using the
// new C++ FIDL bindings
class Device : public fidl::WireServer<fidl_examples_echo::Echo> {

  // This is the main entry point for the driver.
  static zx_status_t Bind(void* ctx, zx_device_t* parent) {
    // When creating the device, we initialize it with a dispatcher provided by
    // the driver framework. This dispatcher is allows us to schedule
    // asynchronous work on the same thread as other drivers. You may opt to
    // create your own dispatcher which is serviced on a thread you spawn if you
    // desire instead.
    auto* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    auto device = std::make_unique<Device>(parent, dispatcher);

    // We add the FIDL protocol we wish to export to our child to our outgoing
    // directory. When a connection is attempted we will bind the server end of
    // the channel pair to our server implementation.
    zx::result = device->outgoing_.AddService<fidl_examples_echo::EchoService>(
        fidl_examples_echo::EchoService::InstanceHandler({
            .echo = device->bindings_.CreateHandler(device.get(), dispatcher,
                                                    fidl::kIgnoreBindingClosure),
            }));

    // Utilizing the server end of the endpoint pair we created above, we bind
    // it to our outgoing directory.
    result = device->outgoing_.Serve(std::move(endpoints->server));
    if (result.is_error()) {
      zxlogf(ERROR, "Failed to service the outgoing directory");
      return result.status_value();
    }

    // We declare our outgoing protocols here. These will be utilize to
    // help the framework populate node properties which can be used for
    // binding.
    std::array offers = {
        fidl_examples_echo::Service::Name,
    };

    status = device->DdkAdd(ddk::DeviceAddArgs("parent")
                                // The device must be spawned in a separate
                                // driver host.
                                .set_flags(DEVICE_ADD_MUST_ISOLATE)
                                .set_fidl_service_offers(offers)
                                // The client side of outgoing directory is
                                // provided to the framework. This will be
                                // forwarded to the new driver host that spawns to
                                // allow the child driver which binds the ability
                                // to connect to our outgoing FIDL protocols.
                                .set_outgoing_dir(endpoints->client.TakeChannel()));
    if (status == ZX_OK) {
      [[maybe_unused]] auto ptr = device.release();
    } else {
      zxlogf(ERROR, "Failed to add device");
    }

    return status;
  }

 private:
  // This is the implementation of the only method our FIDL protocol requires.
  void EchoString(EchoStringRequestView request, EchoStringCompleter::Sync& completer) override {
    completer.Reply(request->value);
  }

  // This is a helper class which we use to serve the outgoing directory.
  component::OutgoingDirectory outgoing_;
  // This ensures that the fidl connections don't outlive the device object.
  fidl::ServerBindingGroup<fidl_examples_echo::Echo> bindings_;
};
```

## Child Driver (The Client)

### Binding

The first important thing to discuss is how the child driver will bind. It can
bind due to any number of node properties, but if you wish to bind based
on the FIDL service the parent offers, you can bind using `fuchsia.Service`:

```
fuchsia.Service == "fidl.examples.echo.EchoService";
```

You can add additional bind constraints if you desire. Note that the
`fuchsia.Service` property is automatically added when the parent driver declares
FIDL service offers at the time of adding the child node.

### Client Code

The follow code snippet would be found in a child driver which has successfully
bound to the parent driver described above.

```
zx_status_t CallEcho() {
  // The following method allows us to connect to the protocol we desire. This
  // works by providing the server end of our endpoint pair to the framework. It
  // will push this channel through the outgoing directory to our parent driver
  // which will then bind it to its server implementation. We do  not need to
  // name the protocol because the method is templated on the  channel type and
  // it is able to automatically derive the name from the type.
  zx::result client_end = DdkConnectFidlProtocol<fidl_examples_echo::EchoService::Echo>();
  if (client_end.is_error()) {
    zxlogf(ERROR, "Failed to connect fidl protocol: %s", client_end.status_string());
    return client_end.status_value();
  }

  // We turn the client side of the endpoint pair into a synchronous client.
  fidl::WireSyncClient client{std::move(client_end.value())};

  // We can now utilize our client to make calls!
  constexpr std::string_view kInput = "Test String";

  auto result = client->EchoString(fidl::StringView::FromExternal(std::string_view(kInput)));
  if (!result.ok()) {
    zxlogf(ERROR, "Failed to call EchoString");
    return result.status();
  }
  if (result->response.get() != kInput) {
    zxlogf(ERROR, "Unexpected response: Actual: \"%.*s\", Expected: \"%.*s\"",
           static_cast<int>(result->response.size()), result->response.data(),
           static_cast<int>(kInput.size()), kInput.data());
    return ZX_ERR_INTERNAL;
  }

  return ZX_OK;
}
```

