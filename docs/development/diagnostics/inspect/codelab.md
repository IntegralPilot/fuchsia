# Codelab: Using Inspect

In this codelab, you will learn to use the Inspect (Rust and C++) to publish
diagnostic information from your programs, and use Inspect information to debug
your programs.

In this codelab, you're going to modify programs to output Inspect data.
You will learn:

* How to include the Inspect libraries.

* How to initialize Inspect in your components.

* How to solve real bugs by writing and reading Inspect data.

* How to read Inspect data to verify that your program is doing what you want.

## What is Inspect?

Inspect allows Fuchsia Components to expose structured, hierarchical
information about their current state.

Learn more about Inspect in the
[Fuchsia Component Inspection](/docs/development/diagnostics/inspect/README.md)
documentation.

### What benefits does Inspect provide?

Component Inspection supports many use cases, including:

* Debugging

  View Inspect data from your running component to identify
  problems. For example, you can learn if your component is currently
  connected to a dependency.

* Monitoring system health

  Inspect data provides insight into overall system state. For example,
  you can learn why your system is not connected to the internet.

* Gathering usage or performance statistics

  You can read Inspect data from multiple components at the same time
  to understand system performance. For example, you can see the list
  of incoming connections to your component along with your component's
  memory usage.

### What kind of information can I store in Inspect?

You determine the structure and content of the data you expose in
Inspect. Some examples include:

* The number of open WiFi connections.
* The number of requests that the program has served.
* The number of errors that a parser has encountered.
* The contents of a data structure.

### API Reference

* {C++}

   [Inspect library documentation](/sdk/lib/inspect/component/cpp/component.h)

