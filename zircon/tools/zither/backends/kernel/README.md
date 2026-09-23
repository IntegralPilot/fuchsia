# The Zither Kernel Backend

The Zither kernel backend is responsible for generating syscall-related glue
logic for the kernel and vDSO, derived from the FIDL syscall specifications of
library `zx`.

## Output layout

Most generated files are .inc files with legacy 'macro template' APIs, allowing
for further, flexible, generation logic:
* `<lib/syscalls/category.inc>`: a categorization of syscalls with macro
  helpers;
* `<lib/syscalls/kernel-wrappers.inc>`: kernel-side boilerplate for
  syscall definitions;
* `<lib/syscalls/kernel.inc>`: macro-templated enumeration of syscalls
  from the kernel's point of view;
* `<lib/syscalls/syscalls.inc>`: macro-templated enumeration of syscalls
  from userspace's point of view;
* `<lib/syscalls/zx-syscall-numbers.h>`: `#define`s of syscall numbers.

## GN integration

`${fidl_target}_zither.kernel` generates a C++ library target aggregating the
generated sources, and giving the headers and .inc files as public
dependencies.

## Bindings

Any declaration type not mentioned below is ignored.

### Syscalls

Each syscall declaration yields one 'entry' in each of the generated files.
