# Simple Driver DFv1

Reviewed on: 2024-02-20

Simple Driver is an example that showcase how to write a DFv1 driver with common driver patterns:
- Adding logs
- Adding a child node

This example also demonstrate the driver lifecycle such as Init(), Suspend(), and
Release().

## Building

To include the driver to your build, append `--with //examples/drivers:drivers` to your `fx
set` command. For example:

```bash
$ fx set core.x64 --with //examples/drivers:drivers
$ fx build
```

## Running

Register the driver by running this command
```bash
$ ffx driver register fuchsia-pkg://fuchsia.com/simple_dfv1#meta/simple_dfv1.cm
```

Verify that `fuchsia-pkg://fuchsia.com/simple_dfv1#meta/simple_dfv1.cm` shows up in this command
```bash
ffx driver list
```

Add a test node that binds to the driver:
```bash
$ ffx driver test-node add simple_node_v1 gizmo.example.TEST_NODE_ID=simple_driver_v1
```

Run the following command to verify that the driver is bound to the node:
```bash
$ ffx driver list-devices -v simple_node_v1
```

You should see something like this:
```
Name     : simple_node_v1
Moniker  : dev.simple_node
Driver   : fuchsia-pkg://fuchsia.com/simple_dfv1#meta/simple_dfv1.cm
1 Properties
[ 1/  2] : Key "gizmo.example.TEST_NODE_ID"   Value "simple_driver_v1"
0 Offers

Name     : simple_child
Moniker  : dev.simple_node_v1.simple_child
Driver   : unbound
2 Properties
[ 1/  2] : Key "fuchsia.test.TEST_CHILD"      Value "simple"
[ 2/  2] : Key "fuchsia.Service"              Value "fuchsia.driver.compat.Service"
1 Offers
Service: fuchsia.driver.compat.Service
  Source: dev.simple_node_v1
  Instances: default
```

If you want to explore the driver lifecycle and test the driver's stop functions, stop the driver by
removing the node:
```bash
$ ffx driver test-node remove simple_node_v1
```

View the driver logs with `ffx log --filter simple` to see the execution order of the driver functions.

## Source layout

The core implementation is in `simple_driver.cc`.
