# Driver Transport Rust Example

Reviewed on: 2025-07-29

This example demonstrates a parent driver serving the fuchsia.hardware.i2c FIDL
protocol over driver transport and a child driver that connects to the protocol
to interact with the parent driver, with both drivers written in rust and using
the new rust wire bindings.

## Building

To include the driver to your build, append `--with //examples/drivers:drivers`
to your `fx set` command. For example:

```bash
$ fx set core.x64 --with //examples/drivers:drivers
$ fx build
```

## Running

Register the parent driver by running this command (note: this driver doesn't exist yet)
```bash
$ ffx driver register fuchsia-pkg://fuchsia.com/driver_transport_rust_next#meta/driver_transport_parent.cm
```

Register the child driver by running this command
```bash
$ ffx driver register fuchsia-pkg://fuchsia.com/driver_transport_rust_next#meta/driver_transport_child.cm
```

Verify that both drivers show up in this command
```bash
ffx driver list
```

Add a test node that binds to the parent driver:
```bash
$ ffx driver test-node add driver_transport_parent gizmo.example.TEST_NODE_ID=driver_transport_parent
```

Run the following command to verify that the driver is bound to the node:
```bash
$ ffx driver list-devices -v transport_rust
```

You should see something like this:
```
Name     : driver_transport_rust_next_parent
Moniker  : dev.driver_transport_rust_next_parent
Driver   : fuchsia-pkg://fuchsia.com/driver_transport_rust_next#meta/driver_transport_parent.cm
1 Properties
[ 1/  1] : Key "gizmo.example.TEST_NODE_ID"   Value "driver_transport_rust_next_parent"
0 Offers

Name     : driver_transport_rust_next_child
Moniker  : dev.driver_transport_rust_next_parent.driver_transport_rust_next_child
Driver   : fuchsia-pkg://fuchsia.com/driver_transport_rust_next#meta/driver_transport_child.cm
1 Properties
[ 1/  1] : Key "fuchsia.Service"              Value "fuchsia.hardware.i2cimpl.Service"
1 Offers
Service: fuchsia.hardware.i2cimpl.Service
  Source: dev.driver_transport_rust_next_parent
  Instances: default

Name     : transport-child
Moniker  : dev.driver_transport_rust_next_parent.driver_transport_rust_next_child.transport-child
Driver   : unbound
1 Properties
[ 1/  1] : Key "fuchsia.test.TEST_CHILD"      Value "rust i2cimpl server"
0 Offers
```

## Testing

Include the tests to your build by appending `--with-test
//examples/drivers:hermetic_tests` to your `fx set` command. For example:

```bash
$ fx set core.x64 --with //examples/drivers:drivers --with-test //examples/drivers:hermetic_tests
$ fx build
```

Run unit tests with the command:
```bash
$ fx test driver-rust-next-driver
```

This will run unit tests against both the parent and child driver implementations.

## Source layout

The core implementation of the parent driver is in `parent/src/*.rs` and the child driver
is in `child/src/*.rs`.
