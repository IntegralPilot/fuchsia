# Driver communication

In Fuchsia, all communication occurs over
[capabilities](/docs/concepts/components/v2/capabilities/README.md), such as
protocols and services. The component framework handles the routing of
capabilities between components, which includes both drivers and non-drivers.
Capabilities are added to one component's outgoing directory, and if properly
routed will be available in another component's incoming namespace.

Drivers communicate with other drivers and non-drivers primarily using
[service capabilities](/docs/concepts/components/v2/capabilities/service.md)
(which this doc will refer to as "services" for brevity). There are
only two differences between service communication involving drivers and service
communication between non-drivers, and it involves how services are routed.

  1. Driver-to-driver routing uses dynamic capability routing where the Driver
     Manager creates routes from a parent node to a child node during the child
     driver's component creation.
  2. Services originating from drivers and passing to non-drivers are
     **Aggregated**, because all drivers reside in
     [collections](/docs/concepts/components/v2/realms.md#collections).  This
     means that all service instances exposed by drivers will be aggregated
     into the same directory named after the service name

Note: Some components still use [devfs](#legacy-devfs) to communicate with drivers.
This method of discovering and communicating with drivers is
deprecated. To migrate a driver or client from devfs to use services, go to:
[Devfs Driver Deprecation][devfs-migration-guide]

## The anatomy of a service {:#what-is-a-service}

A service instance represents a directory with one or more protocols. The
contained protocols are called service members. The service member has its own
type, which can then be used in tools like
[`ServiceMemberWatcher`][servicememberwatcher]{:external} to indicate not only
the service, but the specific protocol therein.

Consider the following FIDL definition:

```fidl
library fuchsia.example;

protocol Echo { ... }
protocol Broadcast { ... }

// Note: most services only contain one protocol.
service EchoService {
    speaker client_end:Broadcast;
    loopback client_end:Echo;
};
```

The following describes the values from the example:

- `fuchsia.example.EchoService` is the service capability
  - This is used in `.cml` files and is the name of the service directory
- `fuchsia_example::EchoService` is the c++ type of the service
- `fuchsia_example::EchoService::Speaker` is the c++ type of the **service member**
  - This type is really only used by tools like `ServiceMemberWatcher`.
  - This service member will appear in the `<instance_name>` directory as `speaker`
  - connecting to `/svc/fuchsia.example.EchoService/<instance_name>/speaker` will
    give you a channel that expects the protocol `fuchsia_example::Broadcast`.

## Driver side: Advertising a service

### Create server implementation for each protocol {:#create-server .numbered}

For each protocol in your service, you must have an server implementation of the
fidl protocol - a class that inherits from `fidl::WireServer` or `fidl::Server`.
If your service has multiple members with the same protocol, you can use the same
server implementation for each protocol.

### Create ServiceInstanceHandler  {:#create-handler .numbered}

Next you will need to create a `ServiceInstanceHandler`: a set of functions that
are called whenever a client connects to a protocol of your service. Fortunately,
`fidl::ServerBindingGroup` makes this very easy.

Add the binding group to your server class:

```cpp
fidl::ServerBindingGroup<fuchsia_examples::Echo> loopback_bindings_;
fidl::ServerBindingGroup<fuchsia_examples::Broadcast> speaker_bindings_;
```

You can then create a `ServiceInstanceHandler`. `this` in this example points
to the service instance you identified in the previous step.

```cpp
  fuchsia_examples::EchoService::InstanceHandler handler({
      .loopback = loopback_bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure),
      .speaker = speaker_bindings_.CreateHandler(this, dispatcher(), fidl::kIgnoreBindingClosure),
  });
```

Note that you will need to have a separate `ServerBindingGroup` or at least
`CreateHandler` call for each protocol within the service. (Most services only
have one protocol.)

### Advertise the service {:#advertise-service .numbered}

```cpp
zx::result add_result =
    outgoing()->AddService<fuchsia_examples::EchoService>(std::move(handler));
```

### Driver to Driver: Add offer when creating the child

If the service you just advertised should be routed to your child, you need to
add it to the offers you pass in when creating the child. For this example:

```cpp
  // Add a child with a `fuchsia_examples::EchoService` offer.
  std::vector<fuchsia_driver_framework::NodeProperty2> properties = {};
  zx::result child_result = AddChild("my_child_name", properties,
      std::array{fdf::MakeOffer2<fuchsia_examples::EchoService>()});
```

This instructs the Driver Manager to route that service from you to your child.
Since the instance name will not be randomized, it is recommended to specify the
instance name as the name of the child component as an argument to `AddService`.

## Route the service

Now that your service is advertised, it needs to be `expose`d from your driver,
and `use`d by your client. If the client is a child driver, then no additional
routing is necessary. If the client is a non-driver component, then you must
`expose` the service up to an ancestor node both the client and driver share,
(usually the `#core` component), then `offer`ed down to the client.

### Expose the service from your driver

You must add the service to the cml file for your driver, to both the
`Capability` and `Expose` fields. The capabilities stanza defines a capability,
and the expose specifies the source of the capability.

```fidl
capabilities: [
    { service: "fuchsia.examples.EchoService" },
],
expose: [
    {
        service: "fuchsia.examples.EchoService",
        from: "self",
    },
],
```

### Route the service

Between your driver and your component, add the service to `offer` and `expose`
fields. You will most probably need to modify multiple `cml` files. See the
examples below for how other drivers have routed their services. Take note of
which realm your service needs to reach. For example, for components in the
`bootstrap` realm, your service must come from `#boot-drivers` or `#base-drivers`.

Note: This step can be skipped if the service is being routed between
drivers by the Driver Manager.

```
{
      service: "fuchsia.example.EchoService",
      from: "parent",
      to: "#my_client",
},
```

See the following examples for how other drivers have routed their services:

 - [Routing usb-peripheral to a command-line tool][usb-peripheral-routing-cl]{:.external}
 - [Routing cpu.ctrl to command-line tool][cpu-ctl-routing-cl]{:.external}
 - [Routing vsock class][vsock-migration-cl]{:.external}

More information about routing capabilities can be found on the
[Connect Components][connect-components] page. If you run into problems, the
[troubleshooting][troubleshoot-routes] section may be helpful, as well as the
[debugging][devfs-migration-debugging] section of the
[Devfs Migration Guide][devfs-migration-guide].

Note: If you are [migrating from devfs][devfs-migration-guide], your service
should be routed to `#core` already. This also means that tests using
`DriverTestRealm` do not need to add routing in `.cml` files.

### `Use` the service in your client

In your client, add the service to the 'use' list:

```
use: [
    { service: "fuchsia.example.EchoService", },
 ],
```

## Connect to a service as a client

A service is just a directory with protocols inside it, made available by the
component framework in the `/svc/` directory by service name. Therefore, you
can connect to the protocols offered by the service at:

```
/svc/<ServiceName>/<instance_name>/<ServiceMemberName>
```

For the example in Step 1, this would be:

```
/svc/fuchsia.example.EchoService/<instance_name>/loopback
   and
/svc/fuchsia.example.EchoService/<instance_name>/speaker
```

The instance name is randomly generated.

To connect to a service member, the recommended approach is to watch the
service directory for instances to appear. There are various tools that can
assist you with this, but for services with a single protocol,
[`ServiceMemberWatcher`][servicememberwatcher]{:.external} is recommended for
C++ and `Service` is recommended for Rust.


*  {Synchronous C++}

   ```cpp
   SyncServiceMemberWatcher<fuchsia_examples::EchoService::Loopback> watcher;
   zx::result<ClientEnd<fuchsia_examples::Echo>> result = watcher.GetNextInstance(true);
   ```

*  {Asynchronous C++}

   ```cpp
   #include <lib/component/incoming/cpp/service_watcher.h>
   using component::SyncServiceMemberWatcher;
   // Define a callback function:
   void OnInstanceFound(ClientEnd<fuchsia_examples::Echo> client_end) {...}
   // Optionally define an idle function, which will be called when all
   // existing instances have been enumerated:
   void AllExistingEnumerated() {...}
   // Create the ServiceMemberWatcher:
   ServiceMemberWatcher<fuchsia_examples::EchoService::Loopback> watcher;
   watcher.Begin(get_default_dispatcher(), &OnInstanceFound, &AllExistingEnumerated);
   // If you want to stop watching for new service entries:
   watcher.Cancel()
   ```

*  {Rust}

   ```rust
     use fuchsia_component::client::Service;
     let device = Service::open(fuchsia_examples::EchoServiceMarker)
         .context("Failed to open service")?
         .watch_for_any()
         .await
         .context("Failed to find instance")?
         .connect_to_device()
         .context("Failed to connect to device protocol")?;
   ```

You should now be able to access your driver's service.

## Appendix

### Using Services with DriverTestRealm {:#using-services-with-dtr}

Test clients using the DriverTestRealm can use the following steps to route the
service capability from the driver under test out to the test code.

 1. Setup the driver test realm with `driver_exposes` in your options:

    *  {C++}

        ```cpp
        async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
        std::vector<fuchsia_component_test::Capability> exposes = { {
            fuchsia_component_test::Capability::WithService(
                fuchsia_component_test::Service{ {.name = fuchsia_examples::EchoService::Name}}),
        }};
        auto realm_builder = component_testing::RealmBuilder::Create();
        driver_test_realm::Setup(
            realm_builder,
            loop.dispatcher(),
            driver_test_realm::OptionsBuilder().driver_exposes(exposes).Build(),
            {});

        auto realm = realm_builder.Build(loop.dispatcher());
        ```

    *  {Rust}

        ```rust
        let echo_capability = fuchsia_component_test::Capability::service::<fuchsia_examples::EchoServiceMarker>().into();
        let exposed_capabilities = vec![echo_capability];

        // Create the RealmBuilder.
        let builder = RealmBuilder::new().await?;
        builder.driver_test_realm_setup(Options::new().driver_exposes(exposed_capabilities), {}).await?;

        // Build the Realm.
        let realm = builder.build().await?;
        ```

 2. Connect to the realm's `exposed()` directory to wait for a service instance:

    *  {C++}

        ```cpp
        fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir{
            realm.component().exposed().unowned_channel()->get()};
        component::SyncServiceMemberWatcher<fuchsia_examples::EchoService::Device> watcher(
            svc_dir);
        // Wait indefinitely until a service instance appears in the service directory
        zx::result<fidl::ClientEnd<fuchsia_examples::Echo>> echo_client =
            watcher.GetNextInstance(false);
        ```

    *  {Rust}

        ```rust
        use fuchsia_component::client::Service;
        // Connect to the `Device` service.
        let device = Service::open_from_dir(realm.root.get_exposed_dir(), fuchsia_examples::EchoServiceMarker)
            .context("Failed to open service")?
            .watch_for_any()
            .await
            .context("Failed to find instance")?
            .connect_to_device()
            .context("Failed to connect to device protocol")?;
        ```

Examples: The code in this section is from the following CLs:

 - [C++ Updating usb-peripheral client][usb-peripheral-dtr-client-cl]{:.external}
 - [Rust Adding service client test][rust-dtr-cl]{:.external}

### Legacy driver communication using devfs {:#legacy-devfs}

The [driver manager][driver-manager] hosts a virtual filesystem named `devfs`
(as in "device filesystem"). This virtual filesystem provides uniform access to
all driver services in a Fuchsia system to Fuchsia’s user-space services
(that is, components external to the drivers). These non-driver components
establish initial contacts with drivers by discovering the services of the
target drivers in `devfs`.

Strictly speaking, `devfs` is a directory capability exposed by the driver
manager. Therefore, by convention, components that wish to access drivers mount
`devfs` under the `/dev` directory in their namespace (although it’s not
mandated that `devfs` to be always mounted under `/dev`).

`devfs` hosts virtual files that enable Fuchsia components to route messages to
the interfaces implemented by the drivers running in a Fuchsia system.
In other words, when a client (that is, a non-driver component) opens a file
under the `/dev` directory, it receives a channel that can be used to make
FIDL calls directly to the driver mapped to the file. For example,
a Fuchsia component can connect to an input device by opening and writing to
a file that looks like `/dev/class/input-report/000`. In this case,
the client may receive a channel that speaks the `fuchsia.input.report` FIDL.

Drivers can use the [`DevfsAddArgs`][devfs-add-args] table to export
themselves into `devfs` when they add a new node.

The following events take place for non-driver to driver communication:

1. To discover driver services in the system, a non-driver component scans the
   directories and files in `devfs`.
2. The non-driver component finds a file in `devfs` that represents a service
   provided by the target driver.
3. The non-driver component opens this file which establishes a connection
   with the target driver.
4. After the initial contact, a FIDL connection is established between the
   non-driver component and the driver.
5. From this point, all communication takes place over the FIDL channels.

Note: Use of devfs is discouraged in new code. Whenever a driver exposes a protocol
in devfs, the Driver Manager automatically exposes the corresponding service
as well, using a look-up table in [class_names.h][class-names]. This allows
any client to [switch from devfs to services][devfs-migration-guide] with minimal
effort.


<!-- Reference links -->

<!-- Code links -->
[servicememberwatcher]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/component/incoming/cpp/service_member_watcher.h
[class-names]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/bin/driver_manager/devfs/class_names.h;l=48

<!-- Other docs -->
[devfs-migration-guide]: /docs/contribute/open_projects/drivers/devfs_to_service.md
[devfs-migration-debugging]: /docs/contribute/open_projects/drivers/devfs_to_service.md#debugging

[driver-manager]: driver_framework.md#driver_manager
[driver-runtime]: driver_framework.md#driver_runtime
[node-topology]: drivers_and_nodes.md#node_topology
[devfs-add-args]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.driver.framework/topology.fidl
[components]: /docs/concepts/components/v2/README.md
[connect-components]: /docs/development/components/connect.md
[troubleshoot-routes]: /docs/development/components/connect.md#troubleshooting-troubleshooting

<!-- Gerrit links -->
[usb-peripheral-dtr-client-cl]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1199584
[usb-peripheral-routing-cl]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1200394
[cpu-ctl-routing-cl]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1028574
[rust-dtr-cl]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1203869/8/src/devices/tests/v2/services/test.rs
[vsock-migration-cl]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1213569
