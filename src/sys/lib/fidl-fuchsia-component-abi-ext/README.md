# A FIDL ABI Revision Parsing Library for In-Tree Packaged Component Resolvers

This library provides a means for Component Resolvers serving the `fuchsia.component.resolution.Resolver`
protocol to read and parse a `fuchsia.version.AbiRevision` value from a package directory proxy.

The Component Resolver may include the parsed result in the `fuchsia.component.resolution.Component`
returned to Component Manager.

The abi revision is read from the `AbiRevision::PATH` established by [RFC-0135](/docs/contribute/governance/rfcs/0135_package_abi_revision) and defined in the [version-history library](/src/lib/versioning/version-history/rust/src/lib.rs).
