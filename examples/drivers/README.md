# Driver Example Index

This is an index of the examples intended to demonstrate driver concepts.

## Basic examples

Examples that showcase common driver concepts.

### Skeleton driver

The [skeleton driver example](/examples/drivers/skeleton/) is a minimal driver written in DFv2.

### Template driver

[Template driver](/examples/drivers/template/) is a minimal driver written in DFv2, and built with SDK rules, which can be used either in-tree or out-of-tree.

### Simple driver

The [simple driver examples](/examples/drivers/simple/) showcases how to write a [DFv1 driver](/examples/drivers/simple/dfv1/) and a [DFv2 driver](/examples/drivers/simple/dfv2/) with common patterns:

*   Adding logs
*   Adding a child node
*   Implementing stop functions
*   Setting up a compat device server

### Metadata

The [metadata example](/examples/drivers/metadata) demonstrates how to define metadata types and pass metadata from a driver to its children.

## Transport examples

Examples that showcase communication between drivers.

### Banjo

[DFv1](/examples/drivers/transport/banjo/v1/) and [DFv2](/examples/drivers/transport/banjo/v2) driver examples that demonstrate a parent driver serving a Banjo transport and a child driver that connects and queries data from it.

### Zircon transport

[DFv1](/examples/drivers/transport/zircon/v1/) and [DFv2](/examples/drivers/transport/zircon/v2) driver examples that demonstrate a parent driver serving a FIDL protocol over zircon transport and a child driver that connects and queries data from it.


### Driver transport

[DFv1](/examples/drivers/transport/driver/v1/) and [DFv2](/examples/drivers/transport/driver/v2) driver examples that demonstrate a parent driver serving a FIDL protocol over zircon transport and a child driver that connects and queries data from it.


## Bind examples


### Bind library

This [example](/examples/drivers/bind/bindlib/) demonstrates how to write a bind library.


### Bind library codegen

This [example](/examples/drivers/bind/bindlib_codegen/) demonstrates how to use the C++ and Rust constants generated from a bind library.


### FIDL bind library codegen

This [example](/examples/drivers/bind/bindlib_codegen/) demonstrates how to write a bind library.

## Test examples

### Unit tests

Here are various unit tests in the examples

*   [Simple driver](examples/drivers/simple/dfv2/tests/test.cc) - barebones unit test for the simple driver example
*   [Banjo transport](/examples/drivers/transport/banjo/v2/tests/) - unit test that verifies that the child driver can query from a fake banjo server
*   [Zircon transport](/examples/drivers/transport/zircon/v2/tests/) - unit test that verifies that the child driver can query from a fake FIDL zircon server
*   [Driver transport](/examples/drivers/transport/driver/v2/tests/) - unit test that verifies that the child driver can query from a fake FIDL driver server

### Integration tests

The [Driver Test Realm examples](/examples/drivers/driver_test_realm/) demonstrate how to write hermetic and non-hermetic integration tests for a driver.
