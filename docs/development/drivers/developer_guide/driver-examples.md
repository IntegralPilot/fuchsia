# Driver examples

This page contains a list of the examples intended to demonstrate Fuchsia
driver concepts.

## Basic examples

These examples showcase common [driver concepts][driver-concepts]:

* **Skeleton driver** - The [skeleton driver example][skeleton-driver] is
  a minimal driver written in DFv2.

* **Template driver** - The [template driver][driver-template] is a minimal
  driver written in DFv2, and built with SDK rules (which can be used in
  both the Fuchsia source checkout and Fuchsia SDK environments).

* **Simple driver** - The [simple driver examples][simple-driver] showcases
  how to write a [DFv1 driver][dfv1-simple-driver] and
  a [DFv2 driver][dfv2-simple-driver] with the following common patterns:

  *   Adding logs
  *   Adding a child node
  *   Implementing stop functions
  *   Setting up a compat device server

* **Metadata** - The [metadata example][driver-metadata] demonstrates how
  to define metadata types and pass metadata from a driver to its children.

## Transport examples

These examples showcase communication between drivers:

* **Banjo** - These examples demonstrate a parent driver serving a Banjo
  transport and a child driver that connects and queries data from it.

  * [DFv1 example][dfv1-banjo-transport]
  * [DFv2 example][dfv2-banjo-transport]

* **Zircon transport** - These examples demonstrate a parent driver serving
  a FIDL protocol over Zircon transport and a child driver that connects
  and queries data from it.

  * [DFv1 example][dfv1-zircon-transport]
  * [DFv2 example][dfv2-zircon-transport]

* **Driver transport** - These examples demonstrate a parent driver serving
  a FIDL protocol over driver transport and a child driver that connects and
  queries data from it.

  * [DFv1 example][dfv1-driver-transport]
  * [DFv2 example][dfv2-driver-transport]

## Bind examples

These examples demonstrate how to write and use bind libraries for drivers:

* **Bind library** - This [example][bindlib] demonstrates how to write a bind library.

* **Bind library codegen** - This [example][bindlib-codegen] demonstrates how to
  use the C++ and Rust constants generated from a bind library.

* **FIDL bind library codegen** - This [example][bindlib-codegen] demonstrates how to
  write a bind library.

## Test examples

These examples demonstrate writing unit and integration tests for drivers.

### Unit tests

Unit test examples:

*  [**Simple driver**][dfv2-test-cc] - Barebones unit test for the simple driver
   example
*  [**Banjo transport**][dfv2-test-banjo] - Unit test that verifies that the child
   driver can query from a fake Banjo server
*  [**Zircon transport**][dfv2-test-zircon] - Unit test that verifies that the
   child driver can query from a fake FIDL zircon server
*  [**Driver transport**][dfv2-test-driver] - Unit test that verifies that the child
   driver can query from a fake FIDL driver server

### Integration tests

The [Driver Test Realm examples][driver-test-realm] demonstrate how to write
hermetic and non-hermetic integration tests for a driver.

<!-- Reference links -->

[driver-concepts]: /docs/concepts/drivers/README.md
[skeleton-driver]: /examples/drivers/skeleton/
[driver-template]: /examples/drivers/template/
[simple-driver]: /examples/drivers/simple/
[dfv1-simple-driver]: /examples/drivers/simple/dfv1/
[dfv2-simple-driver]: /examples/drivers/simple/dfv2/
[driver-metadata]: /examples/drivers/metadata
[dfv1-banjo-transport]: /examples/drivers/transport/banjo/v1/
[dfv2-banjo-transport]: /examples/drivers/transport/banjo/v2/
[dfv1-zircon-transport]: /examples/drivers/transport/zircon/v1/
[dfv2-zircon-transport]: /examples/drivers/transport/zircon/v2/
[dfv1-driver-transport]: /examples/drivers/transport/driver/v1/
[dfv2-driver-transport]: /examples/drivers/transport/driver/v2/
[bindlib]: /examples/drivers/bind/bindlib/
[bindlib-codegen]: /examples/drivers/bind/bindlib_codegen/
[dfv2-test-cc]: /examples/drivers/simple/dfv2/tests/
[dfv2-test-banjo]: /examples/drivers/transport/banjo/v2/tests/
[dfv2-test-zircon]: /examples/drivers/transport/zircon/v2/tests/
[dfv2-test-driver]: /examples/drivers/transport/driver/v2/tests/
[driver-test-realm]: /examples/drivers/driver_test_realm/

