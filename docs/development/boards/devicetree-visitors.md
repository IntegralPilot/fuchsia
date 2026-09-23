# Devicetree visitors

**Devicetree visitors** are responsible for converting devicetree data into
driver-specific metadata and bind rules. They parse the relevant devicetree
nodes with respect to a particular [devicetree binding][devicetree-binding]
and produce driver metadata and driver framework node properties. Devicetree
visitor objects implement the interfaces in the
[`driver-visitor.h`][driver-visitor-h] header file and all `Visitor::Visit`
calls are invoked during the devicetree manager's `Walk` call.

## Visitor types {:#visitor-types}

There are two types of devicetree visitors:
[default visitors](#default-visitors) and [driver visitors](#driver-visitors).

### Default visitors {:#default-visitors}

[Default visitors][default-visitors] are for the properties or bind rules
mentioned as standard properties in the devicetree specification. Additionally,
a number of visitors are for the properties that are generic in the Fuchsia
platform (that is, the properties explicitly mentioned in the
`fuchsia_hardware_platform_bus::Node` protocol). These visitors are considered
default for Fuchsia and are also added to the default visitor set.

### Driver visitors {:#driver-visitors}

[Driver visitors][driver-visitors] correspond to driver-specific metadata or
dependencies. These visitors are built as shared libraries using the
`devicetree_visitor` GN target. Creating a shared library of the visitor helps
Fuchsia keep the list of visitors dynamic, that is, visitors can be added and
removed from the [board driver][board-driver] without having to recompile it.
This also helps Fuchsia update and contribute visitors independent of the board
driver.

## Writing a new visitor {:#writing-a-new-visitor}

A new visitor will be needed when a new devicetree
[binding][bindings]{:.external} is created. Typically, a new devicetree binding
is created because either a new metadata is introduced or a composite node
needs to be created from the board driver.

You can use the following `fx` command as a starting point for writing a new
visitor:

```posix-terminal
fx create devicetree visitor --lang cpp --path <VISITOR_PATH>
```

All visitors must include a devicetree schema file (for example, see
[`smc.yaml`][smc-yaml]) representing the bindings that it is parsing.
For a complete devicetree visitor example, see the
[`example-visitor`][example-visitor] directory.

### Helper libraries {:#helper-libraries}

The following helper libraries are available for writing a new visitor:

- **Driver visitors**: This library provides a constructor that takes in a list
  of compatible strings and only calls the visitor when the node with the
  matching compatible string is found.
- **Property parser**: This library provides a parser object which can be
  configured to parse all relevant properties for a node. It also reduces some
  of the complexity involved around parsing [phandle][phandle]{:.external}
  (pointer handle) references.
- **Multi-visitor**: Used for combining multiple visitors into a single object
  that can be passed to the devicetree manager's `Walk` call.
- **Load visitors**: This can be used by the board driver to load shared library
  visitors packaged with the driver in its `/lib/visitors` folder.

## Testing a visitor {:#testing-visitor}

A visitor integration test can be created using the `visitor-test-helper`
library to test the parsing and creation of metadata and bind rules by the
visitor. The helper library creates an instance of the devicetree manager along
with fakes for platform-bus protocols. The DTB containing the test nodes for the
visitor is passed during the initialization of the test helper. All of the
properties set by the visitor (that is, metadata and properties or bind rules)
are recorded by the test helper and can be used to verify the visitor's
behavior.

<!-- Reference links -->

[smc-yaml]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/devicetree/visitors/default/smc/smc.yaml
[driver-visitor-h]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/devicetree/visitors/driver-visitor.h
[bindings]: https://devicetree-specification.readthedocs.io/en/v0.3/device-bindings.html#device-bindings
[example-visitor]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/devicetree/examples/example-visitor/
[phandle]: https://devicetree-specification.readthedocs.io/en/v0.3/devicetree-basics.html#phandle
[devicetree-binding]: /docs/development/boards/devicetree-overview.md#devicetree-bindings-and-visitors
[board-driver]: /docs/development/boards/bringup.md#board-driver
[default-visitors]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/devicetree/visitors/default/
[driver-visitors]: https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/lib/driver/devicetree/visitors/drivers/
