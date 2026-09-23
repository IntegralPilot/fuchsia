# Userboot library

The kernel itself directly launches one program in one user process at boot.
That first process, and the program running in it, are called "userboot".

This library facilitates writing userboot programs.  It supports writing
specific userboot programs to do whatever you want them to do.  The library is
not much involved in _what_ userboot _does_.  It takes care of _how_ it can do
_anything_ in the special circumstances of _what_ and _where_ userboot _is_.

## Kernel storage packing

The userboot program is a single ELF file.  Unlike all other user-space
programs stored in a bootable ZBI, userboot is packed into the `STORAGE_KERNEL`
ZBI item along with kernel binaries as part of the kernel ZBI.  Normal userland
is instead packed into the `STORAGE_BOOTFS` ZBI item that's combined with the
kernel ZBI during the product assembly phase.  (It's userboot itself that's
responsible for unpacking the `BOOTFS` item, so it can't be inside there!)

The file is found inside the "kernel package" selected by the `kernel.select`
boot option.  In the build system, this means its `executable()` should be in
`deps` of a [`kernel_package()`](/zircon/kernel/kernel_package.gni).  Then that
goes into `deps` of a [`kernel_image()`](/zircon/kernel/kernel_image.gni).

The `kernel.select.userboot` boot option sets the file name to be found
_inside_ the selected kernel package; by default it's just `userboot`, but it
can be any relative path matching the `distribution_entries` metadata used in a
target that went into `deps` of `kernel_package()`.

## Kernel program loading

The kernel loads userboot as a mostly-normal ELF file, with some restrictions.

 * It must be a self-contained, statically linked executable (a static PIE): it
   cannot use a `PT_INTERP` (separate dynamic linker), nor have any `DT_NEEDED`
   dependencies other than the vDSO.

 * It must not have `PT_LOAD` segments that overlap in the file.  This is a
   constraint on the link-time layout that is ordinarily met by binaries linked
   for Fuchsia, but is required by neither ELF nor the system program loader.

## Kernel bootstrap protocol

Like any Zircon process, userboot is started with a single handle from which it
bootstraps all other capabilities and information it needs.  The kernel passes
a channel handle on which it sends two messages (not necessarily queued yet
when the process starts).  Each message contains only handles.  There is no
additional data about the handles, and no specified order or number of handles
in each message.  Each handle's purpose can be identified from its object type
and the details available from queries via that handle (`zx_object_get_info`,
etc.).  The distinction between these two messages is reflected in the
structure of the library.

### Process capability message

The first message contains the few essential handles describing the userboot
process itself.  The userboot library provides custom startup code for libc
that uses this message.  The message has already been read to initialize
library state before any C++ static constructors or the userboot program's
`main` function run.

This message is kept separate precisely because of this topical separation:
it's just about bootstrapping the _userboot program_.  The userboot library
handles this part completely.

### System capability message

The second and final message contains all the handles that constitute
system-wide privilege:

 * The root job handle.
 * Resource handles, identified by resource kind.
   * This includes the root resource and some others.
   * All other resource objects are made from these in userland.
 * VMO handles, identified by name.
   * `zbi`: The ZBI, where `CMDLINE`, `STORAGE_BOOTFS`, etc. are found.
   * `vdso/*`: The various vDSO images as ELF in VMOs blessed to make syscalls.
   * Various others to be made available to userland as `/kernel/...` files.

The library does not consume this message.  Instead, the library's [startup
support API](#startup-support-api) hands the channel off to the program after
reading the process capability.  It's entirely up to the program to read and
decode the system capability message and bootstrap _the whole system_.  The
library doesn't impose any data structures to represent this, nor do any
allocation to hold the message or its representation.  If the details of this
message change in the future, that will be between the kernel and userboot
programs, without changes to the userboot library or its APIs.

## Library features

This library facilitates writing a userboot program.  In the build system,
`deps` on the [userboot](BUILD.gn) library automatically propagate the use of
pure static linking via the [`//sdk/lib/c:static`](/sdk/lib/c/BUILD.gn) target.
(Note the `userboot` library target must be in the _direct_ `deps` of the
`executable()` GN target, or treated as such via `public_deps` propagation.
Otherwise, it may be necessary to add a direct dependency on `...:static`.)

### Process startup support

The userboot library and the C library together take care of process startup so
that things look somewhat "normal" to the userboot program.  The usual `main`
function is called as in any other program with the canonical signatures
available.  However, there are never any arguments or environment (such that
`argc == 0`).  So there's no reason to use a `main` signature that takes any
arguments.

When `main` returns it will do the normal things just as when `exit` is called:
run any destructors or `atexit` hooks, etc; then call `zx_process_exit` with
the exit code (return value from `main` or argument to `exit`).  However,
nothing ever notices the exit code.  If userboot crashes rather than exiting,
the kernel will print exception details with a register dump, etc.  But if
userboot exits intentionally, the kernel considers this "normal":
 * If any other processes are running, then great!  Userboot's job is done.
 * If the root job ever becomes empty because no process is running anymore,
   the system always just reboots or shuts it down.

In short, the main "startup" support is just that the C library pretty much all
works normally and is available to be used in normal ways.  (The main caveats
are just the context: there are no other processes on the system to provide any
services; and only libraries that support full static linking can be used, e.g.
there is no fdio---as well as nothing for it to talk to.)  `main` should return
(or lead to calling `exit` or `_exit`, etc.) either when it's done and other
things are running now; or with no other process running when in a bad panic or
minimal testing scenario without proper drivers to manage full-system shutdown.

### Startup support API

[`<lib/userboot/startup.h>`](include/lib/userboot/startup.h) defines what
little API there is for startup support per se: The single function
`TakeBootstrapChannel()` returns the startup channel handle, transferring
ownership to the caller.  That channel is expected to get the system capability
message from the kernel, and no further messages (the kernel closes its end
after sending the message).  There is no guarantee that the message has already
been sent or the peer (kernel) side closed by the time the channel is handed
over.  The channel signals must be waited for as usual.

This simple API uses C linkage and a trivial signature so it can be used easily
from C, C++, or Rust or other languages with even minimal C interoperation.

### Additional library utilities

So far the library has no additional utility APIs.  If there are common pieces
that should be reused across different userboot programs but are very specific
to the niche of userboot work, this is a natural place to add them.

Other libraries not specific to the userboot context support ZBI decoding,
decompression, BOOTFS format, process management, `fuchsia.ldsvc` protocol
implementation, ELF loading, etc.

## Testing support library

One big feature of userboot programs being "somewhat normal" is that they can
be tested and debugged somewhat normally as well.  Aside from the special
properties and privileges of particular resource objects, a userboot program
running in a normal sandbox without special privileges works much the same as
if it had been launched by the kernel at boot time.  It's not alone on the
system.  But it only knows about the task hierarchy under the job it's given as
"the root job".  It only knows what resource and VMO handles it's given and
what queries on those handles report about them, not what kernel or hardware
privilege they truly confer.

The [userboot-testing](testing) library provides support for writing Fuchsia
test components with [gtest](/third_party/googletest) that exercise userboot
programs in sandbox environments.

The [API](testing/include/lib/userboot/testing/launcher.h) provides for:

 * Fetching a userboot ELF file under test from the test package.

 * Creating an empty job to stand in for the root job for the life of a test.

 * Launching a userboot test process in a sandbox, including:
   * Automatic generation of the process capability message.
   * Passing through a vector of handles as the system capability message.
   * Waiting for the process to terminate.

This allows the test logic to concentrate on the userboot "business logic":
what it expects in the system capability message; and what it does with that.
