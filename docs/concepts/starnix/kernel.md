# Starnix kernel

The Starnix kernel implements the Linux Userspace API (UAPI) on Fuchsia. The
Starnix kernel [intercepts syscalls][starnix-syscalls] from Linux processes and
manages the required semantics to execute Linux programs correctly. This
document outlines the internal structure of the Starnix kernel.

## Approach

Starnix aims to run unmodified Linux binaries. To execute these binaries as-is,
Starnix maintains bug-for-bug compatibility with the Linux kernel. This
high-fidelity interoperability relies on extensive testing.

To understand Linux UAPI semantics, tests probe the edge and corner cases of
the interface. Once these tests pass on the Linux kernel, they run in Fuchsia's
continuous integration infrastructure, initially marked as failing on Starnix.
As Starnix improves, these tests eventually pass, allowing them to be marked as
passing.

### Unit versus userspace tests

In most cases, [tests for Starnix are userspace programs][starnix-syscall-tests]
compiled as Linux binaries. This approach allows the same tests to run against
both Starnix and the Linux kernel, ensuring identical behavior.

In some cases, unit tests run inside the Starnix kernel. These tests depend on
implementation details not exposed through the UAPI. While this allows testing
internal logic, it cannot guarantee that the behavior matches the Linux kernel.
Additionally, these tests often require more maintenance as the implementation
evolves. However, this approach is useful for scenarios that are easier to test
with access to kernel internals.

In other cases, integration tests run userspace programs with assertions on the
Fuchsia side. These tests verify invariants not easily accessible from
userspace.

### The track_stub! macro

The Linux UAPI semantics are vast. An individual syscall or pseudofile may have
more functionality than is currently implemented. The `track_stub!` macro
tracks unimplemented semantics, documenting missing code paths and linking them
to issue tracker bugs. It also instruments the Starnix kernel binary to observe
when Linux programs trigger these paths.

Conceptually, the original Starnix kernel implementation was a `match` statement
on the syscall number, where every syscall called the `track_stub!` macro. As
support for more sophisticated Linux programs grew, actual implementations
replaced these macros. For syscalls with multiple options or modes,
`track_stub!` is pushed inside the function to "implement" missing options.

Internally, there is a [dashboard][starnix-not-implemented] tracks statistics on
how often and in what scenarios the `track_stub!` macro executes. As Starnix
functionality expands, the `track_stub!` macro continues to track progress.

## Structure

The Starnix kernel consists of several Rust crates. The Starnix kernel itself is
a crate that contains `main.rs`, the main entry point. The core machinery is in
the `starnix_core` crate, located in the middle of the dependency graph.

### Process model

The Starnix kernel runs as a collection of Fuchsia processes within a job. There
is one Fuchsia process for every Linux address space (conceptually a Linux
process), plus one additional *initial* Fuchsia process. This additional process
is where the Starnix kernel binary is loaded and begins executing. This process
contains the [Starnix shared address space][shared-address-space] but not a
[restricted address space][restricted-address-space].

The initial process' main thread runs a standard Fuchsia async executor to
handle FIDL requests, such as requests to run a [Starnix
container][starnix-container]. This process also contains background threads,
called *kthreads*, which run kernel background tasks. Running these threads
in the initial process ensures they outlive the userspace processes that may
have triggered their creation.

Warning: Do not use standard Rust facilities (like `std::thread::spawn`) to
spawn threads. Instead, use `kernel.kthreads` to spawn a kthread in the initial
process, ensuring it persists independently of other processes. For more
information, see [Spawning Threads in Starnix][spawning-threads-in-starnix].

### `starnix_syscall_loop` crate

Upon creation, userspace threads enter the main *syscall loop*, implemented by
the [`starnix_syscall_loop` crate][starnix-syscall-loop]. In this loop, the
thread enters user mode (i.e. restricted mode) with a specific machine state.
Control of the thread returns to the Starnix kernel when the Linux program exits
user mode, typically due to a syscall, exception, or being *kicked* back to
kernel mode.

