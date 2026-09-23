# New C++ FIDL bindings examples

This directory contains example code for a service.

## Running the example

To run the example, run the `echo_realm` component.
This creates the client and server component instances and routes the
capabilities:

```posix-terminal
ffx component run /core/ffx-laboratory:echo_realm fuchsia-pkg://fuchsia.com/echo-service-cpp-wire#meta/echo_realm.cm
```

Then, we can start the `echo_client` instance:

```posix-terminal
ffx component start /core/ffx-laboratory:echo_realm/echo_client
```

The server component starts when the client attempts to connect to the `Echo`
protocol. You should see the following output using `fx log`:

```none {:.devsite-disable-click-to-copy}
[echo_server] INFO: Running echo server
[echo_server] INFO: Got echo request: hello
[echo_server] INFO: Sending response: hello
[echo_client] INFO: Received response: hello
[echo_server] INFO: Client disconnected
```
