# Scripting remote interaction with a Fuchsia device

This guide contains a tutorial for writing a script to remotely interact with a
Fuchsia device, various other example scripted interactions, and a primer on
implementing a FIDL server in Python on a host.

The methods on this page were originally developed for end-to-end testing, but
you may also find them useful for scripting simple device interactions without
having to write an `ffx` plugin in Rust.

## Background

The following technologies enable scripting remote interactions with a Fuchsia
device:

- [Python FIDL bindings][python-fidl-bindings]
- [Fuchsia Controller][fuchsia-controller]
- [Overnet][overnet]

Together, Fuchsia Controller and Overnet provide a transport between the host
running a Python script and the target Fuchsia device. And the Python FIDL
bindings, like FIDL bindings in other languages, facilitate sending and
receiving messages from components on the target device.

You generally only need to interact with Python FIDL bindings after connecting
to your target device. Connecting to the target device, as you will see, is
done with the `Context` class from the `fuchsia_controller_py` module.

Setting Overnet aside, the `libfuchsia_controller_internal.so` shared library
(which includes the ABI [header][fuchsia-controller-header-file]) is the core
library that drives scripted remote interactions.

## The Context object

The `Context` object is the main entry point for all Fuchsia controller
interactions. It is responsible for creating and managing connections to Fuchsia
devices and is the primary method for getting access to FIDL handles.

When constructed, the `Context` object can take a dictionary of configuration
key/value pairs, similar to the usage of [`ffx config`][ffx-config].

```python
config = { "log.dir": "/path/to/logs", "log.level": "trace" }
ctx = Context(target="nodename", config=config)
```

In most cases, a user must specify a target, which is the nodename of a Fuchsia
device.

The most common way of using this object is to connect to a specific FIDL
protocol on a component running on a Fuchsia device. This is done by calling the
`connect_to_protocol` method, which takes a component moniker and a protocol
name.

## Tutorial

This tutorial walks through writing a Python script that reads a list of
addresses and prints information about the Fuchsia device at each address.

Note: Most FIDL interfaces should just work. However, some advanced APIs are not
supported, e.g., reading from or writing to a VMO.

