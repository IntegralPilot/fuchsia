# ICU library utilities for Fuchsia

## Templates for ICU deployment on Fuchsia

The *.gni templates are shims around templates with the same name, with
`icu_` prepended.

These are used to produce artifacts that use different commit IDs for the  ICU
library for product assembly.

For example, `icu_source_set` is a `source_set` target that produces 3 flavors
of each target it is instantiated for.

For example, `icu_source_set("foo")` produces several targets named
`foo.icu_XXXX`, where `XXXX` is replaced by the commit ID of the ICU library
used in its compilation.

## Why do we do this?

Since the choice of the ICU library is made at compile time, but the choice of
components for a given product is made at product assembly time, we must offer
the product assembly "flavors" of the same components, built with different
ICU commit IDs. Product assembly can then use these "flavored" builds to make
the correct selection of a component.

