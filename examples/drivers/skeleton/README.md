# Skeleton Driver

Reviewed on: 2024-02-06

Skeleton driver is an minimal driver written in DFv2.

## Building

To include the driver to your build, append `--with //examples/drivers:drivers` to your `fx set` command. For example:

```bash
$ fx set core.x64 --with //examples/drivers:drivers
$ fx build
```

## Running

Register the driver by running this command
```bash
$ ffx driver register fuchsia-pkg://fuchsia.com/skeleton_driver#meta/skeleton_driver.cm
```

Verify that `fuchsia-pkg://fuchsia.com/skeleton_driver#meta/skeleton_driver.cm` shows up in the list after running this command
```bash
ffx driver list
```

Add a test node that binds to the driver:
```bash
$ ffx driver test-node add my_node gizmo.example.TEST_NODE_ID=skeleton_driver
```

Run the following command to verify that the driver is bound to the node:
```bash
$ ffx driver list-devices -v my_node
```

You should see something like this:
```
Name     : my_node
Moniker  : dev.my_node
Driver   : fuchsia-pkg://fuchsia.com/skeleton_driver#meta/skeleton_driver.cm
1 Properties
[ 1/  1] : Key "gizmo.example.TEST_NODE_ID"   Value "skeleton_driver"
0 Offers
```

## Source layout

The core implementation is in  `skeleton_driver.cc`.
