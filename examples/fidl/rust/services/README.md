# Rust FIDL Services Example

This directory contains example code for a service.

## Building

To build the components for this example, include the following in your `fx set`
command:

```
--with //examples/fidl/rust/services:echo-service-rust
```

Then, run `fx build`.

## Running the example

To run the example, run the `echo_realm` component.
This creates the client and server component instances and routes the
capabilities:

```posix-terminal
ffx component run /core/ffx-laboratory:echo_realm fuchsia-pkg://fuchsia.com/echo-service-rust#meta/echo_realm.cm
```

Then, we can start the `echo_client` instance:

```posix-terminal
ffx component start /core/ffx-laboratory:echo_realm/echo_client
```

The server component starts when the client attempts to connect to the `Echo`
protocol. You should see the following output using `fx log`:

```none {:.devsite-disable-click-to-copy}
[echo_server] INFO: Received EchoString request for string "hello world!"
[echo_server] INFO: Response sent successfully
[echo_client] INFO: regular response: "hello world!"
[echo_server] INFO: Received EchoString request for string "hello world!"
[echo_server] INFO: Response sent successfully
[echo_client] INFO: reversed response: "!dlrow olleh"
```