1. [Prerequisites](#prerequisites)
2. [Create a build target](#create-a-build-target)
3. [Write the script](#write-the-script)
4. [Build and run the script](#build-and-run-the-script)

Please [file an issue][file-an-issue] if you encounter bugs, or have questions
or suggestions.

### Prerequisites {:.numbered}

This tutorial requires the following:

- [Fuchsia source checkout and associated development environment.][get-started]
- Fuchsia device (physical or emulated) reachable via `ffx` that exposes
  the remote control service (RCS).

  When running `ffx target list`, the field under `RCS` must read `Y`:

  ```none {:.devsite-disable-click-to-copy}
  NAME                    SERIAL       TYPE       STATE      ADDRS/IP                       RCS
  fuchsia-emulator        <unknown>    Unknown    Product    [fe80::5054:ff:fe63:5e7a%4]    Y
  ```

  (For more information, see
  [Interacting with target devices][interact-with-target-devices].)

### Create a build target {:.numbered}

Update a `BUILD.gn` file to include a build target like the following:

```none {:.devsite-disable-click-to-copy}
import("//build/python/python_binary.gni")

assert(is_host)

{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="tools/fidl/fidlgen_python/BUILD.gn" region_tag="describe_host_example_build_target" %}
```

The `ffx` libraries enable connecting to our Fuchsia device. And
the FIDL dependencies make the necessary Python FIDL bindings available to the
script.

### Write the script {:.numbered}

First, our script needs to import the necessary modules:

```py
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="tools/fidl/fidlgen_python/examples/describe_host.py" region_tag="required_import_block" %}
```

The `fidl_fuchsia_developer_remotecontrol` and `fidl_fuchsia_buildinfo` Python
modules contains the Python FIDL bindings for the `fuchsia.developer.ffx` and
`fuchsia.buildinfo` FIDL libraries.

The `Context` object provides connections to Fuchsia targets.

Next, our script needs to define a function (called `describe_host` in this
example) to retrieve information from a target device:

```py
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="tools/fidl/fidlgen_python/examples/describe_host.py" region_tag="describe_host_function" %}
```

This function instantiates a `Context` to connect to a Fuchsia device at
particular IP address, and then call the
`fuchsia.developer.remotecontrol/RemoteControl.IdentifyHost` and
`fuchsia.buildinfo/Provider.GetBuildInfo` methods to get information about the
target.

Note: You might notice the component moniker `core/build-info` in the script.
See the [Finding component monikers][#finding-component-monikers] for how to
discover the component moniker for the Fuchsia component you wish to communicate
with.

Note: The `.unwrap()` called on the result of the `IdentifyHost` call is a
helper method defined for FIDL result types. It either returns the response
contained in the result, if there is one, or raises an AssertionError if the
result contains a framework or domain error. (The `.response` in this example is
a little misleading because the returned `RemoteControlIdentifyHostResult` has a
response field with type `RemoteControlIdentifyHostResponse` that also contains
a response field. There are two nested response fields in the value returned by
`IdentifyHost`).

Finally, you wrap this code with some Python boilerplate to read addresses and
print information for each target device. Thus, you arrive at the following
script:

```py
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="tools/fidl/fidlgen_python/examples/describe_host.py" region_tag="full_code" %}
```

### Build and run the script {:.numbered}

This example code lives in `//tools/fidl/fidlgen_python`, so you build it with
the following command on an `x64` host (after adding
`//tools/fidl/fidlgen_python:examples` to `host_labels` with `fx args`):

```posix-terminal
fx build --host //tools/fidl/fidlgen_python:describe_host_example
```

Next, you use `ffx target list` to identify the address of our target device.

```sh {:.devsite-disable-click-to-copy}
$ ffx target list
NAME                      SERIAL          TYPE        STATE      ADDRS/IP                                       RCS
fuchsia-emulator          <unknown>       core.x64    Product    [127.0.0.1:34953]                              Y
```

Then you run the script!

```sh {:.devsite-disable-click-to-copy}
$ fx run-in-build-dir host_x64/obj/tools/fidl/fidlgen_python/describe_host_example.pyz '127.0.0.1:34953'
Target Info Received:

    --- 127.0.0.1:34953 ---
    nodename: fuchsia-emulator
    product_config: core
    board_config: x64
    version: 2025-04-08T02:04:13+00:00
```

## Finding component monikers
To communicate with a Fuchsia component, a script must know the component's
moniker in advance. A component moniker can be retrieved using `ffx`. For
example, the following `ffx` command will print that `core/build-info` exposes
the `fuchsia.buildinfo/Provider` capability:

```posix-terminal
ffx component capability fuchsia.buildinfo.Provider
```

This command will print output similar to the following:

```sh {:.devsite-disable-click-to-copy}
Declarations:
  `core/build-info` declared capability `fuchsia.buildinfo.Provider`

Exposes:
  `core/build-info` exposed `fuchsia.buildinfo.Provider` from self to parent

Offers:
  `core` offered `fuchsia.buildinfo.Provider` from child `#build-info` to child `#cobalt`
  `core` offered `fuchsia.buildinfo.Provider` from child `#build-info` to child `#remote-control`
  `core` offered `fuchsia.buildinfo.Provider` from child `#build-info` to child `#sshd-host`
  `core` offered `fuchsia.buildinfo.Provider` from child `#build-info` to child `#test_manager`
  `core` offered `fuchsia.buildinfo.Provider` from child `#build-info` to child `#testing`
  `core` offered `fuchsia.buildinfo.Provider` from child `#build-info` to child `#toolbox`
  `core/sshd-host` offered `fuchsia.buildinfo.Provider` from parent to collection `#shell`

Uses:
  `core/remote-control` used `fuchsia.buildinfo.Provider` from parent
  `core/sshd-host/shell:sshd-0` used `fuchsia.buildinfo.Provider` from parent
  `core/cobalt` used `fuchsia.buildinfo.Provider` from parent
```

## Other examples

This section demonstrates various other scripted interactions.

Note: For more information on Python FIDL bindings, see this
[Python FIDL bindings][python-fidl-bindings] page.

### Reboot a device {:.numbered}

There's more than one way to reboot a device. One approach to reboot a device is
to connect to a component running the
`fuchsia.hardware.power.statecontrol/Admin` protocol, which can be found under
`/bootstrap/shutdown_shim`.

With this approach, the protocol is expected to exit mid-execution of the method
with a `PEER_CLOSED` error:

```py
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="src/developer/ffx/lib/fuchsia-controller/end_to_end_tests/mobly/reboot_test.py" region_tag="reboot_example" %}
```

However, a challenging part comes afterward when you need to determine whether
or not the device has come back online. This is usually done by attempting to
connect to a protocol (usually the `RemoteControl` protocol) until a timeout is
reached.



### Run a component

Note: This section may be subject to change depending on the development in the
component framework.

You can use the `RemoteControl` protocol to start a component, which involves
the following steps:

1. Connect to the lifecycle controller:

   ```py
   import fidl_fuchsia_developer_remotecontrol as f_remotecontrol
   import fidl_fuchsia_sys2 as f_sys2
   ch = ctx.connect_to_remote_control_proxy()
   remote_control_proxy = f_remotecontrol.RemoteControlClient(ch)
   client, server = fuchsia_controller_py.Channel.create()
   await remote_control_proxy.root_lifecycle_controller(server=server.take())
   lifecycle_ctrl = f_sys2.LifecycleControllerClient(client)
   ```

2. Attempt to start the instance of the component:

   ```py
   import fidl_fuchsia_component as f_component
   client, server = fuchsia_controller_py.Channel.create()
   await lifecycle_ctrl.start_instance("some_moniker", server=server.take())
   binder = f_component.BinderClient(client)
   ```

   The `binder` object lets the user know whether or not the component remains
   connected. However, it has no methods. Support to determine whether the
   component has become unbound (using the binder protocol) is not yet
   implemented.

### Get a snapshot

Getting a snapshot from a fuchsia device involves running a snapshot and binding
a `File` protocol for reading:

```py
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="src/developer/ffx/lib/fuchsia-controller/end_to_end_tests/mobly/target_identity_tests.py" region_tag="snapshot_example" %}
```

## Implementing a FIDL server on a host

An important task for Fuchsia Controller (either for handling passed bindings or
for testing complex client side code) is to run a FIDL server. In this section,
you return to the `echo` example and implement an `echo` server. The functions
you need to override are derived from the FIDL file definition. So the `echo`
server (using the `ffx` protocol) would look like below:

```py
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="tools/fidl/fidlgen_python/tests/test_server_and_event_handler.py" region_tag="echo_server_impl" %}
```

To make a proper implementation, you need to import the appropriate libraries.
As before, you will import `fidl_fuchsia_developer_ffx`. However, since you're
going to run an `echo` server, the quickest way to test this server is to use a
`Channel` object from the `fuchsia_controller_py` library:

```py
import fidl_fuchsia_developer_ffx as ffx
from fuchsia_controller_py import Channel
```

This `Channel` object behaves similarly to the ones in other languages. The
following code is a simple program that utilizes the `echo` server:

```py
import asyncio
import unittest
import fidl_fuchsia_developer_ffx as ffx
from fuchsia_controller_py import Channel


{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="tools/fidl/fidlgen_python/tests/test_server_and_event_handler.py" region_tag="echo_server_impl" %}


class TestCases(unittest.IsolatedAsyncioTestCase):

    async def test_echoer_example(self):
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="tools/fidl/fidlgen_python/tests/test_server_and_event_handler.py" region_tag="use_echoer_example" %}
```

There are a few things to note when implementing a server:

* Method definitions can either be `sync` or `async`.
* The `serve()` task will process requests and call the necessary method in
  the server implementation until either the task is completed or the
  underlying channel object is closed.
* If an exception occurs when the serving task is running, the client
  channel receives a `PEER_CLOSED` error. Then you must check the result
  of the serving task.
* Unlike Rust's async code, when creating an async task, you must keep
  the returned object until you're done with it. Otherwise, the task may
  be garbage collected and canceled.

### Common FIDL server code patterns

Note: For more information on writing async Python code with Fuchsia Controller,
see this [async Python][async-python] page.

In contrast to the simple `echo` server example above, this section covers
different types of server interactions.

#### Creating a FIDL server class

Let's work with the following FIDL protocol to make a server:

```fidl
library fuchsia.exampleserver;

type SomeGenericError = flexible enum {
    THIS = 1;
    THAT = 2;
    THOSE = 3;
};

closed protocol Example {
    strict CheckFileExists(struct {
        path string:255;
        follow_symlinks bool;
    }) -> (struct {
        exists bool;
    }) error SomeGenericError;
};
```

FIDL method names are derived by changing the method name from Camel case to
Lower snake case. So the method `CheckFileExists` in Python changes to
`check_file_exists`.

The anonymous struct types is derived from the whole protocol name and
method. As a result, they can be quite verbose. The input method's input
parameter is defined as a type called `ExampleCheckFileExistsRequest`. And
the response is called `ExampleCheckFileExistsResponse`.

Putting these together, the FIDL server implementation in Python looks like
below:

```py
import fidl_fuchsia_exampleserver as fe

class ExampleServerImpl(fe.ExampleServer):

    def some_file_check_function(path: str) -> bool:
        # Just pretend a real check happens here.
        return True

    def check_file_exists(self, req: fe.ExampleCheckFileExistsRequest) -> fe.ExampleCheckFileExistsResponse:
        return fe.ExampleCheckFileExistsResponse(
            exists=ExampleServerImpl.some_file_check_function()
        )
```

It is also possible to implement the methods as `async` without issues.

In addition, returning an error requires wrapping the error in the FIDL
`DomainError` object, for example:

```py
import fidl_fuchsia_exampleserver as fe

from fidl import DomainError

class ExampleServerImpl(fe.ExampleServer):

    def check_file_exists(self, req: fe.ExampleCheckFileExistsRequests) -> fe.ExampleCheckFileExistsResponse | DomainError:
        return DomainError(error=fe.SomeGenericError.THIS)
```

#### Handling events

Event handlers are written similarly to servers. Events are handled on the
client side of a channel, so passing a client is necessary to construct an event
handler.

Let's start with the following FIDL code to build an example:

```fidl
library fuchsia.exampleserver;

closed protocol Example {
    strict -> OnFirst(struct {
        message string:128;
    });
    strict -> OnSecond();
};
```

This FIDL example contains two different events that the event handler needs
to handle. Writing the simplest class that does nothing but print looks like
below:

```py
import fidl_fuchsia_exampleserver as fe

class ExampleEventHandler(fe.ExampleEventHandler):

    def on_first(self, req: fe.ExampleOnFirstRequest):
        print(f"Got a message on first: {req.message}")

    def on_second(self):
        print(f"Got an 'on second' event")
```

If you want to stop handling events without error, you can raise
`fidl.StopEventHandler`.

An example of this event can be tested using some existing fidlgen_python
testing code. But first, make sure that the Fuchsia controller tests have been
added to the Fuchsia build settings, for example:

```sh {:.devsite-disable-click-to-copy}
fx set ... --with-host //tools/fidl/fidlgen_python:tests
```

With a protocol from `fuchsia.controller.test` (defined in
[`fuchsia_controller.test.fidl`][test-fidl]), you can write code that
uses the `ExampleEvents` protocol, for example:

```py
import asyncio
import fidl_fuchsia_controller_test as fct

from fidl import StopEventHandler
from fuchsia_controller_py import Channel

class ExampleEventHandler(fct.ExampleEventsEventHandler):

    def on_first(self, req: fct.ExampleEventsOnFirstRequest):
        print(f"Got on-first event message: {req.message}")

    def on_second(self):
        print(f"Got on-second event")
        raise StopEventHandler

async def main():
    client_chan, server_chan = Channel.create()
    client = fct.ExampleEventsClient(client_chan)
    server = fct.ExampleEventsServer(server_chan)
    event_handler = ExampleEventHandler(client)
    event_handler_task = asyncio.get_running_loop().create_task(
        event_handler.serve()
    )
    server.on_first(message="first message")
    server.on_second()
    server.on_complete()
    await event_handler_task

if __name__ == "__main__":
    asyncio.run(main())
```

Then this can be run by completing the Python environment setup steps in the
[next section](#experiment-with-the-python-interpreter). When run, it prints
the following output and exits:

```sh {:.devsite-disable-click-to-copy}
Got on-first event message: first message
Got on-second event
```

For more examples on server testing, see this
[`test_server_and_event_handler.py`][test-server-and-event-handler] file.

<!-- Reference links -->

[python-fidl-bindings]: /docs/development/tools/fuchsia-controller/fidl-bindings.md
[fuchsia-controller]: /src/developer/ffx/lib/fuchsia-controller/README.md
[overnet]: /src/connectivity/overnet/README.md
[fuchsia-controller-header-file]: /src/developer/ffx/lib/fuchsia-controller/cpp/fuchsia_controller_internal/fuchsia_controller.h
[file-an-issue]: https://issuetracker.google.com/issues/new?component=1378581&template=1840403
[interact-with-target-devices]: /docs/development/tools/ffx/getting-started.md#interacting_with_target_devices
[test-server-and-event-handler]: /tools/fidl/fidlgen_python/tests/test_server_and_event_handler.py
[test-fidl]: /src/developer/ffx/lib/fuchsia-controller/fidl/fuchsia_controller.test.fidl
[async-python]: /docs/development/tools/fuchsia-controller/async-python.md
[get-started]: /docs/get-started/README.md
[ffx-config]: /docs/development/tools/ffx/commands/config.md#runtime-configuration
