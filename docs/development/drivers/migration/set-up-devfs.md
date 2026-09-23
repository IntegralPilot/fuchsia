# Set up devfs in a DFv2 driver

This guide provides instructions on how to set up [`devfs`][devfs] in a DFv2
driver, which enables the driver's services to be discovered by other Fuchsia
components in the system. This guide uses an [example DFv2 driver][example-driver]
to walk through the `devfs` setup process.

Important: Setting up `devfs` is no longer necessary. See
[Driver Communication][driver-communication] for a guide on how to use services
in your driver.

The steps are:

1. [Identify the target FIDL protocol](#identify-the-target-fidl-protocol)
1. [Add a ServerBindingGroup object](#add-a-serverbindinggroup-object)
1. [Create a callback function](#create-a-callback-function)
1. [Initialize a devfs connector](#initialize-a-devfs-connector)
1. [Bind the devfs connector to a dispatcher](#bind-the-devfs-connector-to-a-dispatcher)
1. [Add the devfs connector to a child node](#add-the-devfs-connector-to-a-child-node)

## 1. Identify the target FIDL protocol {:#identify-the-target-fidl-protocol}

In the DFv2 driver's source code, identify the target FIDL protocol (which can
be inherited from `fidl::Server<>` or `fidl::WireServer<>`) for this `devfs`
setup, for example:

```cpp
class RetrieverDriver final : public fdf::DriverBase,
                              public fidl::Server<fuchsia_examples_metadata::Retriever> {

  ...

  // fuchsia.hardware.test/Child implementation.
  void GetMetadata(GetMetadataCompleter::Sync& completer) override {
    ...
  }
};
```

(Source: [`retriever-driver.cc`][retriever-driver-cc])

The examples in this guide use the `Retriever` protocol defined in
[`fuchsia.examples.metadata.fidl`][retriever-protocol].

## 2. Add a ServerBindingGroup object {:#add-a-serverbindinggroup-object}

Add a `ServerBindingGroup` object to the driver class for binding the FIDL
protocol, for example:

```cpp
class RetrieverDriver final : public fdf::DriverBase,
                              public fidl::Server<fuchsia_examples_metadata::Retriever> {

...

fidl::ServerBindingGroup<fuchsia_examples_metadata::Retriever> bindings_;
```

(Source: [`retriever-driver.cc`][retriever-driver-cc])

## 3. Create a callback function {:#create-a-callback-function}

Create a callback function, which will be invoked when a client tries to
connect to the driver. In this callback function, add the binding from
the `ServerBindingGroup<>` object to a `ServerEnd` object, for example:

```cpp
class RetrieverDriver final : public fdf::DriverBase,
                              public fidl::Server<fuchsia_examples_metadata::Retriever> {
  ...

  void Serve(fidl::ServerEnd<fuchsia_examples_metadata::Retriever> request) {
    bindings_.AddBinding(dispatcher(), std::move(request), this, fidl::kIgnoreBindingClosure);
  }
```

(Source: [`retriever-driver.cc`][retriever-driver-cc])

## 4. Initialize a devfs connector {:#initialize-a-devfs-connector}

Initialize a `devfs` connector object by passing the target FIDL protocol
in [step 1](#identify-the-target-fidl-protocol) and the callback function
created in [step 3](#create-a-callback-function), for example:

```cpp
  driver_devfs::Connector<fuchsia_examples_metadata::Retriever> devfs_connector_{
      fit::bind_member<&RetrieverDriver::Serve>(this)};
```

(Source: [`retriever-driver.cc`][retriever-driver-cc])

## 5. Bind the devfs connector to a dispatcher {:#bind-the-devfs-connector-to-a-dispatcher}

Bind the `devfs` connector to the driver's [dispatcher][dispatcher], for example:

```cpp
zx::result connector = devfs_connector_.Bind(dispatcher());
    if (connector.is_error()) {
      fdf::error("Failed to bind devfs connector: {}", connector);
      return connector.status_value();
    }
```

(Source: [`retriever-driver.cc`][retriever-driver-cc])

## 6. Add the devfs connector to a child node {:#add-the-devfs-connector-to-a-child-node}

When adding a child node, pass the bound `devfs` connector from
[step 5](#bind-the-devfs-connector-to-a-dispatcher) to the node's
`NodeAddArgs` object. To do so, create a `DevfsAddArgs` object, which is
defined in [`fuchsia.driver.framework/topology.fidl`][topology-fidl], and
set the `connector` field to the `devfs` connector, for example:

```cpp
fuchsia_driver_framework::DevfsAddArgs devfs_args{
  {.connector = std::move(connector.value())}
};

zx::result owned_child = AddOwnedChild("retriever", devfs_args);
if (owned_child.is_error()) {
  fdf::error("Failed to add owned child: {}", owned_child);
  return owned_child.error_value();
}
```

(Source: [`retriever-driver.cc`][retriever-driver-cc])

<!-- Reference links -->

[devfs]: /docs/concepts/drivers/driver_communication.md
[driver-communication]: /docs/concepts/drivers/driver_communication.md
[example-driver]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/
[retriever-driver-cc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/metadata/retriever/retriever-driver.cc
[retriever-protocol]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/metadata/fuchsia.examples.metadata/fuchsia.examples.metadata.fidl
[dispatcher]: /docs/concepts/drivers/driver-dispatcher-and-threads.md
[topology-fidl]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.driver.framework/topology.fidl