When a Linux program issues a syscall, the `dispatch_syscall` function in the
`starnix_syscall_loop` crate decodes the syscall and invokes the corresponding
syscall implementation function. Separating `dispatch_syscall` from the syscall
implementations allows sharding implementations across multiple crates.
Currently, most implementations reside in `starnix_core`, but
[plans exist][bug-470456509] to move them to reduce complexity.

### Modules

Many Starnix kernel features are implemented as [*modules*][starnix-modules].
During initialization, the Starnix kernel only initializes modules that are
enabled through a corresponding feature flag. Modules typically register
themselves with `starnix_core`. For example, a device module registers as the
handler for specific device numbers with the `DeviceRegistry`, while a file
system module registers with the `FsRegistry`.

Modules are not called directly by `starnix_core`. Instead, they implement
traits corresponding to the abstractions they provide. For example, device
modules implement the `DeviceOps` trait, while file system modules implement
the `FileSystemOps` trait. These traits often return objects implementing other
traits, such as `FileOps` and `FsNodeOps`.

Modules requiring kernel-global state should use the `kernel.expando` object
instead of defining fields on the `Kernel` struct. The `kernel.expando` is keyed
by Rust type, which allows modules to define unique storage slots without
collision risk. This mechanism also avoids resource allocation for unused
modules.

### `starnix_core` crate

The [`starnix_core` crate][starnix-core] contains the Starnix kernel's core
machinery, including tasks, memory management, device registration, and the
virtual file system (VFS). These tightly interrelated subsystems have many
circular dependencies. Most of the design of `starnix_core` is documented with
rustdoc comments in the source code.

### Libraries

Code required by `starnix_core`, modules, or other components, but independent
of `starnix_core`, should reside in a separate crate within the
[`//src/starnix/lib` directory][starnix-lib]. Separate crates clarify the
code's dependency graph and improves incremental build times, as changes to
`starnix_core` do not trigger rebuilds of these independent libraries.

### UAPI

The [`starnix_uapi` crate][starnix-uapi] is at the bottom of the Starnix
kernel dependency graph. It defines ergonomic Rust types for Linux UAPI
concepts. For example, where the Linux UAPI defines a `u32` with specific bit
semantics, `starnix_uapi` might use the `bitflags!` macro to define a
corresponding Rust type.

The `starnix_uapi` crate also defines the `UserAddress` type, which represents a
userspace address. The Starnix kernel uses `UserAddress` instead of Rust
pointers for userspace memory to prevent accidental dereferencing. This
mitigates two hazards:

* Userspace supplying a kernel address to manipulate kernel memory.
* Kernel panicking when accessing userspace addresses without using the safe
  `usercopy` machinery.

The `starnix_uapi` crate depends on `linux_uapi`, a crate which is automatically
generated with `bindgen` from the C definitions of the Linux UAPI used by Linux
programs.

<!-- Reference links -->

[starnix-core]: /src/starnix/kernel/core/
[starnix-lib]: /src/starnix/lib/
[starnix-syscalls]: /docs/concepts/starnix/syscalls.md
[starnix-syscall-tests]: /src/starnix/tests/syscalls/cpp/
[starnix-syscall-loop]: /src/starnix/kernel/syscall_loop/
[starnix-uapi]: /src/starnix/lib/starnix_uapi/
[starnix-vfs]: /docs/concepts/starnix/vfs.md
[starnix-container]: /docs/concepts/starnix/containers.md
[starnix-not-implemented]: http://goto.google.com/starnix-not-implemented
[starnix-modules]: /src/starnix/modules/
[bug-470456509]: https://fxbug.dev/470456509
[shared-address-space]: /docs/concepts/starnix/syscalls.md#shared-starnix-instance
[restricted-address-space]: /docs/concepts/starnix/syscalls.md#running-a-linux-program-in-restricted-mode
[spawning-threads-in-starnix]:/docs/development/starnix/common_coding_patterns_in_starnix.md#spawning-threads-in-starnix
