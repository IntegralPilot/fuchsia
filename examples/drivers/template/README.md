# Template Driver

Reviewed on: 2024-02-26

Template driver is an minimal driver written in DFv2, and built with SDK rules, which can be used either in-tree or out-of-tree.

## Building

To include the driver to your build, append `--with //examples/drivers:drivers` to your `fx set` command. For example:

```bash
$ fx set core.x64 --with //examples/drivers:drivers
$ fx build
```

## Running

### In-tree
Start a package server for adding the driver to:
```bash
$ fx serve
```

Register the driver by running this command in-tree

```bash
$ ffx driver register fuchsia-pkg://fuchsia.com/template#meta/template.cm
```

### Out-of-tree
To get an SDK provisioned out of tree repository, make a request for a new Fuchsia managed repository to Infra.
See go/fuchsia-oot-driver-migration for further instructions.
From an SDK provisioned repository bazel can run the driver:
```bash
$ bazel run //examples/drivers/template:pkg.publish
```

Verify that `fuchsia-pkg://fuchsia.com/template_driver#meta/template.cm` shows up in the list after running this command
```bash
ffx driver list
```

Add a test node that binds to the driver:
```bash
$ ffx driver test-node add my_node gizmo.example.TEST_NODE_ID=template
```

Run the following command to verify that the driver is bound to the node:
```bash
$ ffx driver list-devices -v my_node
```

You should see something like this:
```
Name     : my_node
Moniker  : dev.my_node
Driver   : fuchsia-pkg://fuchsia.com/template_driver#meta/template.cm
1 Properties
[ 1/  1] : Key "gizmo.example.TEST_NODE_ID"   Value "template"
0 Offers
```

## Testing

This template has unit tests that can be run in-tree and OOT

### In-Tree Testing
`fx bazel` does not support running Bazel based tests, to run tests in-tree the test targets are wrapped in gn as a bazel-fuchsia-test-package, and executed with fx test e.g.

1) Start the emulator
`ffx emu start --headless`

2) Start a package server
`fx serve`

3) Run the test
`fx test //examples/drivers/template:template-test`

### Out-of-tree Testing
To get an SDK provisioned out of tree repository, make a request for a new Fuchsia managed repository to Infra.
See go/fuchsia-oot-driver-migration for further instructions.
From a provisioned repository, bazel can run the Fuchsia test workflows. Tests can be invoked with:

1) Start the emulator
`ffx emu start --headless`

2) Run the test
` bazel test //examples/drivers/template:template-test --test_output=streamed`

or

`bazel run //examples/drivers/template:template-test`

Note: The Fuchsia Bazel workflows set up an server automatically, having `fx serve` running will occupy the package server port and cause the test to fail.

## Source layout

The core implementation is in  `template_driver.cc`.
Unit test implementation is in `/tests/tests.cc`.

