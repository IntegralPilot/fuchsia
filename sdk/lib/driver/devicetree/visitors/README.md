# Devicetree Visitors

Devicetree visitors are objects that implement the interfaces in `visitor.h`. They are responsible
to parse the relevant devicetree nodes with respect to a particular devicetree binding and produce
either driver metadata and/or driver framework node properties. All |Visitor::Visit| calls are
invoked during the devicetree manager |Walk| call.

## Default and driver visitors

Visitors for properties/bindings that are mentioned as standard properties in the devicetree
specification will be added under the default visitors. Additionally visitors for properties that
are generic in Fuchsia platform i.e. properties explicitly mentioned in
|fuchsia_hardware_platform_bus::Node| are considered default for Fuchsia and are added to the
default visitor set.

Visitors corresponding to driver specific metadata or dependencies are built as shared libraries
using the `devicetree_visitor` GN target. Creating a shared library of the visitor helps to keep the
list of visitors dynamic i.e. visitors can be added and removed from the board driver without having
to recompile it. This also helps to update and contribute visitors independent of the board driver.

## Writing new visitors

A new visitor will be needed when a new devicetree binding is created. And typically a new
devicetree binding is created either because a new metadata was introduced, and/or a node
needs to be created from the board driver.

`fx create devicetree visitor --lang cpp --path <visitor path>` can be used as a starting point for
writing a new visitor.

All visitors should include a devicetree schema file representing the bindings that it is parsing.

## Helper libraries

### Driver visitors

This library provides a constructor that takes in a list of compatible strings and only calls the
visitor when the node with the matching compatible string is found.

### Property parser

This library provides a parser object which can be configured to parse all relevant properties for a
node. It also reduces some of the complexity involved around parsing phandle references.

### Multi-visitor

Used for combining multiple visitors in to a single object that can be passed to the devicetree
manager |Walk| call.

### Load Visitors

This can be used by the board driver to load shared library visitors packaged with the driver in
it's `/lib/visitors` folder.

## Testing

A visitor integration test can be created using `visitor-test-helper` to test the parsing and
creation of metadata and bind rules by the visitor. The helper library creates an instance of
devicetree manager along with fakes for platform-bus protocols. The DTB containing the test nodes
for the visitor is passed during the initialization of the test helper. All of the properties set by
the visitor (i.e. metadata and bind rules/properties) are recorded by the test helper and can be
used to verify the visitor behavior.
