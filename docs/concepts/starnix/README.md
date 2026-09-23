# Starnix

Starnix is a [runner][starnix-runner] that allows unmodified Linux programs to run on
a Fuchsia system. Linux programs on Fuchsia are run in a user-space process whose
system interface is compatible with the Linux ABI (application binary interface).
Instead of using the Linux kernel to implement this interface, Fuchsia implements the
interface in a Fuchsia user-space program called Starnix. Starnix then serves as
a compatibility layer that translates requests (syscalls) from Linux programs to the
appropriate Fuchsia subsystems.

## Table of contents

- [**Starnix Kernel**][starnix-kernel]: Describes the design of the Starnix
  kernel.
- [**Making Linux syscalls in Fuchsia**][making-linux-syscalls]: Describes how Starnix
  handles Linux system calls in Fuchsia.
- [**Starnix container**][starnix-container]: Explains how Starnix containers
  enable Linux binaries to run in different Linux environments.
- [**Architecture of the Starnix VFS**][starnix-vfs]: Explains the key concepts and
  structure of Starnix's virtual file system (VFS).

## Additional resources

- For detailed information on Starnix's design, see
  [RFC-0082: Running unmodified Linux programs on Fuchsia][starnix-rfc].
- For instructions on running Linux programs, tests, and interactive consoles,
  see Starnix's [`README.md`][starnix-readme] file.
- For standard coding patterns and best practices, see the
  [Starnix development][starnix-development] section.

<!-- Reference links -->

[starnix-runner]: /docs/concepts/components/v2/starnix.md
[starnix-rfc]: /docs/contribute/governance/rfcs/0082_starnix.md
[starnix-kernel]: /docs/concepts/starnix/kernel.md
[starnix-readme]: https://cs.opensource.google/fuchsia/fuchsia/+/main:/src/starnix/kernel/README.md
[making-linux-syscalls]: /docs/concepts/starnix/syscalls.md
[starnix-container]: /docs/concepts/starnix/containers.md
[starnix-vfs]: /docs/concepts/starnix/vfs.md
[starnix-development]: /docs/development/starnix/common_coding_patterns_in_starnix.md
