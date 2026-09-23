# Fuchsia Portability Shims for `fsck-msdosfs`

This directory (`sys/`) contains lightweight compatibility header shims required to compile the
upstream FreeBSD `fsck_msdosfs` C source code in the Fuchsia build environment.

## Purpose

Upstream FreeBSD `fsck_msdosfs` relies on standard BSD system headers (`<sys/cdefs.h>`,
`<sys/endian.h>`, `<sys/limits.h>`, `<sys/queue.h>`) that are not present in the Fuchsia C
sysroot.

By providing these minimal local headers under `sys/` and adding `include_dirs = [ "." ]` to
[`BUILD.gn`](../BUILD.gn), upstream FreeBSD `.c` files can be imported and compiled with zero
modifications to their `#include` statements.

## Included Headers

- **`cdefs.h`**: Portability definitions for BSD-specific macros (`__RCSID`, `__FBSDID`,
  `__dead2`, etc.) and BSD integer types (`u_int`, `u_int64_t`, etc.).
- **`endian.h`**: Endianness decoding/encoding helpers (`le16dec`, `le16enc`, `le32dec`,
  `le32enc`).
- **`limits.h`**: Forwards to standard `<limits.h>`.
- **`queue.h`**: Includes [`//third_party/sbase/queue.h`](../../../third_party/sbase/queue.h) for
  standard BSD queue macros (`TAILQ_*`).
