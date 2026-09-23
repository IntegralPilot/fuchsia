# Starnix Modules

Starnix modules are crates that are statically linked into the Starnix kernel and provide
self-contained functionality.

## Dependencies

Starnix modules can depend on `starnix_core` but `starnix_core` cannot depend on any modules.
A Starnix module cannot depend on another Starnix module. If you need to share code between
two modules, please either put that code in `//src/starnix/lib`, if the code does not need to
depend on `starnix_core`, or in `//src/starnix/kernel`, if the code does need to depend on
`starnix_core`.

## Use cases

Two common use cases for Starnix modules are device drivers and file systems.

## Configuration

To ensure code quality and consistency across all Starnix modules, each
module's `BUILD.gn` file must include the common clippy lint configuration and
other compiler flags.

This is done by adding the following to the `configs` list of your
`rustc_library` target:

```gn
  configs += [ "//src/starnix/build:kernel_library_config" ]
```

This configuration enforces a standard set of lints and aligns other flags with
the library's use in the Starnix kernel binary.
