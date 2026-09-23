# The Zither Legacy Syscall C Declaration Backend

The Zither legacy syscall C declaration backend is responsible for generating
several .inc files with macro template APIs used to define the C syscall
declarations in the public Zircon headers.

## Output layout

* `gen/cdecls.inc`: macro-templated enumeration of syscalls meant
  for the public headers
* `gen/testonly-cdecls.inc`: similar to cdecls.inc, but consisting
  of the test-only syscalls.
* `gen/cdecls-next.inc`: similar to cdecls.inc, but consisting
  of the "next" syscalls.

## GN integration

`${fidl_target}_zither.legacy_syscall_cdecl` generates a C++ library target
aggregating the generated sources, and giving the .inc files as public
dependencies.

## Bindings

Any declaration type not mentioned below is ignored.

### Syscalls

Each `@next` syscall declaration yields an entry in `testonly-cdecls.inc`; each
`@testonly` one yields an entry in `cdecls-next.inc`; all remaining ones yield
entries in `cdecls.inc`.
