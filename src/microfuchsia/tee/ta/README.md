# TA

This directory contains Trusted Application (TA) implementations and build rules.

## Examples

* noop - All entry points are no-ops

* panic - All entry points panic

## Build rules and linking

TAs are compiled against the TEE Internal API headers in
//src/microfuchsia/tee/tee_internal_api/include.

TAs are linked as shared libraries against the interface definition
in //src/microfuchsia/tee/tee_internal_api/libtee_internal.ifs which enumerates the symbols that
are (intentionally) exported from the runtime.

The symbols exported by the TA itself are further verified to consist of the
official entrypoints (see //src/microfuchsia/tee/ta/abi.ifs).

## Packaging

TAs are included in a Fuchsia package as a regular shared library. The file
'config/ta_name' is also included in this package so that the loader (not yet
implemented) can determine the name of the library from a well-known path. This
is subject to change depending on how we structure the runtime and TA loading.

