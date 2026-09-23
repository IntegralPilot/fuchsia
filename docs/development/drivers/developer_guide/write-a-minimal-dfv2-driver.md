# Write a minimal DFv2 driver

This guide walks through the steps involved in creating a minimal DFv2 driver.

The instructions in this guide are based on the minimal
[skeleton driver][skeleton-driver], which provides the minimum implementation
necessary to build, load, and register a new DFv2 driver in a Fuchsia system.

The steps are:

1. [Create a driver header file](#create-a-driver-header-file).
1. [Create a driver source file](#create-a-driver-source-file).
1. [Add the driver export macro](#add-the-driver-export-macro).
1. [Create a build file](#create-a-build-file).
1. [Write bind rules](#write-bind-rules).
1. [Create a driver component](#create-a-driver-component).

For more DFv2-related features, see [Additional tasks](#additional-tasks).

## Create a driver header file {:#create-a-driver-header-file .numbered}

To create a header file for your DFv2 driver, do the following:

1. Create a new header file (`.h`) for the driver (for example,
   `skeleton_driver.h`).

1. Include the following interface to the header file:

   ```cpp
   #include <lib/driver/component/cpp/driver_base2.h>
   ```

1. Add an interface for the [`DriverBase2`][driver-base] class,
   for example:

   ```cpp
   #include <lib/driver/component/cpp/driver_base2.h>

   namespace skeleton {

   class SkeletonDriver : public fdf::DriverBase2 {
    public:
     SkeletonDriver();

     // Called by the driver framework to initialize the driver instance.
     zx::result<> Start(fdf::DriverContext context) override;
   };

   }  // namespace skeleton
   ```

   (Source: [`skeleton_driver.h`][skeleton-driver-h])

## Create a driver source file {:#create-a-driver-source-file .numbered}

To implement the basic methods for the `DriverBase2` class,
do the following:

1. Create a new source file (`.cc`) for the driver (for example,
   `skeleton_driver.cc`).

1. Include the header file created for the driver, for example:

   ```cpp
   #include "skeleton_driver.h"
   ```

1. Implement the basic methods for the class, for example:

   ```cpp
   #include "skeleton_driver.h"

   namespace skeleton {

   SkeletonDriver::SkeletonDriver() : DriverBase2("skeleton_driver") {}

   zx::result<> SkeletonDriver::Start(fdf::DriverContext context) {
     return zx::ok();
   }

   }  // namespace skeleton
   ```

   (Source: [`skeleton_driver.cc`][skeleton-driver-cc])

   This driver constructor needs to pass the driver name (for example,
   `"skeleton_driver"`) to the `DriverBase2` class.

## Add the driver export macro {:#add-the-driver-export-macro .numbered}

To add the driver export macro, do the following:

1. In the driver source file, include the following header file:

   ```cpp
   #include <lib/driver/component/cpp/driver_export2.h>
   ```

1. Add the following macro (which exports the driver class) at the
   bottom of the driver source file:

   ```cpp
   FUCHSIA_DRIVER_EXPORT2(skeleton::SkeletonDriver);
   ```

   For example:

   ```cpp
   #include <lib/driver/component/cpp/driver_base2.h>
   #include <lib/driver/component/cpp/driver_export2.h>

   #include "skeleton_driver.h"

   namespace skeleton {

   SkeletonDriver::SkeletonDriver() : DriverBase2("skeleton_driver") {}

   zx::result<> SkeletonDriver::Start(fdf::DriverContext context) {
     return zx::ok();
   }

   }  // namespace skeleton

   FUCHSIA_DRIVER_EXPORT2(skeleton::SkeletonDriver);
   ```

   (Source: [`skeleton_driver.cc`][skeleton-driver-cc])

## Create a build file {:#create-a-build-file .numbered}

To create a build file for the driver, do the following:

1. Create a new `BUILD.gn` file.
1. Include the following line to import the driver build rules:

   ```gn
   import("//build/drivers.gni")
   ```

1. Add a target for the driver, for example:

   ```gn
   fuchsia_cc_driver("driver") {
     output_name = "skeleton_driver"
     sources = [ "skeleton_driver.cc" ]
     deps = [
       "//sdk/lib/driver/component/cpp",
       "//src/devices/lib/driver:driver_runtime",
     ]
   }
   ```

   (Source: [`BUILD.gn`][build-gn])

   The `output_name` field must be unique among all drivers.

## Write bind rules {:#write-bind-rules .numbered}

To write bind rules for your driver, do the following:

1. Create a new bind rule file (`.bind`) for the driver
   (for example, `skeleton_driver.bind`) in the `meta` directory.

1. Add basic bind rules, for example:

   ```
   using gizmo.example;
   gizmo.example.TEST_NODE_ID == "skeleton_driver";
   ```

   (Source: [`skeleton_driver.bind`][skeleton-driver-bind])

1. In the `BUILD.gn` file, include the following line to import the
   bind build rules:

   ```gn
   import("//build/bind/bind.gni")
   ```

1. In the `BUILD.gn` file, add a target for the driver's bind rules,
   for example:

   ```gn
   driver_bind_rules("bind") {
     rules = "meta/skeleton.bind"
     bind_output = "skeleton_driver.bindbc"
     deps = [ "//examples/drivers/bind_library:gizmo.example" ]
   }
   ```

   (Source: [`BUILD.gn`][build-gn])

   The `bind_output` field must be unique among all drivers.

   Note: To learn more about finding node properties and writing
   bind rules, see [Bind Rules Tutorial][node-properties].

## Create a driver component {:#create-a-driver-component .numbered}

To create a Fuchsia component for the driver, do the following:

1. Create a new component manifest file (`.cml`) in the `meta`
   directory (for example, `skeleton_driver.cml`).

1. Include the following component shards:

   ```
   {
       include: [
           "inspect/client.shard.cml",
           "syslog/client.shard.cml",
       ],
   }
   ```

1. Add the driver's `program` information using the following format:

   ```
   {
       program: {
           runner: "driver",
           binary: "driver/<OUTPUT_NAME>.so",
           bind: "meta/bind/<BIND_OUTPUT>",
       },
   }
   ```

   The `binary` field must match the `output_name` field in the
   `fuchsia_driver` target of the `BUILD.gn` file, and the `bind`
   field must match `bind_output` in the `driver_bind_rules` target,
   for example:

   ```
   {
       include: [
           "inspect/client.shard.cml",
           "syslog/client.shard.cml",
       ],
       program: {
           runner: "driver",
           binary: "driver/skeleton_driver.so",
           bind: "meta/bind/skeleton.bindbc",
       },
   }
   ```

   (Source: [`skeleton_driver.cml`][skeleton-driver-cml])

1. Create a new JSON file to provide the component's information
   (for example, `component-info.json`) in the `meta` directory.

1. Add the driver component's information in JSON format, for example:

   ```json
   {
       "short_description": "Driver Framework example for a skeleton DFv2 driver",
       "manufacturer": "",
       "families": [],
       "models": [],
       "areas": [
           "DriverFramework"
       ]
   }
   ```

   (Source: [`component-info.json`][component-info-json])

1. In the `BUILD.gn` file, include the following line to import the component
   build rules:

   ```gn
   import("//build/components.gni")
   ```

1. In the `BUILD.gn` file, add a target for the driver component, for example:

   ```gn
   fuchsia_driver_component("component") {
     component_name = "skeleton"
     manifest = "meta/skeleton.cml"
     deps = [
        ":bind",
        ":driver"
     ]
     info = "component-info.json"
   }
   ```

   (Source: [`BUILD.gn`][build-gn])

   See the rules for these fields below:

   - Set the `manifest` field to the location of the driver's `.cml` file.
   - Set the `info` field to the location of the driver component
     information JSON file.
   - Set the `deps` array to include the `fuchsia_driver` and
     `driver_bind_rules` targets from the `BUILD.gn` file.

You can now build, load, and register this DFv2 driver in a Fuchsia system

## Additional tasks {:#additional-tasks}

Note: The instructions below are based on the [Simple driver][simple-driver].

This section provides additional features you can add to your minimal DFv2
driver:

- [Add logs](#add-logs)
- [Add a child node](#add-a-child-node)
- [Clean up the driver](#clean-up-the-driver)
- [Add a compat device server](#add-a-compat-device-server)

### Add logs {:#add-logs}

To print logs from a DFv2 driver, use the logging macros (`fdf::info`, `fdf::error`, `fdf::warn`, `fdf::debug`, `fdf::trace`). These macros use `std::format`-style string formatting.

To use logs from your DFv2 driver, do the following:

1. Include the following header:

   ```cpp
   #include <lib/driver/logging/cpp/logger.h>
   ```

1. Use the `fdf::` macros to print logs, for example:

   ```cpp
   fdf::info("Starting SimpleDriver");
   fdf::error("Failed to add child: {}", status);
   ```

Note: The Fuchsia environment supplies a specialized `std::formatter` for `zx::result` and `zx::status`. You can pass these objects directly to the logger without calling `.status_string()`.

In `BUILD.gn`, make sure you depend on the logging library:
```gn
deps = [
  "//sdk/lib/driver/logging/cpp",
]
```

### Add a child node {:#add-a-child-node}

A DFv2 driver can add child nodes using the following `Node` protocol in the
[`fuchsia.driver.framework`][topology-fidl] FIDL library:

```none {:.devsite-disable-click-to-copy}
open protocol Node {
    flexible AddChild(resource struct {
        args NodeAddArgs;
        controller server_end:NodeController;
        node server_end:<Node, optional>;
    }) -> () error NodeError;
};
```

To facilitate this, during startup the driver framework provides a client of
the bound node's `Node` protocol to the DFv2 driver, through the `DriverBase2`.
The driver can access its node client at any time to create child nodes on it.
However directly using this FIDL library requires a setup that includes
creating FIDL channel pairs and constructing the `NodeAddArgs` table.
Therefore the `DriverBase2` class provides a set of helper functions to make
adding child nodes easier. (To see these helpers, check out the
[`driver_base2.h`][driver-base-add-child] file.)

There are two types of nodes a DFv2 driver can add: **unowned** and **owned**.
The main difference between an unowned node and an owned node is whether they
participate in the [driver match process][driver-matching] or not.

The driver framework tries to find a driver that matches the properties of
unowned nodes so it can bind a driver to the node. Once a driver is matched and
bound to a node, the bound driver becomes the owner of the node.
On the other hand, owned nodes do not participate in matching since the driver
that created the node is already the owner.

#### DriverBase2 helper functions

The client to the node that your driver is currently bound to is stored in the
`DriverBase2` object. This allows the driver to use the `DriverBase2` class's
`AddChild()` and `AddOwnedChild()` functions to add a child node to this node.

However, to use these `DriverBase2` helper functions, the node must not have been
moved out of the driver. If the node is moved out or your target node is not the
node that the driver is currently bound to (ie. for a grand-child node),
you need to use the namespace methods available in the
[`add_child.h`][fdf-add-child] file instead. These methods are the same as the
`DriverBase2` helper functions except they can be used to add a child to a node
beyond the reach of the `DriverBase2` object, by providing the correct parent
node client as a target.

Lastly, these helper functions take care of logging errors if they happen,
so no logging is needed by the driver.

#### Create an unowned node

To create an unowned node, a driver can use the `DriverBase2::AddChild()` helper
functions. These functions allow setting the
properties on an unowned node, which the driver framework uses to find a matching
driver. The return result of both is a client end to the `NodeController` protocol,
which can either be kept by the driver or discarded safely.

The example code below creates an unowned node under the driver's bound node:

```cpp
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/drivers/simple/dfv2/simple_driver.cc" adjust_indentation="auto" region_tag="add_child" %}
```
(Source: [`simple_driver.cc`][simple-example])

#### Create an owned node

To create an owned node, a driver can use the `DriverBase2::AddOwnedChild()` helper
functions. These functions do not provide a properties argument since an owned
node does not participate in driver matching. The return result of both is an
`OwnedChildNode` object that contains a client end to the `NodeController` (which
is safe to discard) and a client end to the `Node` protocol, which is
**not safe to discard**. The driver must hold on to the `Node` client for as long as
it wants the owned node to stay around. Dropping this client will cause the driver
framework to remove the node.

The example code below creates an owned node:

```cpp
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/drivers/simple/dfv2/simple_driver.cc" adjust_indentation="auto" region_tag="add_owned_child" %}
```

(Source: [`simple_driver.cc`][simple-example])

### Clean up the driver {:#clean-up-the-driver}

If a DFv2 driver needs to perform teardowns before it is stopped (for example,
stopping threads), then you need to override and implement the
`DriverBase2`'s `Stop()` method (for asynchronous cleanup) or perform synchronous
cleanup in the destructor.

The `Stop()` function is called before the driver's `fdf` dispatchers are
shut down and the driver is deallocated. Therefore, the driver needs to
implement `Stop()` if it needs to perform certain operations before the
driver's dispatchers shut down, for example:

```cpp
void SimpleDriver::Stop(fdf::StopCompleter completer) {
 // Teardown threads
  fdf::info("Stopping SimpleDriver");
  completer(zx::ok());
}
```

### Add a compat device server {:#add-a-compat-device-server}

If your DFv2 driver has descendant DFv1 drivers that haven't yet migrated to
DFv2, you need to use the compatibility shim to enable your DFv2 driver to talk
to other DFv1 drivers in the system. For more details, see the
[Set up the compat device server in a DFv2 driver][set-up-compat-device-server]
guide.

<!-- Reference links -->

[skeleton-driver]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/skeleton/
[simple-driver]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/simple/
[driver-base]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/component/cpp/driver_base2.h
[skeleton-driver-h]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/skeleton/skeleton_driver.h
[skeleton-driver-cc]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/skeleton/skeleton_driver.cc
[build-gn]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/skeleton/BUILD.gn
[skeleton-driver-bind]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/skeleton/meta/skeleton_driver.bind
[node-properties]: /docs/development/drivers/tutorials/bind-rules-tutorial.md#looking_up_node_properties
[skeleton-driver-cml]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/skeleton/meta/skeleton_driver.cml
[component-info-json]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/skeleton/meta/component-info.json
[structured-logger-h]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/logging/cpp/structured_logger.h
[topology-fidl]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.driver.framework/topology.fidl
[node-adds-args]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/component/cpp/node_add_args.h
[bind-rules-tutorial]: /docs/development/drivers/tutorials/bind-rules-tutorial.md
[set-up-compat-device-server]: /docs/development/drivers/migration/set-up-compat-device-server.md
[simple-example]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/drivers/simple/dfv2/simple_driver.cc
[driver-base-add-child]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/component/cpp/driver_base2.h;l=225-258
[fdf-add-child]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/node/cpp/add_child.h
[driver-matching]: /docs/concepts/drivers/driver_binding.md
