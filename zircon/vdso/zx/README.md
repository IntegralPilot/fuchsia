# zx FIDL library

The library imported by `using zx;` statements in `.fidl` files.

This library is the subset of syscall interface (e.g., its data types) that is
modelable in FIDL today. This library is not to be confused with
[`//zircon/vdso`](/zircon/vdso/), which describes the complement, representing
a strict superset of this one (using experimental FIDL). As
[described there](/zircon/vdso/README.md), more of that library will eventually
be migrated into this one as it becomes possible to model new constructs.
