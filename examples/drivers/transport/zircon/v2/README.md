# Zircon Transport Example

Reviewed on: 2024-03-21

This example demonstrates a parent driver serving the fuchsia.hardware.i2c FIDL protocol over zircon transport and a child driver that connects to the protocol to interact with the parent driver.

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
$ ffx driver register fuchsia-pkg://fuchsia.com/zircon_transport#meta/zircon_transport_parent.cm
```

Register the child driver by running this command
```bash
$ ffx driver register fuchsia-pkg://fuchsia.com/zircon_transport#meta/zircon_transport_child.cm
```

Verify that both drivers show up in this command
```bash
ffx driver list
```

Add a test node that binds to the parent driver:
```bash
$ ffx driver test-node add zircon_transport_parent gizmo.example.TEST_NODE_ID=zircon_transport_parent
```

Run the following command to verify that the driver is bound to the node:
```bash
$ ffx driver list-devices -v transport
```

You should see something like this:
```
Name     : zircon_transport_parent
Moniker  : dev.zircon_transport_parent
Driver   : fuchsia-pkg://fuchsia.com/zircon_transport#meta/zircon_transport_parent.cm
1 Properties
[ 1/  1] : Key "gizmo.example.TEST_NODE_ID"   Value "zircon_transport_parent"
0 Offers

Name     : zircon_transport_child
Moniker  : dev.zircon_transport_parent.zircon_transport_child
Driver   : fuchsia-pkg://fuchsia.com/zircon_transport#meta/zircon_transport_child.cm
1 Properties
[ 1/  1] : Key "fuchsia.Service"              Value "fuchsia.hardware.i2c.Service"
1 Offers
Service: fuchsia.hardware.i2c.Service
  Source: dev.zircon_transport_parent
  Instances: default

Name     : transport-child
Moniker  : dev.zircon_transport_parent.zircon_transport_child.transport-child
Driver   : unbound
0 Properties
0 Offers
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
$ fx test zircon_transport_example_test
```

The unit test sets up a fake driver transport server for the child driver and verifies that
the driver successfully queried the values from the server.

## Source layout

The core implementation of the parent driver is in `parent-driver.cc` and the child driver
is in `child-driver.cc`. Unit tests are located in `tests`.
