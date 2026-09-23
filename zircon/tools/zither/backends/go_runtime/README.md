# The Zither Go Runtime Backend

The Zither Go runtime backend is responsible for generating the go.git sources
defining the language's support on Fuchsia, derived from the FIDL syscall
specifications of library `zx`.

## Output layout

There are two sets of go.git sources generated:
* Private syscall bindings, for the `runtime` package:
  - `src/runtime/vdso_keys_fuchsia.go`
  - `src/runtime/vdsocalls_fuchsia_${ARCH}.s`
* Public syscall bindings defined as jumps to the runtime ones:
  - `src/syscall/zx/syscalls_fuchsia.go`
  - `src/syscall/zx/syscalls_fuchsia_${ARCH}.s`

This organization is due to the fact that the `runtime` package needs to make
use of Fuchsia's syscalls, but the latter are usually expected to be defined in
`syscall`, while `syscall/zx` depends on `runtime`.

## GN integration

`${fidl_target}_zither.go_runtime` generates the set of sources, but this set
is only meant to be accessed in GN - via `zither_golden_files()` - for
testing. To properly generate sources for go.git, `zither` is expected to be
invoked directly.

## Bindings

Any declaration type not mentioned below is ignored.

### Syscalls

Each syscall declaration yields one private FFI binding in the `runtime`
package, `vdsoCall_zx_foo_bar()`, and one public one in `syscall/zx`,
`Sys_foo_bar()`.
