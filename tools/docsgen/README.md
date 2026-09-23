Docsgen
===========

The present tool generates reference documentation without the usage of recipes.
Currently the following generation is supported.

- bazel-docgen
- Clidoc
- CMLdoc
- Dart
- Driversdoc
- FIDL
- Helpdoc
- Syscalls

## Usage

Docsgen can be built using the command `fx build tools/docsgen`

For example, you can set it to a minimal configuration:

```
fx set minimal.x64 --with //tools/docsgen
```

Then, build `docsgen`:

```
fx build tools/docsgen
```

You can then see the built documentation in the `out/default/obj/tools/docsgen`
directory of your Fuchsia checkout.
