# The Zither Rust Syscall Backend

The Zither Rust syscall backend is responsible for generating thin FFI syscall
wrappers, derived from the FIDL syscall specifications of library `zx`.

## Output layout

A crate of name `fuchsia-zircon-sys` is generated, but has no particularly
documented layout.

## GN integration

`${fidl_target}_zither.rust_syscall` generates a `rustc_library()` target
defining the crate.

## Bindings

Any declaration type not mentioned below is ignored.

### Syscalls

Each syscall declaration yields one FFI wrapper in the crate.
