# `cm_rust` library

This library contains common component manager representations of the concepts
that appear in component manifests (`.cml` files and binary `.cm` files).

When converting from `fuchsia.component.decl` FIDL data types, `cm_rust` will
perform validation using [`cm_fidl_validator`][cm_fidl_validator]. The native
Rust representations in `cm_rust` can be more ergonomic and has stronger
guarantees than the equivalents generated from FIDL. For example, some table
fields are no longer optional after validation and converted to `cm_rust` types.

## Relationship to `.cml`

`.cml` files go through a series of transformations before they are consumed by
component manager at runtime.

- JSON5 `.cml` files are transformed by `cmc` into `.cm` files, a
  `fuchsia.component.decl/Component` FIDL object stored in FIDL persistence
  convention:
  - It applies some local validation on the `.cml` content.
  - It uses `cm_fidl_validator` to validate the resulting
    [`Component`][fdecl-component] declaration.
  - This step does not involve `cm_rust`.
- When instantiating a component at runtime, component manager will transform
  the `fuchsia.component.decl/Component` FIDL into the native Rust
  representation as defined in `cm_rust`:
  - It uses `cm_rust` to transform the Component Rust FIDL binding type into a
    native Rust representation.
  - In doing so, it uses `cm_fidl_validator` to validate the `Component`
    declaration. This may seem duplicative but is necessary because we should
    not trust that the `.cm` file was properly validated.
  - Most other business logic is based on the `cm_rust` types.

Each of these stages perform some level of validation, which includes validating
things like entity names, paths, URLs, etc. All of these types require the same
validation and should be represented by the types in this library.

When adding a basic, common type to the CML syntax, consider whether that type
should be added here, so that every stage of the transformation pipeline can
benefit.

## `serde` integration

When built for host, these types come with `serde` serialization and
deserialization implementations that perform the required validation. `serde` is
not supported on device builds.

[cm_fidl_validator]: /src/sys/lib/cm_fidl_validator/
[fdecl-component]: /sdk/fidl/fuchsia.component.decl/component.fidl
