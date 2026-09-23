# The zither Syscall Documentation Backend

The zither syscall documentation backend is responsible for generating the
official syscall documentation markdown found at
[//docs/reference/syscalls](/docs/reference/syscalls/).

TODO(https://fxbug.dev/42061412): Once we have a first-class representation of syscalls
in FIDL, this backend should be retired in favour of `fidldoc` support.

## Output layout

A `${syscall_name}.md` page is generated per syscall, as well as
`syscall_docs.gni` containing a GN list, `syscall_docs`, of all such pages.

## GN integration

`${fidl_target}_zither.syscall_docs` generates these files, which are intended
to be accessed via `zither_golden_files()` in order to be checked in as source.
The contents of the generated `syscall_docs.gni` in turn informs the generation
logic of all expected outputs.

## Bindings

Any declaration type not mentioned below is ignored.

### Syscalls

Each syscall declaration yields one documentation page and one entry in
`syscall_docs`.
