# Simple Rust Driver

Reviewed on: 2024-10-03

Simple Rust Driver is an example that showcase how to write a Rust driver with
common driver patterns:
- Adding logs
- Adding a child node
- Implementing start and stop functions

## Building

To include the driver to your build, append `--with //examples/drivers:drivers`
to your `fx set` command. For example:

```bash
$ fx set core.x64 --with //examples/drivers:drivers
$ fx build
```

## Running

Register the driver by running this command
```bash
$ ffx driver register fuchsia-pkg://fuchsia.com/simple_rust_driver#meta/simple_rust_driver.cm
```

Verify that
`fuchsia-pkg://fuchsia.com/simple_rust_driver#meta/simple_rust_driver.cm` shows
up in the list after running this command
```bash
ffx driver list
```

Add a test node that binds to the driver:
```bash
$ ffx driver test-node add my_node gizmo.example.TEST_NODE_ID=simple_rust_driver
```

Run the following command to verify that the driver is bound to the node:
```bash
$ ffx driver list-devices -v my_node
```

You should see something like this:
```
Name     : my_node
Moniker  : dev.my_node
Driver   : fuchsia-pkg://fuchsia.com/simple_rust_driver#meta/simple_rust_driver.cm
1 Properties
[ 1/  1] : Key "gizmo.example.TEST_NODE_ID"   Value "simple_rust_driver"
0 Offers

Name     : simple_child
Moniker  : dev.my_node.simple_child
Driver   : unbound
1 Properties
[ 1/  1] : Key "fuchsia.test.TEST_CHILD"      Value "simple"
0 Offers
```

If you want to explore the driver lifecycle and test the driver's stop
functions, stop the driver by removing the node:
```bash
$ ffx driver test-node remove my_node
```

View the driver logs with `ffx log --filter simple_rust_driver` to see the
execution order of the driver functions.

## Testing

Include the tests to your build by appending `--with //examples/drivers:tests` to your `fx
set` command. For example:

```bash
$ fx set core.x64 --with //examples/drivers:drivers --with //examples:tests
$ fx build
```

Run unit tests with the command:
```bash
$ fx test simple-rust-driver
```

The tests run the simple rust driver and then verify that a child was added with
the correct properties.

## Source layout

The core implementation is in  `src/lib.rs`
