# Simple Driver

Reviewed on: 2024-02-05

Simple Driver is an example that showcase how to write a DFv2 driver with common
driver patterns:
- Adding logs
- Adding a child node
- Implementing stop functions
- Setting up a compat device server

The example also implements the constructor and destructor to showcase the driver lifecycle.
View the logs to see what order the constructor, DriverBase functions, and destructor are
called in.

## Building

To include the driver to your build, append `--with //examples/drivers:drivers` to your `fx
set` command. For example:

```bash
$ fx set core.x64 --with //examples/drivers:drivers
$ fx build
```

## Running

Note: Ensure you have a package server serving to the emulator.

Register the driver by running this command
```bash
$ ffx driver register fuchsia-pkg://fuchsia.com/simple_driver#meta/simple_driver.cm
```

Verify that `fuchsia-pkg://fuchsia.com/simple_driver#meta/simple_driver.cc` shows up in this command
```bash
ffx driver list
```

Add a test node that binds to the driver:
```bash
$ ffx driver test-node add simple_node gizmo.example.TEST_NODE_ID=simple_driver
```

Run the following command to verify that the driver is bound to the node:
```bash
$ ffx driver list-devices -v simple_node
```

You should see something like this:
```
Name     : simple_node
Moniker  : dev.simple_node
Driver   : fuchsia-pkg://fuchsia.com/simple_driver#meta/simple_driver.cm
1 Properties
[ 1/  1] : Key "gizmo.example.TEST_NODE_ID"   Value "simple_driver"
0 Offers

Name     : simple_child
Moniker  : dev.simple_node.simple_child
Driver   : unbound
1 Properties
[ 1/  1] : Key "fuchsia.test.TEST_CHILD"      Value "simple"
0 Offers
```

If you want to explore the driver lifecycle and test the driver's stop functions, stop the driver by
removing the node:
```bash
$ ffx driver test-node remove simple_node
```

View the driver logs with `ffx log --filter simple` to see the execution order of the driver functions.

## Testing

Include the tests to your build by appending `--with //examples/drivers:tests` to your `fx
set` command. For example:

```bash
$ fx set core.x64 --with //examples/drivers:drivers --with //examples:tests
$ fx build
```

Run unit tests with the command:
```bash
$ fx test simple-driver-test
```

The unit test runs the simple DFv2 driver and then verifies to see if
a child was added.

## Source layout

The core implementation is in  `simple_driver.cc`. Unit tests are located in `tests`.
