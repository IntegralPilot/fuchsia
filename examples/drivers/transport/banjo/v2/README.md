# Banjo Transport Driver Example

Reviewed on: 2024-02-27

This example demonstrates a parent driver serving a Banjo transport and a child driver
that connects and queries data from it.

## Building

To include the driver to your build, append `--with //examples/drivers:drivers` to your `fx
set` command. For example:

```bash
$ fx set core.x64 --with //examples/drivers:drivers
$ fx build
```

## Running

Register the parent driver by running this command
```bash
$ ffx driver register fuchsia-pkg://fuchsia.com/banjo_transport#meta/banjo_transport_parent.cm
```

Register the child driver by running this command
```bash
$ ffx driver register fuchsia-pkg://fuchsia.com/banjo_transport#meta/banjo_transport_child.cm
```

Verify that both drivers show up in this command
```bash
ffx driver list
```

Add a test node that binds to the parent driver:
```bash
$ ffx driver test-node add banjo_transport_parent gizmo.example.TEST_NODE_ID=banjo_parent
```

Run the following command to verify that the driver is bound to the node:
```bash
$ ffx driver list-devices -v banjo_transport
```

You should see something like this:
```
Name     : banjo_transport_parent
Moniker  : dev.banjo_transport_parent
Driver   : fuchsia-pkg://fuchsia.com/banjo_transport#meta/banjo_transport_parent.cm
1 Properties
[ 1/  1] : Key "gizmo.example.TEST_NODE_ID"   Value "banjo_parent"
0 Offers

Name     : banjo-transport-child
Moniker  : dev.banjo_transport_parent.banjo-transport-child
Driver   : fuchsia-pkg://fuchsia.com/banjo_transport#meta/banjo_transport_child.cm
3 Properties
[ 1/  3] : Key fuchsia.BIND_PROTOCOL          Value 0x00001c
[ 2/  3] : Key "gizmo.example.TEST_NODE_ID"   Value "banjo_child"
[ 3/  3] : Key "fuchsia.Service"              Value "fuchsia.driver.compat.Service"
1 Offers
Service: fuchsia.driver.compat.Service
  Source: dev.banjo_transport_parent
  Instances: default
```

## Testing

Include the tests to your build by appending `--with //examples/drivers:tests` to your `fx
set` command. For example:

```bash
$ fx set core.x64 --with //examples/drivers:drivers --with //examples:tests
$ fx build
```

Run unit tests with the command:
```bash
$ fx test banjo_transport_example_test
```

The unit test sets up a fake banjo server for the child driver and verifies that the
driver successfully queried the values from the server.

## Source layout

The core implementation of the parent driver is in `parent-driver.cc` and the child driver
is in `child-driver.cc`. Unit tests are located in `tests`.