* {Rust}

   [fuchsia-inspect crate documentation](https://fuchsia-docs.firebaseapp.com/rust/fuchsia_inspect/index.html)

## What you’ll need

* Basic knowledge of Rust, or C++.
* Access to a Fuchsia source tree you can execute build commands in.

## Source code

The code is available at:

* [//examples/diagnostics/inspect/codelab/cpp][inspect-cpp-codelab].
* [//examples/diagnostics/inspect/codelab/rust][inspect-rust-codelab].

This codelab is organized into several parts, each with their own
subdirectory. The starting point for the codelab is part 1,
and the code for each part contains the solution for the previous parts.

* [C++ Part 1][cpp-part1]
* [Rust Part 1][rust-part1]

Note: For Rust, there is also an ergonomic library with a higher level API:
[fuchsia-inspect-derive][fuchsia-inspect-derive]. However, it's recommended to
understand the concepts explained in this codelab before using that other
library.

When working on this codelab, you may continue adding your solutions to
"part\_1", or you may skip around by building on the existing solutions.

## Prerequisites

Set up your development environment.

This codelab assumes you have completed [Getting Started](/docs/get-started/README.md) and have:

1. A checked out and built Fuchsia tree.
2. A device or emulator (`ffx emu`) that runs Fuchsia.
3. A workstation to serve components (`fx serve`) to your Fuchsia device or emulator.
   - Refer to the [Components Developer Guides](/docs/development/components/README.md)
     to learn about running components.

To build and run the examples in this codelab, add the following arguments
to your `fx set` invocation:

* {C++}

   Note: Replace `core.x64` with your preferred product and board configuration.

   ```posix-terminal
   fx set core.x64 \
   --with //examples/diagnostics/inspect/codelab/cpp \
   --with-test //examples/diagnostics/inspect/codelab/cpp:hermetic_tests
   ```

* {Rust}

   Note: Replace `core.x64` with your preferred product and board configuration.

   ```posix-terminal
   fx set core.x64 \
   --with //examples/diagnostics/inspect/codelab/rust
   ```


## Part 1: A buggy component

There is a component that serves a protocol called [Reverser][fidl-reverser]:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/fidl/reverser.test.fidl" region_tag="reverser_fidl" adjust_indentation="auto" %}
```

This protocol has a single method, called `Reverse`, that simply reverses
any string passed to it. An implementation of the protocol is provided,
but it has a critical bug. The bug makes clients who attempt to call
the `Reverse` method see that their call hangs indefinitely. It is up to
you to fix this bug.

### Run the component

There is a client application that will launch the Reverser component and send the rest of its
command line arguments as strings to Reverse:


1. See usage

   Depending on the part of the codelab you wish to run, you'd launch the
   `client_i` component, where `i` is a number in range \[1, 5\]. For example, to
   launch the client talking to the reverser from part 2 of the codelab:

   * {C++}

      ```posix-terminal
      ffx component run /core/ffx-laboratory:client_part_2 fuchsia-pkg://fuchsia.com/inspect_cpp_codelab#meta/client_part_2.cm
      ```

   * {Rust}

      ```posix-terminal
      ffx component run /core/ffx-laboratory:client_part_2 fuchsia-pkg://fuchsia.com/inspect_rust_codelab#meta/client_part_2.cm
      ```


2. Run part 1 code, and reverse the string "Hello"

   To specify just the single string "Hello" modify the `program.args` section of
   the [common.shard.cml][cpp-common-cml], build and run the following:

   * {C++}

      ```posix-terminal
      ffx component run /core/ffx-laboratory:client_part_1 fuchsia-pkg://fuchsia.com/inspect_cpp_codelab#meta/client_part_1.cm
      ```

      To see the command output take a look at the logs:

      ```posix-terminal
      ffx log --tag inspect_cpp_codelab
      ```

      We see in the logs that the component got the "Hello" as input, but we
      don't see the correct reversed output.

   * {Rust}

      ```posix-terminal
      ffx component run /core/ffx-laboratory:client_part_1 fuchsia-pkg://fuchsia.com/inspect_rust_codelab#meta/client_part_1.cm
      ```

      To see the command output take a look at the logs:

      ```posix-terminal
      ffx log --tag inspect_rust_codelab
      ```

      We see in the logs that the component got the "Hello" as input, but we
      don't see the correct reversed output.

   As you can see in the log the reverser doesn't work properly.

3. Try running the client with more arguments:

   Add the string "World" to the `program.args` section of the
   [common.shard.cml][cpp-common-cml]:

   ```json5
   {
       program: {
           args: [
               "Hello",
               "World",
           ],
       },
   }
   ```

   Build and run the following:

   * {C++}

      ```posix-terminal
      ffx component run --recreate /core/ffx-laboratory:client_part_1 fuchsia-pkg://fuchsia.com/inspect_cpp_codelab#meta/client_part_1.cm
      ```

   * {Rust}

      ```posix-terminal
      ffx component run --recreate /core/ffx-laboratory:client_part_1 fuchsia-pkg://fuchsia.com/inspect_rust_codelab#meta/client_part_1.cm
      ```

   We can see that the component printed the first input, but we don't see the
   expected output and also no second input.

You are now ready to look through the code to troubleshoot the issue.

### Look through the code

Now that you can reproduce the problem, take a look at what the client is doing:

* {C++}

   In the [client main][cpp-client-main]:

   ```cpp
   {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/client/main.cc" region_tag="reverse_loop" adjust_indentation="auto" %}
   ```

* {Rust}

   In the [client main][rust-client-main]:

   ```rust
   {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/client/src/main.rs" region_tag="reverse_loop" adjust_indentation="auto" %}
   ```


In this code snippet, the client calls the `Reverse` method but never
seems to get a response. There doesn't seem to be an error message
or output, the component just hangs.

Take a look at the server code for this part of the
codelab. There is a lot of standard component setup:

* {C++}

   In the [part 1 main][cpp-part1-main]:

   - Logging initialization

     ```cpp
     {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_1/main.cc" region_tag="init_logger" adjust_indentation="auto" %}
     ```

   - Creating an asynchronous executor

     ```cpp
     {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_1/main.cc" region_tag="async_executor" adjust_indentation="auto" %}
     ```

   - Serving a public service

     ```cpp
     {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_1/main.cc" region_tag="serve_outgoing" adjust_indentation="auto" %}
     ```

* {Rust}

   In the [part 1 main][rust-part1-main]:

   - Logging initialization

     ```rust
     {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_1/src/main.rs" region_tag="init_logger" adjust_indentation="auto" %}
     ```

   - ServiceFs initialization

     ```rust
     {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_1/src/main.rs" region_tag="servicefs_init" adjust_indentation="auto" %}
     ```

   - ServiceFs collection

     ```rust
     {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_1/src/main.rs" region_tag="servicefs_collect" adjust_indentation="auto" %}
     ```

   - Serving a public service

     ```rust
     {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_1/src/main.rs" region_tag="serve_service" adjust_indentation="auto" %}
     ```

See what the reverser definition is:

* {C++}

   In [reverser.h][cpp-part1-reverser-h]:

   ```cpp
   {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_1/reverser.h" region_tag="reverser_h" adjust_indentation="auto" %}
   ```

   This class implements the `Reverser` protocol. A helper method called
   `CreateDefaultHandler` constructs an `InterfaceRequestHandler` that
   creates new `Reverser`s for incoming requests.

* {Rust}

   In [reverser.rs][rust-part1-reverser]:

   ```rust
   {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_1/src/reverser.rs" region_tag="reverser_def" adjust_indentation="auto" %}
   ```

   This struct serves the `Reverser` protocol. The `ReverserServerFactory` (will make more sense
   later) constructs a `ReverserServer` when a new connection to `Reverser` is established.


### Add Inspect

Now that you know the code structure, you can start to instrument the
code with Inspect to find the problem.

Note: [Inspect](/docs/development/diagnostics/inspect/README.md) is a powerful instrumentation feature for
Fuchsia Components. You can expose structured information about the component's state to diagnose
the problem.

You may have previously debugged programs by printing or logging. While
this is often effective, asynchronous Components that run persistently
often output numerous logs about their internal state over time. This
codelab shows how Inspect provides snapshots of your component's current
state without needing to dig through logs.

1. Include Inspect dependencies:

   * {C++}

      In [BUILD.gn][cpp-part1-build] in `public_deps` under `source_set("lib")`:

      ```cpp
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_2/BUILD.gn" region_tag="part_1_solution_build_dep" adjust_indentation="auto" %}
      ```

   * {Rust}

      In [BUILD.gn][rust-part1-build] in `deps` under `rustc_binary("bin")`:

      ```rust
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_2/BUILD.gn" region_tag="part_1_solution_build_dep" adjust_indentation="auto" %}
      ```

2. Initialize Inspect:

   * {C++}

      In [main.cc][cpp-part1-main]:

      ```cpp
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_2/main.cc" region_tag="part_1_include_inspect" adjust_indentation="auto" %}
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_2/main.cc" region_tag="part_1_init_inspect" adjust_indentation="auto" %}
      ```


   * {Rust}

      In [main.rs][rust-part1-main]:

      ```rust
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_2/src/main.rs" region_tag="part_1_use_inspect" adjust_indentation="auto" %}
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_2/src/main.rs" region_tag="part_1_serve_inspect" adjust_indentation="auto" %}
      ```

   You are now using Inspect.

3. Add a simple "version" property to show which version is running:

   * {C++}

      ```cpp
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_2/main.cc" region_tag="part_1_write_version" adjust_indentation="auto" %}
      ```

      This snippet does the following:

      1. Obtain the "root" node of the Inspect hierarchy.

         The Inspect hierarchy for your component consists of a tree of Nodes,
         each of which contains any number of properties.

      2. Create a new property as part of `RecordString`.

         This adds a new `StringProperty` on the root. This `StringProperty`
         is called "version", and its value is "part2". We're going to set our
         property to "part1".

      3. Record the new property in the root node.

         The usual way of creating properties is through `Create*` methods on nodes. The lifetime of
         a property created with these methods is tied to the object returned and destroying the
         object causes the property to disappear. The library provides convenience methods `Record*`
         that perform creation of a property and tie the property lifetime to the node on which the
         method was called. As a result, the new property lives as long as the node itself (in this
         case, as long as the root node, so the entire execution of the component).

   * {Rust}

     ```rust
     {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_2/src/main.rs" region_tag="part_1_write_version" adjust_indentation="auto" %}
     ```

     This snippet does the following:

     1. Obtain the "root" node of the Inspect hierarchy.

        The Inspect hierarchy for your component consists of a tree of Nodes,
        each of which contains any number of properties.

     2. Create a new property using `record_string`.

        This adds a new `StringProperty` on the root. This `StringProperty`
        is called "version", and its value is "part2". We're going to set our
        property to "part1".

     3. It records it in the root node.

        The usual way of creating properties is through `create_*` methods on nodes. The lifetime of
        a property created with these methods is tied to the object returned and destroying the
        object causes the property to disappear. The library provides convenience methods `record_*`
        that perform creation of a property and tie the property lifetime to the node on which the
        method was called. As a result, the new property lives as long as the node itself (in this
        case, as long as the root node, so the entire execution of the component).


### Reading Inspect data

Now that you have added Inspect to your component, you can read what it says:

1. Rebuild and update the target system

   ```posix-terminal
   fx build && fx ota
   ```

2. Run the client:

   * {C++}

      ```posix-terminal
      ffx component run --recreate /core/ffx-laboratory:client_part_1 fuchsia-pkg://fuchsia.com/inspect_cpp_codelab#meta/client_part_1.cm

      ffx log --tag inspect_cpp_codelab
      ```

   * {Rust}

      ```posix-terminal
      ffx component run --recreate /core/ffx-laboratory:client_part_1 fuchsia-pkg://fuchsia.com/inspect_rust_codelab#meta/client_part_1.cm

      ffx log --tag inspect_rust_codelab
      ```

3. Use `ffx inspect` to view your output:

   ```posix-terminal
   ffx inspect show
   ```

   This dumps all of the Inspect data for the entire system, which may be a lot of data.

4. Since `ffx inspect` supports glob matching, run:

   * {C++}

      ```posix-terminal
      ffx inspect show 'core/ffx-laboratory:client_part_1/reverser'
      # or `ffx inspect show inspect_cpp_codelab`
      metadata:
        filename = fuchsia.inspect.Tree
        component_url = fuchsia-pkg://fuchsia.com/inspect_cpp_codelab#meta/part_1.cm
        timestamp = 4728864898476
      payload:
        root:
          version = part1
      ```

   * {Rust}

      ```posix-terminal
      ffx inspect show 'core/ffx-laboratory:client_part_1/reverser'
      # or `ffx inspect show inspect_rust_codelab`
      metadata:
        filename = fuchsia.inspect.Tree
        component_url = fuchsia-pkg://fuchsia.com/inspect_rust_codelab#meta/part_1.cm
        timestamp = 4728864898476
      payload:
        root:
          version = part1
      ```

5. You can also view the output as JSON:

   * {C++}

      ```posix-terminal
      ffx --machine json-pretty inspect show 'core/ffx-laboratory:client_part_1/reverser'
      ```

      You should see an output like:

      ```json5 {:.devsite-disable-click-to-copy}
      [
        {
          "data_source": "Inspect",
          "metadata": {
            "errors": null,
            "filename": "fuchsia.inspect.Tree",
            "component_url": "fuchsia-pkg://fuchsia.com/inspect_pp_codelab#meta/part_1.cm",
            "timestamp": 5031116776282
          },
          "moniker": "core/ffx-laboratory\\:client_part_5/reverser",
          "payload": {
            "root": {
              "version": "part1",
          },
          "version": 1
        }
      ]
      ```

   * {Rust}

      ```posix-terminal
      ffx --machine json-pretty inspect show 'core/ffx-laboratory:client_part_1/reverser'
      ```

      You should see an output like:

      ```json5 {:.devsite-disable-click-to-copy}
      [
        {
          "data_source": "Inspect",
          "metadata": {
            "errors": null,
            "filename": "fuchsia.inspect.Tree",
            "component_url": "fuchsia-pkg://fuchsia.com/inspect_rust_codelab#meta/part_1.cm",
            "timestamp": 5031116776282
          },
          "moniker": "core/ffx-laboratory\\:client_part_5/reverser",
          "payload": {
            "root": {
              "version": "part1",
          },
          "version": 1
        }
      ]
      ```

### Instrumenting the code to find the bug

Now that you have initialized Inspect and know how to read data, you
are ready to instrument your code and uncover the bug.

The previous output shows you how the component is actually running
and that the component is not hanging completely. Otherwise the Inspect
read would hang.

Add new information per-connection to observe if the connection
is even being handled by your component.

1. Add a new child to your root node to contain statistics about the `reverser` service:

   * {C++}

      ```cpp
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_2/main.cc" region_tag="part_1_new_child" adjust_indentation="auto" %}
      ```

   * {Rust}


      ```rust
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_2/src/main.rs" region_tag="part_1_new_child" adjust_indentation="auto" %}
      ```

2. Update your server to accept this node:

   * {C++}

      Update the definition of `CreateDefaultHandler` in [reverser.h][cpp-part1-reverser-h]
      and [reverser.cc][cpp-part1-reverser-cc]:

      ```cpp
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_2/reverser.h" region_tag="part_1_include" adjust_indentation="auto" %}
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_2/reverser.cc" region_tag="part_1_update_server" adjust_indentation="auto" %}
      ```

   * {Rust}

      Update `ReverserServerFactory::new` to accept this node in [reverser.rs][rust-part1-reverser]:

      ```rust
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_2/src/reverser.rs" region_tag="part_1_use" adjust_indentation="auto" %}
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_2/src/reverser.rs" region_tag="part_1_update_reverser" adjust_indentation="auto" %}
      ```

3. Add a property to keep track of the number of connections:

   Note: Nesting related data under a child is a powerful feature of Inspect.

   * {C++}

      ```cpp
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_2/reverser.cc" region_tag="part_1_add_connection_count" adjust_indentation="auto" %}
      ```

     Note: `node` is moved into the handler so that it is not dropped and
     deleted from the output.

   * {Rust}

      ```rust
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_3/src/reverser.rs" region_tag="part_1_add_connection_count" adjust_indentation="auto" %}
      ```

     Note: `node` is moved into the handler so that it is not dropped and
     deleted from the output.

     Note: `node` is kept in ReverserServerFactory so that it is not dropped and deleted from the
     output together with all the nodes and properties nested under it. If the compiler shows an
     error about dead code, try renaming `node` to `_node`, since the node needs to continue
     existing despite not being read. In the following steps, the example calls `self.node`, so the compiler
     will stop complaining!

   This snippet demonstrates creating a new `UintProperty` (containing a 64
   bit unsigned int) called `connection_count` and setting it to 0. In the handler
   (which runs for each connection), the property is incremented by 1.

4. Rebuild, re-run your component and then run `ffx inspect`:

   * {C++}

      ```posix-terminal
      ffx --machine json-pretty inspect show inspect_cpp_codelab
      ```

   * {Rust}

      ```posix-terminal
      ffx --machine json-pretty inspect show inspect_rust_codelab
      ```

   You should now see:

   ```json5 {:.devsite-disable-click-to-copy}
   ...
   "payload": {
     "root": {
       "version": "part1",
       "reverser_service": {
         "connection_count": 1,
       }
     }
   }
   ```

The output above demonstrates that the client successfully connected
to the service, so the hanging problem must be caused by the Reverser
implementation itself. In particular, it will be helpful to know:

1. If the connection is still open while the client is hanging.

2. If the `Reverse` method was called.


**Exercise**: Create a child node for each connection, and record
"request\_count" inside the Reverser.

- *Hint*: There is a utility function for generating unique names:

   * {C++}

      ```cpp
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_2/reverser.cc" region_tag="part_1_connection_child" adjust_indentation="auto" %}
      ```

   * {Rust}

      ```rust
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_2/src/reverser.rs" region_tag="part_1_connection_child" adjust_indentation="auto" %}
      ```

   This will create unique names starting with "connection".


* {C++}

   *Hint*: You will find it helpful to create a constructor for Reverser
   that takes `inspect::Node`. [Part 3](#part-3) of this codelab explains why this is
   a useful pattern.

* {Rust}

   *Hint*: You will find it helpful to create a constructor for `ReverserServer`
   that takes `inspect::Node` for the same reason as we did for `ReverserServerFactory`.

- *Hint*: You will need to create a member on Reverser to hold the
`request_count` property. Its type will be `inspect::UintProperty`.

- *Follow up*: Does request count give you all of the information you
need? Add `response_count` as well.

- *Advanced*: Can you add a count of *all* requests on *all*
connections? The Reverser objects must share some state. You may find
it helpful to refactor arguments to Reverser into a separate struct
(See solution in [part 2](#part-2) for this approach).

After completing this exercise and running `ffx inspect`, you should see
something like:

```json5 {:.devsite-disable-click-to-copy}
...
"payload": {
  "root": {
    "version": "part1",
    "reverser_service": {
      "connection_count": 1,
      "connection0": {
        "request_count": 1,
      }
    }
  }
}
```

The output above shows that the connection is still open and it received one request.

* {C++}

   If you added "response\_count" as well, you may have noticed the bug.
   The `Reverse` method receives a `callback`, but it is never called with the value of `output`.

* {Rust}

   If you added "response\_count" as well, you may have noticed the bug.
   The `Reverse` method receives a `responder`, but it is never called with the value of `result`.


1. Send the response:

   * {C++}

      ```cpp
      // At the end of Reverser::Reverse
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_2/reverser.cc" region_tag="part_1_callback" adjust_indentation="auto" %}
      ```

   * {Rust}

      ```rust
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_2/src/reverser.rs" region_tag="part_1_respond" adjust_indentation="auto" %}
      ```

2. Run the client again:

   * {C++}

      ```posix-terminal
      ffx component run --recreate /core/ffx-laboratory:client_part_1 fuchsia-pkg://fuchsia.com/inspect_cpp_codelab#meta/client_part_1.cm
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
      Creating component instance: client_part_1

      ffx log --tag inspect_cpp_codelab
      [00039.129068][39163][39165][inspect_cpp_codelab, client] INFO: Input: Hello
      [00039.194151][39163][39165][inspect_cpp_codelab, client] INFO: Output: olleH
      [00039.194170][39163][39165][inspect_cpp_codelab, client] INFO: Input: World
      [00039.194402][39163][39165][inspect_cpp_codelab, client] INFO: Output: dlroW
      [00039.194407][39163][39165][inspect_cpp_codelab, client] INFO: Done reversing! Please use `ffx component stop`
      ```

   * {Rust}

      ```posix-terminal
      ffx component run --recreate /core/ffx-laboratory:client_part_1 fuchsia-pkg://fuchsia.com/inspect_rust_codelab#meta/client_part_1.cm
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
      Creating component instance: client_part_1

      ffx log --tag inspect_rust_codelab
      [00039.129068][39163][39165][inspect_rust_codelab, client] INFO: Input: Hello
      [00039.194151][39163][39165][inspect_rust_codelab, client] INFO: Output: olleH
      [00039.194170][39163][39165][inspect_rust_codelab, client] INFO: Input: World
      [00039.194402][39163][39165][inspect_rust_codelab, client] INFO: Output: dlroW
      [00039.194407][39163][39165][inspect_rust_codelab, client] INFO: Done reversing! Please use `ffx component stop`
      ```

The component continues to run until you execute `ffx component stop`. As long as the component runs
you can run `ffx inspect` and observe your output.

This concludes part 1. Optionally, you may commit your changes:

```posix-terminal
git commit -am "solution to part 1"
```

## Part 2: Diagnosing inter-component problems {#part-2}

Note: All links and examples in this section refer to "part\_2" code. If
you are following along, you may continue using "part\_1."

You received a bug report. The "FizzBuzz" team is saying they
are not receiving data from your component.

In addition to serving the Reverser protocol, the component also reaches
out to the "FizzBuzz" service and prints the response:

* {C++}

   ```cpp
   {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_1/main.cc" region_tag="fizzbuzz_connect" adjust_indentation="auto" %}
   ```

* {Rust}

   ```rust
   {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_1/src/main.rs" region_tag="fizzbuzz_connect" adjust_indentation="auto" %}
   ```

If you see the logs, you will see that this log is never printed.

* {C++}

   ```posix-terminal
   ffx log --tag inspect_cpp_codelab
   ```

* {Rust}

   ```posix-terminal
   ffx log --tag inspect_rust_codelab
   ```

You will need to diagnose and solve this problem.

### Diagnose the issue with Inspect

1. Run the component to see what is happening:

   Note: Replace 2 with 1 if you are continuing from part 1.

   * {C++}

      ```posix-terminal
      ffx component run /core/ffx-laboratory:client_part_2 fuchsia-pkg://fuchsia.com/inspect_cpp_codelab#meta/client_part_2.cm
      ```

   * {Rust}

      ```posix-terminal
      ffx component run /core/ffx-laboratory:client_part_2 fuchsia-pkg://fuchsia.com/inspect_rust_codelab#meta/client_part_2.cm
      ```

   Fortunately the FizzBuzz team instrumented their component using Inspect.

2. Read the FizzBuzz Inspect data using `ffx inspect` as before, you should see:

   ```json5 {:.devsite-disable-click-to-copy}
   "payload": {
       "root": {
           "fizzbuzz_service": {
               "closed_connection_count": 0,
               "incoming_connection_count": 0,
               "request_count": 0,
               ...
   ```

   This output confirms that FizzBuzz is not receiving any connections.

3. Add Inspect to identify the problem:

   * {C++}

      ```cpp
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_2/main.cc" region_tag="instrument_fizzbuzz" adjust_indentation="auto" %}
      ```

   * {Rust}

      ```rust
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_2/src/main.rs" region_tag="instrument_fizzbuzz" adjust_indentation="auto" %}
      ```

**Exercise**: Add Inspect to the FizzBuzz connection to identify the problem

- *Hint*: Use the snippet above as a starting point, it provides an
error handler for the connection attempt.

* {C++}

   *Follow up*: Can you store the status somewhere? You can convert it
   to a string using `zx_status_get_string(status)`.

   *Advanced*: `inspector` has a method called `Health()` that announces
   overall health status in a special location. Since our service is not
   healthy unless it can connect to FizzBuzz, can you incorporate this:

     ```cpp
     /*
     "fuchsia.inspect.Health": {
         "status": "STARTING_UP"
     }
     */
     inspector.Health().StartingUp();

     /*
     "fuchsia.inspect.Health": {
         "status": "OK"
     }
     */
     inspector.Health().Ok();

     /*
     "fuchsia.inspect.Health": {
         "status": "UNHEALTHY",
         "message": "Something went wrong!"
     }
     */
     inspector.Health().Unhealthy("Something went wrong!");
     ```

* {Rust}

   *Advanced*: `fuchsia_inspect::component` has a function called `health()` that returns an object
   that announces overall health status in a special location (a node child of the root of the
   inspect tree). Since our service is not healthy unless it can connect to FizzBuzz, can
   you incorporate this:

   ```rust
   /*
   "fuchsia.inspect.Health": {
       "status": "STARTING_UP"
   }
   */
   fuchsia_inspect::component::health().set_starting_up();

   /*
   "fuchsia.inspect.Health": {
       "status": "OK"
   }
   */
   fuchsia_inspect::component::health().set_ok();

   /*
   "fuchsia.inspect.Health": {
       "status": "UNHEALTHY",
       "message": "Something went wrong!"
   }
   */
   fuchsia_inspect::component::health().set_unhealthy("something went wrong!");
   ```

Once you complete this exercise, you should see that the connection
error handler is being called with a "not found" error. Inspect
output showed that FizzBuzz is running, so maybe something is
misconfigured. Unfortunately not everything uses Inspect (yet!) so
look at the logs:

* {C++}

   ```posix-terminal
   ffx log --filter FizzBuzz
   ```

   You should see an output like:

   ```none {:.devsite-disable-click-to-copy}
   ...
   ...  No capability available at path /svc/fuchsia.examples.inspect.FizzBuzz
   for component /core/ffx-laboratory:client_part_2/reverser, verify the
   component has the proper `use` declaration. ...
   ```

* {Rust}

   ```posix-terminal
   ffx log --filter FizzBuzz
   ```

   You should see an output like:

   ```none {:.devsite-disable-click-to-copy}
   ...
   ... No capability available at path /svc/fuchsia.examples.inspect.FizzBuzz
   for component /core/ffx-laboratory:client_part_2/reverser, verify the
   component has the proper `use` declaration. ...
   ```

Sandboxing errors are a common pitfall that are sometimes difficult to uncover.

Note: While you could have looked at the logs from the beginning to find
the problem, the log output for the system can be extremely verbose. The
particular log that you are looking for was a kernel log from the framework,
which is additionally difficult to test for.

Looking at part2 meta, you can see it is missing the service:

* {C++}

    Add a `use` entry for `Fizzbuzz` to [part_2/meta][cpp-part2-meta]:

    ```json5
    use: [
        { protocol: "fuchsia.examples.inspect.FizzBuzz" },
    ],
    ```

* {Rust}

    Add a `use` entry for `Fizzbuzz` to [part_2/meta][rust-part2-meta]:

    ```json5
    use: [
        { protocol: "fuchsia.examples.inspect.FizzBuzz" },
    ],
    ```

After you added "fuchsia.examples.inspect.FizzBuzz", rebuild,
and run again. You should now see FizzBuzz in the logs and an OK status:

* {C++}

   ```posix-terminal
   ffx log --tag inspect_cpp_codelab
   ```

   You should see an output like:

   ```none {:.devsite-disable-click-to-copy}
   [inspect_cpp_codelab, part2] INFO: main.cc(57): Got FizzBuzz: 1 2 Fizz
   4 Buzz Fizz 7 8 Fizz Buzz 11 Fizz 13 14 FizzBuzz 16 17 Fizz 19 Buzz Fizz
   22 23 Fizz Buzz 26 Fizz 28 29 FizzBuzz
   ```

* {Rust}

   ```posix-terminal
   ffx log --tag inspect_rust_codelab
   ```

   You should see an output like:

   ```none {:.devsite-disable-click-to-copy}
   [inspect_rust_codelab, part2] INFO: main.rs(52): Got FizzBuzz: 1 2 Fizz
   4 Buzz Fizz 7 8 Fizz Buzz 11 Fizz 13 14 FizzBuzz 16 17 Fizz 19 Buzz Fizz
   22 23 Fizz Buzz 26 Fizz 28 29 FizzBuzz
   ```

This concludes Part 2.

Optionally, you can now commit your solution:

```posix-terminal
git commit -am "solution for part 2"
```

## Part 3: Unit Testing for Inspect {#part-3}

Note: All links and examples in this section refer to "part\_3" code. If
you are following along, you may continue using the part you started with.

All code on Fuchsia should be tested, and this applies to Inspect data as well.

While Inspect data is not *required* to be tested in general, you
need to test Inspect data that is depended upon by other tools such as
Triage or Feedback.

Reverser has a basic unit test. Run it:

* {C++}

   The unit tests is located in [reverser\_unittests.cc][cpp-part3-unittest].

   ```posix-terminal
   fx test inspect_cpp_codelab_unittests
   ```

* {Rust}

   The unit test is located in [reverser.rs > mod tests][rust-part3-unittest].

   ```posix-terminal
   fx test inspect_rust_codelab_unittests
   ```

Note: This runs unit tests for all parts of this codelab.

The unit test ensures that Reverser works properly (and doesn't hang!), but it does
not check that the Inspect output is as expected.

Note: If you are following along from part\_1, you will need to uncomment
some lines in the part_1 unit test and pass default values for the Inspect properties to your
Reverser.

Passing Nodes into constructors is a form of [Dependency
Injection](https://en.wikipedia.org/wiki/Dependency_injection), which
allows you to pass in test versions of dependencies to check their state.

The code to open a Reverser looks like the following:

* {C++}

   ```cpp
   {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_3/reverser_unittests.cc" region_tag="open_reverser" adjust_indentation="auto" %}
   // Alternatively
   binding_set_.AddBinding(std::make_unique<Reverser>(inspect::Node()),
                           ptr.NewRequest());
   ```

* {Rust}

   ```rust
   {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_2/src/reverser.rs" region_tag="open_reverser" adjust_indentation="auto" %}
   ```

A default version of the Inspect Node is passed into the Reverser. This
allows the reverser code to run properly in tests, but it does not
support asserting on Inspect output.


* {C++}

   **Exercise**: Change `OpenReverser` to take the dependency for Reverser
   as an argument and use it when constructing Reverser.

   - *Hint*: Create an `inspect::Inspector` in the test function. You can
   get the root using `inspector.GetRoot()`.

   - *Hint*: You will need to create a child on the root to pass in to `OpenReverser`.

* {Rust}

   **Exercise**: Change `open_reverser` to take the dependency for a `ReverserServerFactory`
   as an argument and use it when constructing Reverser.

   - *Hint*: Create a `fuchsia_inspect::Inspector` in the test function. You can
     get the root using `inspector.root()`.

   - *Note*: Do not use `component::inspector()` directly in your tests, this creates a static
     inspector that will be alive in all your tests and can lead to flakes or unexpected behaviors.
     For unit tests, alwas prefer to use a new `fuchsia_inspect::Inspector`

   - *Hint*: You will need to create a child on the root to pass in to `ReverserServerFactory::new`.

**Follow up**: Create multiple reverser connections and test them independently.

Following this exercise, your unit test will set real values in an
Inspect hierarchy.

Add code to test the output in Inspect:

* {C++}

   ```cpp
   {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_4/reverser_unittests.cc" region_tag="include_testing" adjust_indentation="auto" %}
   {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_4/reverser_unittests.cc" region_tag="get_hierarchy" adjust_indentation="auto" %}
   ```

   Note: If you use the LazyNode or LazyValues features, you will need to
   use inspect::ReadFromInspector and run the returned fpromise::promise to
   completion. See the solution to this part for an example.

   The snippet above reads the underlying virtual memory object (VMO)
   containing Inspect data and parses it into a readable hierarchy.

   You can now read individual properties and children as follows:

   ```cpp
   {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_4/reverser_unittests.cc" region_tag="assertions" adjust_indentation="auto" %}
   ```

* {Rust}

   ```rust
   {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_4/src/reverser.rs" region_tag="include_testing" adjust_indentation="auto" %}
   {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_4/src/reverser.rs" region_tag="test_inspector" adjust_indentation="auto" %}
   {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_4/src/reverser.rs" region_tag="assert_tree" adjust_indentation="auto" %}
   ```

The snippets above read a snapshot from the underlying virtual memory object (VMO)
containing Inspect data and parses it into a readable hierarchy.

**Exercise**: Add assertions for the rest of your Inspect data.

This concludes Part 3.

Optionally, you may commit your changes:

```posix-terminal
git commit -am "solution to part 3"
```

## Part 4: Integration Testing for Inspect

Note: All links and examples in this section refer to "part\_4" code. If
you are following along, you may continue using the part you started with.

[Integration testing](https://en.wikipedia.org/wiki/Integration_testing)
is an important part of the software development workflow for
Fuchsia. Integration tests allow you to observe the behavior of your
actual component when it runs on the system.

### Running integration tests

You can run the integration tests for the codelab as follows:

* {C++}

   ```posix-terminal
   fx test inspect_cpp_codelab_integration_tests
   ```

* {Rust}

   ```posix-terminal
   fx test inspect_rust_codelab_integration_tests
   ```

This runs integration tests for all parts of this codelab.

### View the code

Look at how the integration test is setup:

1. View the component manifest for the integration test:

   * {C++}

     Find the component manifest (cml) in [part_4/meta][cpp-part4-integration-meta]

   * {Rust}

     Find the component manifest (cml) in [part_4/meta][rust-part4-integration-meta]

   You should see:

   ```json5
   {
      ...
      use: [
         { protocol: "fuchsia.diagnostics.ArchiveAccessor" },
      ]
   }
   ```

   This file uses the protocol `fuchsia.diagnostics.ArchiveAccessor` from
   parent. This protocol is available to all tests to enable to read diagnostics
   about all components under the test realm.

2. Look at the integration test itself. The individual test cases are fairly straightforward:

   * {C++}

      Locate the integration test in [part4/tests/integration_test.cc][cpp-part4-integration].

      ```cpp
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_4/tests/integration_test.cc" region_tag="integration_test" adjust_indentation="auto" %}
      ```

      `StartComponentAndConnect` is responsible for creating a new test
      environment and starting the codelab component inside of it. The
      `include_fizzbuzz_service` option instructs the method to optionally
      include FizzBuzz. This feature tests that your Inspect output is as
      expected in case it fails to connect to FizzBuzz as in Part 2.

   * {Rust}

      Locate the integration test in [part4/tests/integration_test.rs][rust-part4-integration].

      ```rust
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_4/tests/integration_test.rs" region_tag="integration_test" adjust_indentation="auto" %}
      ```

      `IntegrationTest::start` is responsible for creating a new test
      environment and starting the codelab component inside of it. The
      `include_fizzbuzz` option instructs the method to optionally
      launch the FizzBuzz component. This feature tests that your Inspect
      output is as expected in case it fails to connect to FizzBuzz as in Part 2.

3. Add the following method to your test fixture to read from the ArchiveAccessor service:

   * {C++}

     ```cpp
     {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_5/tests/integration_test.cc" region_tag="include_json" adjust_indentation="auto" %}
     {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_5/tests/integration_test.cc" region_tag="get_inspect" adjust_indentation="auto" %}
     ```

   * {Rust}

     ```rust
     {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_5/tests/integration_test.rs" region_tag="include_test_stuff" adjust_indentation="auto" %}
     {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_5/tests/integration_test.rs" region_tag="get_inspect" adjust_indentation="auto" %}
     ```

4. **Exercise**. Use the returned data in your tests and add assertions to the returned data:

   * {C++}

      ```cpp
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_5/tests/integration_test.cc" region_tag="parse_result" adjust_indentation="auto" %}
      ```

      Add assertions on the returned JSON data.

      - *Hint*: It may help to print the JSON output to view the schema.

      - *Hint*: You can read values by path as follows:

      - *Hint*: You can `EXPECT_EQ` by passing in the expected value as a rapidjson::Value:
        `rapidjson::Value("OK")`.

      ```cpp
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/cpp/part_5/tests/integration_test.cc" region_tag="hint_get_value" adjust_indentation="auto" %}
      ```

   * {Rust}

      ```rust
      {% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/diagnostics/inspect/codelab/rust/part_5/tests/integration_test.rs" region_tag="result_hierarchy" adjust_indentation="auto" %}
      ```

      Add assertions on the returned `DiagnosticsHierarchy`.

      - *Hint*: It may help to print the JSON output to view the schema.


Your integration test will now ensure your inspect output is correct.

This concludes Part 4.

Optionally, you may commit your solution:

```posix-terminal
git commit -am "solution to part 4"
```

## Part 5: Feedback Selectors

This section is under construction.

- TODO: Writing a feedback selector and adding tests to your integration test.

- TODO: Selectors for Feedback and other pipelines

[fidl-fizzbuzz]: /examples/diagnostics/inspect/codelab/fidl/fizzbuzz.test.fidl
[fidl-reverser]: /examples/diagnostics/inspect/codelab/fidl/reverser.test.fidl
[inspect-cpp-codelab]: /examples/diagnostics/inspect/codelab/cpp
[cpp-common-cml]: /examples/diagnostics/inspect/codelab/cpp/client/meta/common.shard.cml
[cpp-part1]: /examples/diagnostics/inspect/codelab/cpp/part_1
[cpp-part1-main]: /examples/diagnostics/inspect/codelab/cpp/part_1/main.cc
[cpp-part1-reverser-h]: /examples/diagnostics/inspect/codelab/cpp/part_1/reverser.h
[cpp-part1-reverser-cc]: /examples/diagnostics/inspect/codelab/cpp/part_1/reverser.cc
[cpp-part1-build]: /examples/diagnostics/inspect/codelab/cpp/part_1/BUILD.gn
[cpp-client-main]: /examples/diagnostics/inspect/codelab/cpp/client/main.cc#118
[cpp-part2-meta]: /examples/diagnostics/inspect/codelab/cpp/part_2/meta/integration_test.cml
[cpp-part3-unittest]: /examples/diagnostics/inspect/codelab/cpp/part_3/reverser_unittests.cc
[cpp-part4-integration]: /examples/diagnostics/inspect/codelab/cpp/part_4/tests/integration_test.cc
[cpp-part4-integration-meta]: /examples/diagnostics/inspect/codelab/cpp/part_4/meta/integration_test.cml
[inspect-rust-codelab]: /examples/diagnostics/inspect/codelab/rust
[rust-common-cml]: /examples/diagnostics/inspect/codelab/rust/client/meta/common.shard.cml
[rust-part1]: /examples/diagnostics/inspect/codelab/rust/part_1
[rust-part1-main]: /examples/diagnostics/inspect/codelab/rust/part_1/src/main.rs
[rust-part1-reverser]: /examples/diagnostics/inspect/codelab/rust/part_1/src/reverser.rs
[rust-part1-build]: /examples/diagnostics/inspect/codelab/rust/part_1/BUILD.gn
[rust-client-main]: /examples/diagnostics/inspect/codelab/rust/client/src/main.rs#41
[rust-part2-meta]: /examples/diagnostics/inspect/codelab/rust/part_2/meta/part_2.cml
[rust-part3-unittest]: /examples/diagnostics/inspect/codelab/rust/part_3/src/reverser.rs#99
[rust-part4-integration]: /examples/diagnostics/inspect/codelab/rust/part_4/tests/integration_test.rs
[rust-part4-integration-meta]: /examples/diagnostics/inspect/codelab/rust/part_4/meta/integration_test.cml
[fuchsia-inspect-derive]: /docs/development/languages/rust/ergonomic_inspect.md
