# Inspect Quickstart

This quickstart guides you through the basics of using
[Component Inspection][overview]. You will learn how to integrate Inspect into
your component using the language-specific libraries and review the data using
[`ffx inspect`][ffx-inspect].

For a more detailed walkthrough of Inspect concepts, see the
[Inspect codelab](codelab.md).

## Project setup

See below for the quick start guide in your language of choice:

* {C++}

  This section assumes you are writing an asynchronous component and that
  some part of your component (typically `main.cc`) looks like this:

  ```cpp
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  auto context_ = sys::ComponentContext::CreateAndServeOutgoingDirectory();
  // ...
  loop.Run();
  ```

  This sets up an async loop, creates a `ComponentContext` wrapping handles
  provided by the runtime, and then runs that loop following some other
  initialization work.

  **Add the Inspect library dependencies to your `BUILD.gn` file:**

  ```gn
  {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/cpp/BUILD.gn" region_tag="inspect_libs" adjust_indentation="auto" %}
  ```

  **Add the following includes:**

  ```cpp
  {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/cpp/example_server_app.h" region_tag="inspect_imports" adjust_indentation="auto" %}
  ```

  **Add the following code to initialize Inspect:**

  ```cpp
  {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/cpp/example_server_app.cc" region_tag="initialization" adjust_indentation="auto" %}
  ```

  You are now using Inspect! Create properties in the Inspect tree by attaching
  them to the root node:

  ```cpp
  {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/cpp/example_server_app.cc" region_tag="properties" adjust_indentation="auto" %}
  ```

  Note: For a complete working example, see
  [//examples/diagnostics/inspect/cpp](/examples/diagnostics/inspect/cpp).

  See [Supported Data Types](#supported-types) for a full list of data
  types you can try.

  #### Health checks

  The health check subsystem provides a standardized inspection metric for
  component health. You can use the health node to report the overall status
  of your component:

  ```cpp
  {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/cpp/example_server_app.cc" region_tag="health_check" adjust_indentation="auto" %}
  ```

  Note: For more details on health metrics, see [Health check][health-check].

  #### Testing

  To test your inspect code, you can use
  [//sdklib/inspect/testing/cpp/inspect.h](/sdk/lib/inspect/testing):

  ```cpp
  {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/cpp/example_unittests.cc" region_tag="test_imports" adjust_indentation="auto" %}
  ```

  This library includes a full set of matchers to validate the contents of the
  Inspect tree.

  ```cpp
  {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/cpp/example_unittests.cc" region_tag="inspect_test" adjust_indentation="auto" %}
  ```

* {Rust}

  This section assumes you are writing an asynchronous component and that some
  part of your component (typically `main.rs`) looks similar to this:

  ```rust
  async fn main() -> Result<(), Error> {
    // ...
    let mut service_fs = ServiceFs::new();
    // ...
    service_fs.take_and_serve_directory_handle().unwrap();
    service_fs.collect::<()>().await;
    Ok(())
  }
  ```

  **Add the Inspect library dependencies to your `BUILD.gn` file:**

  ```gn
  {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/rust/BUILD.gn" region_tag="inspect_libs" adjust_indentation="auto" %}
  ```

  **Add the following code to initialize Inspect:**

  ```rust
  {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/rust/src/echo_server.rs" region_tag="initialization" adjust_indentation="auto" %}
  ```

  You are now using Inspect! Create properties in the Inspect tree by attaching
  them to the root node:

  ```rust
  {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/rust/src/echo_server.rs" region_tag="properties" adjust_indentation="auto" %}
  ```

  Note: For a complete working example, see
  [//examples/diagnostics/inspect/rust](/examples/diagnostics/inspect/rust).

  See [Supported Data Types](#supported-types) for a full list of data
  types you can try.

  #### Health checks

  The health check subsystem provides a standardized inspection metric for
  component health. You can use the health node to report the overall status
  of your component:

  ```rust
  {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/rust/src/echo_server.rs" region_tag="health_check" adjust_indentation="auto" %}
  ```

  Note: For more details on health metrics, see [Health check][health-check].

  #### Testing

  To test your Inspect code, you can use `assert_data_tree` to validate the
  contents of the Inspect tree:

  ```rust
  {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/rust/src/echo_server.rs" region_tag="inspect_test" adjust_indentation="auto" %}
  ```

  Note: To learn more about the Rust library, see the complete reference for
  [`fuchsia_inspect`](https://fuchsia-docs.firebaseapp.com/rust/fuchsia_inspect/index.html).


## Inspect Libraries {#inspect-libraries}

<!-- TODO(https://fxbug.dev/42165233): Replace code snippets with examples -->

Now that you have a `root_node` you may start building your
hierarchy. This section describes some important concepts and patterns
to help you get started.

* A Node may have any number of key/value pairs called **Properties**.
* The key for a Value is always a UTF-8 string, the value may be one of the
  [supported types](#supported-types) below.
* A Node may have any number of children, which are also Nodes.

* {C++}

  The code above gives you access to a single node named
  "root". `hello_world_property` is a Property that contains a string value
  (aptly called a **StringProperty**).

  * Values and Nodes are created under a parent Node.

  Class `Node` has creator methods for every type of
  supported value. `hello_world_property` was created using
  `CreateStringProperty`. You could create a child under the root node
  by calling `root_node.CreateChild("child name")`. Note that names must
  always be UTF-8 strings.

  * Values and Nodes have strict ownership semantics.

  `hello_world_property` owns the Property. When it is destroyed (goes
  out of scope) the underlying Property is deleted and no longer present
  in your component's Inspect output. This is true for child Nodes as well.

  If you are creating a value that doesn't need to be modified, use a
  [`ValueList`](/zircon/system/ulib/inspect/include/lib/inspect/cpp/vmo/types.h)
  to keep them alive until they are no longer needed.

  * Inspection is best-effort.

  Due to space limitations, the Inspect library may be unable to satisfy
  a `Create` request. This error is not surfaced to your code: you will
  receive a Node/Property object for which the methods are no-ops.

  * Pattern: Pass in child Nodes to child objects.

  It is useful to add an `inspect::Node` argument to the constructors
  for your own classes. The parent object, which should own its own
  `inspect::Node`, may then pass in the result of `CreateChild(...)`
  to its children when they are constructed:

  ```cpp
  class Child {
    public:
      Child(inspect::Node my_node) : my_node_(std::move(my_node)) {
        // Create a string that doesn't change, and emplace it in the ValueList
        my_node_.CreateString("version", "1.0", &values_);
        // Create metrics and properties on my_node_.
      }

    private:
      inspect::Node my_node_;
      inspect::StringProperty some_property_;
      inspect::ValueList values_;
      // ... more properties and metrics
  };

  class Parent {
    public:
      // ...

      void AddChild() {
        // Note: inspect::UniqueName returns a globally unique name with the specified prefix.
        children_.emplace_back(my_node_.CreateChild(inspect::UniqueName("child-")));
      }

    private:
      std::vector<Child> children_;
      inspect::Node my_node_;
  };
  ```

* {Rust}

  The Rust library provides two ways of managing nodes and properties: creation
  and recording.

  With the `create_*` methods, the ownership of the property or node object belongs
  to the caller. When the returned object is dropped, the property is removed.
  For example:

  ```rust
  {
      let property = root.create_int("name", 1);
  }
  ```

  In this example, `property` went out of scope so a drop on the property is
  called. Readers won't see this property.

  With the `record_*` methods, the lifetime of the property is tied to the parent
  node. When the node is deleted, the recorded property is deleted.

  ```rust
  {
      let node = root.create_child("name");
      {
        node.record_uint(2); // no return
      }
      // The uint property will still be visible to readers.
  }
  ```

  In this example, the uint property associated with `name` is visible to readers
  until the parent `node` goes out of scope.

### Dynamic values

This section describes support in the Inspect libraries for nodes that are
inflated lazily at read-time. The methods accept a callback function instead of
a value. The callback function is invoked when the property value is read.

* {C++}

  The C++ library has two property creators for dynamic values:
  `CreateLazyNode` and `CreateLazyValues`.

  Both of these methods take a callback returning a promise for an
  `inspect::Inspector`, the only difference is how the dynamic values are
  stored in the tree.

  `root->CreateLazyNode(name, callback)` creates a child node of
  `root` with the given `name`. The `callback` returns a promise for an
  `inspect::Inspector` whose root node is spliced into the parent hierarchy
  when read. The example below shows that a child called "lazy" exists with
  the string property "version" and has an additional child that is called
  "lazy."

  `root->CreateLazyValues(name, callback)` works like `root->CreateLazyNode(name,
  callback)`, except all properties and child nodes on the promised root node are
  added directly as values
  to the original `root`. In the second output of this example, the internal
  lazy nodes do not appear and their values are flattened into properties on
  `root`.

  ```cpp
  root->CreateLazy{Node,Values}("lazy", [] {
    Inspector a;
    a.GetRoot().CreateString("version", "1.0", &a);
    a.GetRoot().CreateLazy{Node,Values}("lazy", [] {
      Inspector b;
      b.GetRoot().RecordInt("value", 10);
      return fpromise::make_ok_promise(std::move(b));
    }, &a);

    return fpromise::make_ok_promise(std::move(a));
  });
  ```

  Output (CreateLazyNode):

  ```
  root:
    lazy:
      version = "1.0"
      lazy:
        value = 10
  ```

  Output (CreateLazyValues):

  ```
  root:
    value = 10
    version = "1.0"
  ```

  Warning: It is the developer's responsibility to ensure that names
  flattened from multiple lazy value nodes do not conflict. If they do,
  output behavior is undefined.

  The return value of `CreateLazy{Node,Values}` is a `LazyNode` that owns
  the passed callback.  The callback is never called once the `LazyNode` is
  destroyed. If you destroy a `LazyNode` concurrently with the execution of
  a callback, the destroy operation is blocked until the callback returns
  its promise.

  If you want to dynamically expose properties on `this`, you may simply
  write the following:

  ```cpp
  class Employee {
    public:
      Employee(inspect::Node node) : node_(std::move(node)) {
        calls_ = node_.CreateInt("calls", 0);

        // Create a lazy node that populates values on its parent
        // dynamically.
        // Note: The callback will never be called after the LazyNode is
        // destroyed, so it is safe to capture "this."
        lazy_ = node_.CreateLazyValues("lazy", [this] {
          // Create a new Inspector and put any data in it you want.
          inspect::Inspector inspector;

          // Keep track of the number of times this callback is executed.
          // This is safe because the callback is executed without locking
          // any state in the parent node.
          calls_.Add(1);

          // ERROR: You cannot modify the LazyNode from the callback. Doing
          // so may deadlock!
          // lazy_ = ...

          // The value is set to the result of calling a method on "this".
          inspector.GetRoot().RecordInt("performance_score",
                                        this->CalculatePerformance());

          // Callbacks return a fpromise::promise<Inspector>, so return a result
          // promise containing the value we created.
          // You can alternatively return a promise that is completed by
          // some asynchronous task.
          return fpromise::make_ok_promise(std::move(inspector));
        });
      }

    private:
      inspect::Node node_;
      inspect::IntProperty calls_;
      inspect::LazyNode lazy_;
  };
  ```

* {Rust}

  Refer to [C++ Dynamic Value Support](#c++), as similar concepts apply in Rust.

  Example:

  ```rust
  root.create_lazy_{child,values}("lazy", [] {
      async move {
          let inspector = Inspector::default();
          inspector.root().record_string("version", "1.0");
          inspector.root().record_lazy_{node,values}("lazy", || {
              let inspector = Inspector::default();
              inspector.root().record_int("value", 10);
              // `_value`'s drop is called when the function returns, so it will be removed.
              // For these situations `record_` is provided.
              let _value = inspector.root().create_int("gone", 2);
              Ok(inspector)
          });
          Ok(inspector)
      }
      .boxed()
  });

  Output (create_lazy_node):
  root:
    lazy:
      version = "1.0"
      lazy:
        value = 10

  Output (create_lazy_values):
  root:
    value = 10
    version = "1.0"
  ```

### String references {#string-reference}

* {C++}

  The names of nodes and properties automatically use string interning.

  ```cpp
  using inspect::Inspector;

  Inspector inspector;

  for (int i = 0; i < 100; i++) {
    inspector.GetRoot().CreateChild("child", &inspector);
  }
  ```

  Will generate only one copy of `"child"` which is referenced 100 times.

* {Rust}

  String names are automatically de-duplicated in Rust Inspect. For example,

  ```rust
  use fuchsia_inspect::Inspector;

  let inspector = Inspector::default();
  for _ in 0..100 {
    inspector.root().record_child("child");
  }
  ```

  Will generate only 1 copy of `"child"` which is referenced 100 times.

  This saves 16 bytes for each child node, and has a cost of 32 bytes
  for the shared data. The net result is a savings of 1568 bytes.

### Event logging and timestamps {#event-logging-and-timestamps}

While Inspect properties typically represent instantaneous component state,
components often need to record historical events (such as connection attempts,
state transitions, or errors).

To record rolling event logs and timestamps effectively:

* Follow the [`@time` naming convention](#timestamp-conventions) so timestamps
  are clearly identified by developers and can be parsed by Fuchsia Snapshot
  Viewer (FSV).

* Use [bounded list nodes](#bounded-list-nodes) to maintain a fixed-capacity
  FIFO buffer of events without unbounded memory growth.

#### Timestamp property conventions {#timestamp-conventions}

Note: All of these conventions are weakly held. Inspect is largely free-form
data, and you can choose to depart from these conventions at any time.

Inspect does not have a dedicated timestamp primitive type. Instead, timestamps
are recorded as 64-bit integer properties (`IntProperty` or `UintProperty`),
typically representing nanoseconds or seconds. Fuchsia Snapshot Viewer (FSV)
makes no assumptions about the timeline; users must know from context how to
interpret the timestamp.

FSV parses property keys named `@time` or ending in `@time`
(`<prefix>@time`) to render human-readable dates and calculate elapsed
durations:

* **Event timestamp (`@time`)**: Use the exact property key `@time` for the
  primary timestamp of an event or state change.

* **Interval timestamps (`start@time`, `end@time`)**: Use the `@time` suffix on
  property keys (such as `start@time`, `end@time`, `created@time`, or
  `last_seen@time`) when tracking timestamps and durations across time ranges.

Note: Recording timestamps on the boot timeline is specific to
Diagnostics-provided libraries (such as the `inspect_log!` macro in
`fuchsia-inspect-contrib`). The boot timeline (`zx::BootInstant` in Rust,
`zx::clock::get_boot()` in C++) continues incrementing while the device is in
low-power or suspend states.

#### Bounded list nodes {#bounded-list-nodes}

A bounded list node maintains a circular FIFO buffer of child nodes under a
parent node. Each new event is added as a child node named with an
auto-incrementing index (`"0"`, `"1"`, `"2"`, ...). When the list reaches its
maximum capacity, creating a new entry automatically evicts the oldest entry.

* {Rust}

  In Rust, use `BoundedListNode` from the
  [`fuchsia-inspect-contrib`][fuchsia-inspect-contrib] crate. Combine it with
  the `inspect_log!` macro to record timestamped events with automatic `@time`
  property injection:

  ```rust
  use fuchsia_inspect_contrib::inspect_log;
  use fuchsia_inspect_contrib::nodes::BoundedListNode;

  // Create a bounded list with a capacity of 10 entries under "events".
  let mut events = BoundedListNode::new(root.create_child("events"), 10);

  // Log an event using key-value syntax. An "@time" property is recorded
  // automatically.
  inspect_log!(events, state: "connected", address: 42u64);

  // Log an event using block syntax with multiple fields.
  inspect_log!(events, {
      state: "disconnected",
      reason: "timeout",
      retry_count: 3u32,
  });

  // Log optional fields and nested structures.
  let peer_id: Option<u64> = Some(1234);
  inspect_log!(events, {
      event: "peer_discovered",
      peer_id?: peer_id,
      details: {
          rssi: -45i16,
          channel: 6u8,
      },
  });
  ```

  To record timestamps manually on any node (such as start and end times), use
  the `NodeTimeExt` extension trait:

  ```rust
  use fuchsia_inspect_contrib::nodes::{BootTimeline, NodeTimeExt};

  // Record an "@time" property with the current boot timestamp.
  NodeTimeExt::<BootTimeline>::record_time(&node, "@time");

  // Record interval timestamps on a node.
  node.record_int("start@time", start_instant.into_nanos());
  node.record_int("end@time", end_instant.into_nanos());
  ```

  Note: `BoundedListNode` methods take `&mut self`. If your bounded list is
  shared across threads or asynchronous tasks, wrap it in a `Mutex`.

* {C++}

  In C++, use `inspect::BoundedListNode` by including
  `<lib/inspect/cpp/bounded_list_node.h>`:

  ```cpp
  #include <lib/inspect/cpp/bounded_list_node.h>
  #include <lib/zx/clock.h>

  // Create a bounded list with a capacity of 10 entries under "events".
  inspect::BoundedListNode events(root.CreateChild("events"), 10);

  // Record a timestamped entry using CreateEntry.
  events.CreateEntry([](inspect::Node& entry) {
    entry.RecordInt("@time", zx::clock::get_boot().get());
    entry.RecordString("state", "connected");
    entry.RecordUint("address", 42);
  });

  // Record interval timestamps when measuring operations.
  events.CreateEntry([&](inspect::Node& entry) {
    entry.RecordInt("start@time", start_time.get());
    entry.RecordInt("end@time", zx::clock::get_boot().get());
    entry.RecordString("status", "success");
  });
  ```

  Note: In C++, `inspect::BoundedListNode` is internally synchronized with a
  mutex and is safe for concurrent access across multiple threads.

#### Output hierarchy

When inspected using [`ffx inspect`][ffx-inspect], the resulting bounded event
log appears in the hierarchy as indexed child nodes:

```none {:.devsite-disable-click-to-copy}
root:
  events:
    "0":
      "@time" = 123456789012
      address = 42
      state = "connected"
    "1":
      "@time" = 123457890123
      reason = "timeout"
      retry_count = 3
      state = "disconnected"
```

## Viewing Inspect Data {#view-inspect-data}

You can use the [`ffx inspect`][ffx-inspect] command to view the Inspect data
you exported from your component.

This section assumes you have SSH access to your running Fuchsia system and
that you started running your component. We will use the name
`my_component.cm` as a placeholder for the name of your component's manifest.

Note: Your component's full URL may be
`fuchsia-pkg://fuchsia.com/my_component#meta/my_component.cm`, but
you only need the manifest name to find it. This is separate from
your component's *[moniker]*, which is the location it is running in
the component hierarchy. You may refer to your component by either
moniker or manifest name.

### Read your Inspect data

The command below prints the inspect hierarchies of all components
running in the system:

```posix-terminal
ffx inspect show
```

Using the output from `ffx inspect list`, you can specify a
single component (for example, `core/network/netstack`) as input to
`ffx inspect show`:

```posix-terminal
ffx inspect show core/network/netstack
```

You may specify multiple components (for example, `core/font_provider`
and `core/my_component`):

```posix-terminal
ffx inspect show core/font_provider core/my_component
```

You can also specify a node and property value. To see the
list of all possible [selectors], use `ffx inspect selectors`:

```posix-terminal
ffx inspect selectors core/my_component
```

You may then specify a selector pointing to a node as input to
`ffx inspect show`:

```posix-terminal
ffx inspect show core/my_component:root/my_node
```

This results result in an output that includes that node and all its children
and nested properties:

```none {:.devsite-disable-click-to-copy}
core/my_component:
  metadata:
    name = root
    component_url = fuchsia-pkg://fuchsia.com/my_package#meta/my_component.cm
    timestamp = 1234567890
  payload:
    root:
      my_node:
        hello = "goodbye"
        world = 2
        a_child:
          test = 4.2
```

You may also specify a selector pointing to a property as input to
`ffx inspect show`:

```posix-terminal
ffx inspect show core/my_component:root/my_node:hello
```

This results result in an output like:

```none {:.devsite-disable-click-to-copy}
core/my_component:
  metadata:
    name = root
    component_url = fuchsia-pkg://fuchsia.com/my_package#meta/my_component.cm
    timestamp = 1234567890
  payload:
    root:
      my_node:
        hello = "goodbye"
```

If you don't know the moniker for your component, you can pass a string that you
think is related to your component manifest, url, moniker, etc... The tool then
does a fuzzy match against all components. If more than one match is found,
it'll ask for disambiguation, otherwise it'll return the output you expect.

For example, the following may return more than one match:

```posix-terminal
ffx inspect show network
```

This returns:

```none {:.devsite-disable-click-to-copy}
Fuzzy matching failed due to too many matches, please re-try with one of these:
bootstrap/boot-drivers:PCI0.bus.00_04_0.00_04_0.virtio-net
core/network
core/network-tun
core/network/dhcpd
core/network/dhcpv6-client
core/network/dns-resolver
core/network/http-client
core/network/netcfg
core/network/netcfg/netcfg-config
core/network/netstack
core/network/netstack/dhcp-client
core/network/reachability
```

This example shows an invocation that results in a single match:

```posix-terminal
ffx inspect show feedback
```

This example prints out the Inspect data of `core/feedback`:

#### Print your Inspect in a testing context

Use the JSON pretty printer to obtain a full listing. For example:

```rust
use diagnostics_assertions::JsonGetter;
...
    #[fuchsia::test]
    fn my_test() {
        let inspect = fuchsia_inspect::component::inspector();
        ...
        print!("{}", inspect.get_pretty_json());
    }
```

## Supported Data Types {#supported-types}

 Type | Description | Notes
  -----|-------------|-------
    IntProperty | A metric containing a signed 64-bit integer. | All Languages
    UIntProperty | A metric containing an unsigned 64-bit integer. | Not supported in Dart
    DoubleProperty | A metric containing a double floating-point number. | All Languages
    BoolProperty | A metric containing a double floating-point number. | All Languages
    {Int,Double,Uint}Array | An array of metric types, includes typed wrappers for various histograms. | Same language support as base metric type
    StringArray | An array of strings. Represented as a [StringReference](#string-reference). | Not supported in Dart.
    StringProperty | A property with a UTF-8 string value. | All Languages
    ByteVectorProperty | A property with an arbitrary byte value. | All Languages
    Node | A node under which metrics, properties, and more nodes may be nested. | All Languages
    LazyNode | Instantiates a complete tree of Nodes dynamically. | C++, Rust

<!-- Reference links -->

[ffx-inspect]: https://fuchsia.dev/reference/tools/sdk/ffx.md#inspect
[health-check]: /docs/development/diagnostics/inspect/health.md
[overview]: /docs/development/diagnostics/inspect/README.md
[moniker]: /docs/reference/components/moniker.md
[selectors]: /docs/reference/diagnostics/selectors.md
[fuchsia-inspect-contrib]: https://fuchsia-docs.firebaseapp.com/rust/fuchsia_inspect_contrib/index.html
