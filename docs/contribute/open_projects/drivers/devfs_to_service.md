# Devfs migration guide

## Goal & motivation

Currently most driver access by non-drivers is mediated by the Device File
System (devfs). However devfs is being deprecated. For a detailed background
on why devfs is being deprecated, please refer to the [Devfs Deprecation
RFC](/docs/contribute/governance/rfcs/0263_migrate_driver_communication_to_services.md).
As a brief summary:

 - It is difficult to restrict clients of devfs to specific protocols.
 - The API makes it difficult for drivers to expose more than one protocol.
 - Topological paths and hardcoded class paths expose clients to internal
   implementation details and can result in unintentional breakages.
 - Since devfs connections are unknown to the component manager, they cannot be
   used for dependency tracking

The migration from devfs to services changes the way drivers are accessed by
non-drivers, by switching from a directory in `/dev/class` to using aggregated
services that are mediated by the component manager.

## Technical background

Contributors should be familiar with drivers, FIDL clients and capability routing using
the component framework.

## Picking a task

Each entry in `/dev/class` represents a class to migrate. A full listing of
class names, as well as the services to which they will be migrated can be found in:
[src/devices/bin/driver_manager/devfs/class_names.h][class-names]{:.external}.

Each class has at least one driver and one client, sometimes many. Classes
should ideally be migrated as a a single effort, but each client and driver can
be safely migrated as separate CLs without breakage.

Note that there some clients have features that will add some complexity to the migration.
To find out if your clients will require extra steps to migrate, see:
[Does My Client Require Extra Migration Steps](#extra-steps)

# Migrating a devfs class

Migrating a devfs class is split into multiple steps, which allows non-breaking
changes for out-of-tree drivers and clients. Where warranted, these steps can
be combined into a single CL. The final step should always be run with
`lcs presubmit`, `MegaCQ` and `Run-All-Tests: true` to ensure all clients are
converted.

To migrate a devfs class:

 1. [Verify that devfs is automatically advertising services for your class](#step1)
 2. [Convert clients to use services](#migrate-clients)
 3. [Convert drivers to advertise a service](#migrate-drivers)
 4. [Remove the service and class name from devfs](#cleanup)

## Verify devfs is automatically advertising services for your class {:#step1 .numbered}

Check for your class name in
[class_names.h][class-names]{:.external}.
Check that the service and member name is correct.

For example, if you have a class name of `echo-example` that gives you access
to the protocol `fuchsia.example.Echo`, and have the fidl file:

```fidl
  library fuchsia.example;

  protocol Echo { ... }

  service EchoService {
      my_device client_end:Echo;
  };
```

Your entry in `class_names.h` should be:

```
{"echo-example", {ServiceEntry::kDevfsAndService, "fuchsia.example.EchoService", "my_device"}},
```

If you are confused about the naming, see [What's a Service
Member?](#what-is-a-service-member)

If your service is listed and correct, it should be automatically advertised
whenever a device with the corresponding class name is published to the
`/dev/class` folder. The service capability is also routed out of the driver
collection and over to the `#core` component.

Note: If you do change the service name you will have to update the `.cml`
files to have devfs export the capability correctly. See [this
commit][devfs-driver-routing-cl]{:.external} for where
the service name needs to be updated.

## Convert client to use services {:#migrate-clients .numbered}

Client migration involves the following steps:

 1. [Check if your client requires extra steps](#extra-steps)
 2. [Change the capability routed from a `dev-class` directory to a service](#update-capability-routes)
 3. [Connect to the service instance instead of the devfs instance](#connect-to-service)


### Does my client require extra migration steps? {:#extra-steps .numbered}

Some client migrations may take extra steps, depending on how they use devfs:

 - If your client uses topological paths (starting with `/dev/sys`) or depends
   on sequentially ordered instance names (such as `/dev/class/block/000`),
   follow the instructions at
   [Identifying Service Instances][identifying-service-instances] to
   identify your client's desired service instance correctly.
 - If you are migrating a test that uses `DriverTestRealm` follow the
   instructions at [Using Services with DriverTestRealm](#using-services-with-dtr)

Once you have taken the steps outlined above, you may proceed with client
migration. Migration involves 2 steps:

### Change the capability routed from a `dev-class` directory to a service {:#update-capability-routes .numbered}

Devfs access is granted by a `directory` capability. Services capabilities use
the tag `service`. You will need to update several `.cml` files depending on
the route from the driver to your component.

The changes will generally have this form:

#### Parent of the component
<table>
<tr><th>Devfs</th><th>Services</th></tr>
<tr>
<td><pre><code class="language-c"> {
      directory: "dev-class",
      from: "parent",
      as: "dev-echo-example",
      to: "#timekeeper",
      subdir: "echo-example",
  },
</code></pre></td>
<td><pre><code class="language-c"> {
      service: "fuchsia.example.EchoService",
      from: "parent",
      to: "#timekeeper",
  },
</code></pre></td>
</tr></table>

#### Client component
<table>
<tr><th>Devfs</th><th>Services</th></tr>
<tr>
<td><pre><code class="language-c"> use: [
  {
    directory: "dev-echo-service",
    rights: [ "r*" ],
    path: "/dev/class/echo-service",
  },
 ],
</code></pre></td>
<td><pre><code class="language-c"> use: [
    { service: "fuchsia.example.EchoService", },
 ],
</code></pre></td>
</tr></table>

Example CLs:

 - [Routing usb-peripheral to a command-line tool][usb-peripheral-routing-cl]{:.external}
 - [Routing cpu.ctrl to command-line tool][cpu-ctl-routing-cl]{:.external}

More information about routing capabilities can be found on the
[Connect Components][connect-components] page.  If you run into problems, the
[troubleshooting][troubleshoot-routes] section may be helpful, as well as the
[debugging](#debugging) section below.

Note: If you followed [step 1](#step1), your service should be routed to `#core` already.
This also means that tests using `DriverTestRealm` do not need to add routing
in `.cml` files.

### Connect to the service instance instead of the devfs instance {#connect-to-service .numbered}

When using devfs, you would connect to a **protocol** at:

```
/dev/class/<class_name>/<instance_name>
```

A service is just a directory with protocols inside it, made available by the
component framework in the `/svc/` directory by service name. Therefore, you
can connect to the protocol offered by the service at:

```
/svc/<ServiceName>/<instance_name>/<ServiceMemberName>
```

For the example in Step 1, this would be:

```
/svc/fuchsia.example.EchoService/<instance_name>/my_device
```

For both devfs and services, the recommended approach is to watch the
appropriate directory for instances to appear. There are various tools that
can assist you with this, and your client is most likely using one already:

 - `std::filesystem::directory_iterator` (although this is prohibited in the style guide)
 - `fsl::DeviceWatcher`
 - A new tool has also been added specifically for services:
 [`ServiceMemberWatcher`][servicememberwatcher]{:.external}

You can either continue to use your existing tool, or (recommended) switch to using the
[`ServiceMemberWatcher`][servicememberwatcher]{:.external}  to benefit from type checking.


#### Recommended: Use `ServiceMemberWatcher`

[`ServiceMemberWatcher`][servicememberwatcher]{:.external} performs type checking
to get the service and member names automatically. It can be used
synchronously and asynchronously to connect directly to the protocols of
single-protocol services.  The rust equivalent is `Service`

*  {Synchronous C++}

   ```cpp
   SyncServiceMemberWatcher<fuchsia_examples::EchoService::MyDevice> watcher;
   zx::result<ClientEnd<fuchsia_examples::Echo>> result = watcher.GetNextInstance(true);
   ```

*  {Asynchronous C++}

   ```cpp
   // Define a callback function:
   void OnInstanceFound(ClientEnd<fuchsia_examples::Echo> client_end) {...}
   // Optionally define an idle function, which will be called when all
   // existing instances have been enumerated:
   void AllExistingEnumerated() {...}
   // Create the ServiceMemberWatcher:
   ServiceMemberWatcher<fuchsia_examples::EchoService::MyDevice> watcher;
   watcher.Begin(get_default_dispatcher(), &OnInstanceFound, &AllExistingEnumerated);
   // If you want to stop watching for new service entries:
   watcher.Cancel()
   ```

*  {Rust}

   ```rust
     let device = Service::open(fidl_examples::EchoServiceMarker)
         .context("Failed to open service")?
         .watch_for_any()
         .await
         .context("Failed to find instance")?
         .connect_to_device()
         .context("Failed to connect to device protocol")?;
   ```

Examples:

 - [Converting adb client][adb-servicememberwatcher-cl]{:.external}
 - [Migrating vsock][vsock-migration-cl]{:.external}

#### Alternate Option: Change watched directory and add service member folder to existing code

You can update existing code by changing just a few lines:
 - Watch the directory `/svc/<ServiceName>` instead of `/dev/class/<class_name>`
 - After finding an instance, connect to the service member folder entry,
   rather than the instance folder itself.


*  {C++}

    ```diff
    using std::filesystem;
    - constexpr char kDevicePath[] = "/dev/class/echo-example";
    + constexpr char kServiceName[] = "/svc/fuchsia.example.EchoService";
    + const std::filesystem::path kServiceMember = "my_device";
    - for (auto& dev_path : std::filesystem::directory_iterator(kDevicePath)) {
    + for (auto& instance_path : std::filesystem::directory_iterator(kServiceName)) {
    +     directory_entry dev_path({instance_path / kServiceMember});
      auto dev = component::Connect<i2c::Device>(dev_path.path().c_str());

      ...
    }
    ```

*  {Rust}

    ```diff
    -const ECHO_DIRECTORY: &str = "/dev/class/echo-example";
    +const ECHO_DIRECTORY: &str = "/svc/fuchsia.example.EchoService";
    +const ECHO_MEMBER_NAME: &str = "/my_device";
    let mut dir = std::fs::read_dir(ECHO_DIRECTORY).expect("read_dir failed")?;
    let entry = dir.next()
        .ok_or_else(|| anyhow!("No entry in the echo directory"))?
        .map_err(|e| anyhow!("Failed to find echo device: {e}"))?;
    let path = entry.path().into_os_string().into_string()
        .map_err(|e| anyhow!("Failed to parse the device entry path: {e:?}"))?;

    - fdio::service_connect(&path, server_end.into_channel())
    + fdio::service_connect(&(path + ECHO_MEMBER_NAME), server_end.into_channel())
    ```


## Convert Drivers to advertise a service {:#migrate-servers .numbered}

Note: All clients of a class should be migrated to services before migrating
the drivers, as migrating the drivers may make the dev/class entries
unavailable.

When using Services, it's no longer necessary to use `DdkAdd` (dfv1)
or `AddOwnedChildNode` (dfv2) in order to publish the instance. Instead, you can
just publish the service instance at any point as it's tied to the driver
instance, rather than a particular device/node. However, all non-dynamically
enumerated service instances should be enumerated prior to completing the
start hook in dfv2 and the init hook in dfv1. Of course, if you expect drivers
to bind to your driver, you still need to add devices/nodes for that purpose.

### Identify protocol server implementation {:#identify-server .numbered}

Converting the driver differs between DFv1 and DFv2 drivers, but in both cases
you should already have a class that acts as the server implementation of a
protocol. It may inherit from a `fidl::WireServer` or `fidl::Server`, or in DFv1
it may use the mixin: `ddk::Messageable<Protocol>::Mixin`. *ddk::Messageable
is deprecated howwever, so please do not use it in new code.*

### Create ServiceInstanceHandler  {:#create-handler .numbered}

Next you will need to create a `ServiceInstanceHandler`: a function that is
called whenever someone connects to your service. Fortunately,
`fidl::ServerBindingGroup` makes this very easy.

Add the binding group to your server class:

```cpp
fidl::ServerBindingGroup<fuchsia_examples::Echo> bindings_;
```

You can then create a `ServiceInstanceHandler`.  `this` in this example points
to the service instance you identified in the previous step.

```cpp
  fuchsia_examples::EchoService::InstanceHandler handler({
      .my_device = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(), fidl::kIgnoreBindingClosure),
  });
```

Note that you will need to have a separate `ServerBindingGroup` or at least
`CreateHandler` call for each protocol within the service. (Most services only
have one protocol.) In this example, `device` is the name of the member
protocol in the service definition.

### Advertise the service {:#advertise-service .numbered}

*   {DFv1}

  ```cpp
  zx::result add_result =
        DdkAddService<fuchsia_examples::EchoService>(std::move(handler));
  ```

  You no longer need to add a device to advertise a service. However dfv1 still
  requires that you add at least 1 device before returning from the bind hook,
  so be careful about removing all devices.

*  {DFv2}

  ``` cpp
  zx::result add_result =
      outgoing()->AddService<fuchsia_examples::EchoService>(std::move(handler));
  ```

  To stop advertising to devfs, you will want to delete all the `DevfsAddArgs`.
  You can also delete your `driver_devfs::Connector` class as well as the
  `Serve` function it called. Keep the `fidl::ServerBindingGroup` for your
  protocol though.

  ```diff
  - zx::result connector = devfs_connector_.Bind(async_dispatcher_);
  - auto devfs = fuchsia_driver_framework::wire::DevfsAddArgs::Builder(arena)
  -                 .connector(std::move(connector.value()))
  -                 .class_name("echo-example");
 auto offers = compat_server_.CreateOffers2();
 offers.push_back(fdf::MakeOffer2<fuchsia_example::EchoService>());
 zx::result result = AddChild(kDeviceName,
-                               devfs.Build(),
                               *properties, offers);
  ```

### Expose the service from your driver  {:#expose-service .numbered}

 You must add the service to the cml file for your driver, to both the
 `Capability` and `Expose` fields:

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

If devfs was already advertising the service to your clients, then adding the
above entries into your driver's `.cml` should be the only capability routing
change needed.

## Cleanup {:#cleanup .numbered}

Once all drivers and clients are migrated to services, you can delete the class
name entry in
[src/devices/bin/driver_manager/devfs/class_names.h][class-names]{:.external}.
This will stop devfs from advertising the `/dev/class/<class_name>` directory,
as well as the service it represents. You can also remove the service
capability from `src/devices/bin/driver_manager/devfs/meta/devfs-driver.cml`

Note: When deleting the class entry, you must be sure to run all tryjobs and tests
by running `lcs presubmit`, Adding `MegaCQ` and adding the `Run-All-Tests: true`
statement to your commit message. CQ will normally not run enough tests to
catch any stray clients. In addition, try to make sure that there are no
unconverted command-line applications that use the `/dev/class/` path you are
about to remove.

## Appendix

### What Is a Service Member? {:#what-is-a-service-member}

A service member refers to a specific protocol within a service. The service
member has its own type, which can then be used in tools like `ServiceMemberWatcher`
to indicate not only the service, but the specific protocol therein.

Consider the following fidl definition:

```fidl
library fuchsia.example;

protocol Echo { ... }
protocol Broadcast { ... }

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


### Using Services with DriverTestRealm {:#using-services-with-dtr}

Test clients using the DriverTestRealm use the following steps to route the
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
        driver_test_realm::Setup(realm_builder,
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
        // Connect to the `Device` service.
        let device = client::Service::open_from_dir(realm.root.get_exposed_dir(), fuchsia_examples::EchoServiceMarker)
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


## Examples

Examples have been linked throughout the guide, but compiled here for easy
reference:

- [Migrating overnet-usb (all in one CL)][overnet-usb-cl]{:.external}
- Migrating usb-peripheral
  - [Migrate DriverTestRealm client][usb-peripheral-dtr-client-cl]{:.external}
  - [Add routing and migrate command-line client][usb-peripheral-routing-cl]{:.external}
  - [Migrate Driver and cleanup][usb-peripheral-driver-cl]{:.external}
- Migrating usb-ctrl (older, not as recommended)
  - [Migrate client][cpu-ctl-client-cl]{:.external}
  - [Route capability to clients][cpu-ctl-routing-cl]{:.external}
  - [Migrating DFv2 Driver][cpu-ctl-dfv2-driver-cl]{:.external}
  - [Migrating Dfv1 Driver (uses more boilerplate than current method)][cpu-ctl-dfv1-driver-cl]{:.external}

## Debugging

The most common issue with migrating to services is not connecting all the capabilities
up correctly. Look for errors in your logs similar to:

```none {:.devsite-disable-click-to-copy}
WARN: service `fuchsia.example.EchoService` was not available for target `bootstrap/boot-drivers:PCI0`:
	`fuchsia.example.EchoService` was not offered to `bootstrap/boot-drivers:PCI0` by parent
For more, run `ffx component doctor bootstrap/boot-drivers:PCI0`
```

You can also check your component routing while the system is running.
`ffx component` offers a number of useful tools to diagnose routing issues:

- Call `ffx component list` to get a list of the component names. The `/` indicates
a parent->child relationship, which is useful for understanding the component topology.
- Call `ffx component capability <ServiceName>` to see who touches that service
- `ffx component doctor <your_client>` will list the capabilities your client uses
  and exposes, and gives some indications of what went wrong if routing fails.

Note: The `ffx component` tool does not know about the routing between DriverTestRealm
and the test component.

More information can be found on the [Connect Components][connect-components]
page, especially under [Troubleshooting][troubleshoot-routes].

## Sponsors

Reach out for questions or for status updates:

*   <garratt@google.com>
*   <surajmalhotra@google.com>
*   <fuchsia-drivers-discuss@google.com>

<!-- Reference links -->
<!-- Other Docs -->
[identifying-service-instances]: /docs/contribute/open_projects/drivers/identifying_service_instances.md
[connect-components]: /docs/development/components/connect.md
[troubleshoot-routes]: /docs/development/components/connect.md#troubleshooting-troubleshooting
<!-- Code links -->
[class-names]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/bin/driver_manager/devfs/class_names.h;l=48
[servicememberwatcher]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/component/incoming/cpp/service_member_watcher.h
<!-- Gerrit links -->
[overnet-usb-cl]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1200181
[usb-peripheral-dtr-client-cl]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1199584
[usb-peripheral-routing-cl]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1200394
[usb-peripheral-driver-cl]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1200182
[cpu-ctl-client-cl]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1065241
[cpu-ctl-routing-cl]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1028574
[cpu-ctl-dfv2-driver-cl]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1028575
[cpu-ctl-dfv1-driver-cl]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1064310
[usb-peripheral-routing-cl]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1200394
[devfs-driver-routing-cl]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1205609
[adb-servicememberwatcher-cl]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1197984
[rust-dtr-cl]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1203869/8/src/devices/tests/v2/services/test.rs
[vsock-migration-cl]: https://fuchsia-review.git.corp.google.com/c/fuchsia/+/1213569

