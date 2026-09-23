<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0266" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Problem Statement

Starnix needs a way to supply its client programs with a clock whose behavior
matches the Linux definition of `CLOCK_MONOTONIC`.  This clock needs to
represent a monotonic timeline which experiences the same rate adjustments as
the system-wide UTC timeline, but which is also always both monotonic and
continuous when observed by different threads that are able to establish an
ordering of observations through external means.  In other words, these
properties need to be held consistently across an ordered sequence of
observations made by multiple observers.

Note that the Zircon/Fuchsia definition of `CLOCK_MONOTONIC` does not currently
meet all of these requirements, nor are there any plans to make it do so.  While
Zircon's version is consistently monotonic and continuous, it always runs at the
rate of the system's underlying reference clock hardware, and no attempts are
made to rate match to any external reference.

It is expected that Starnix's clients' usage of the Linux `CLOCK_MONOTONIC`
clock will be high, and every effort needs to be made to make any solution as
inexpensive as possible to query.  Complicating matters is the fact that
Starnix's clients are required to operate in
[restricted mode](/docs/contribute/governance/rfcs/0261_fast_and_efficient_user_space_kernel_emulation.md#element_1_restricted_mode),
meaning that they are not allowed to directly make Zircon syscalls. At best, a
Starnix client could exit restricted mode and enter the Starnix kerenel, where
Zircon syscalls are possible.  Note that this is the strategy currently being
used by Starnix to provide access to the system-wide UTC timeline, however the
performance cost of this approach is already quite high.  An ideal solution
would be able to provide access to Linux `CLOCK_MONOTONIC` to Starnix clients,
without requiring that the client exit restricted mode _or_ enter the Zircon
kernel as that overhead has already become effectively too much to bear.

## Summary

Zircon [Kernel Clocks Objects](/reference/kernel_objects/clock) are
already capable of meeting _most_ of the requirements of the Linux version of
`CLOCK_MONOTONIC`.  They provide a way to construct a user-mode defined timeline
which is guaranteed (by the kernel) to be always both monotonic and continuous
when observed by multiple observers, and which can also be delegated to an
authority (in this case, the Timekeeper) for
[maintenance](/reference/kernel_objects/clock#maintaining-a-clock).

This RFC proposes a way to allow a Zircon kernel clock to be read in a fashion
which does not require entering the Zircon kernel, making the read operation's
overhead very inexpensive, provided that the reference clock for the clock
object can also be read without needing to enter the Zircon kernel.
Additionally, this approach to reading the clock object will provide a path for
allowing Starnix clients to read the clock without even needing to exit
restricted mode, giving Starnix the ability to publish *both* Linux
`CLOCK_MONOTONIC` and UTC to their clients without needing to exit restricted
mode.

## Stakeholders

+ Starnix implementers
+ Timekeeper maintainers
+ Kernel maintainers
+ Media maintainers

_Facilitator:_

+ jamesr@google.com

_Reviewers:_

+ rashaeqbal@google.com
+ rudymathu@google.com
+ jamesr@google.com
+ adanis@google.com
+ mpuryear@google.com

_Consulted:_

+ maniscalco@google.com
+ mcgrathr@google.com
+ fmil@google.com

_Socialization:_

This RFC was proposed in an internal design document and was discussed with
stakeholders until it was deemed ready to begin the RPC process.

## Requirements

One of the primary purposes of this proposal is to provide a path for Starnix
to provide a clock representing the Linux version of `CLOCK_MONOTONIC` to its
restricted mode processes.  While there are secondary benefits to being able to
memory map clocks, the requirements stated here primarily come from needing to
satisfy the Starnix and Linux defined requirements.

Memory mapped clocks need to be able to:

+ Meet the Linux requirements for `CLOCK_MONOTONIC`
+ Be subjected to the same rate adjustments as the system-wide UTC clock.
+ Maintain monotonicity and continuity over an ordered sequence of observations
  made by multiple observers.
+ Be queried from a Starnix restricted mode process at an "affordable runtime
  cost".

### Linux requirements for `CLOCK_MONOTONIC`

Per the Linux `clock_gettime`
[man page](https://man7.org/linux/man-pages/man3/clock_gettime.3.html), the
requirements for `CLOCK_MONOTONIC` are as follows.

```
A nonsettable system-wide clock that represents monotonic time since—as
described by POSIX—"some unspecified point in the past".  On Linux, that point
corresponds to the number of seconds that the system has been running since it
was booted.

The CLOCK_MONOTONIC clock is not affected by discontinuous jumps in the system
time (e.g., if the system administrator manually changes the clock), but is
affected by the incremental adjustments performed by adjtime(3) and NTP.  This
clock does not count time that the system is suspended.  All CLOCK_MONOTONIC
variants guarantee  that  the  time returned by consecutive calls will not go
backwards, but successive calls may—depending on the architecture—return
identical (not-increased) time values.
```

### Maintainability

The Linux definition of `CLOCK_MONOTONIC` requires that same rate adjustments
made to maintain the system-wide definition of UTC also be made to the
`CLOCK_MONOTONIC` timeline.  Currently, the clock representing the UTC timeline
for the system is created by the Component Manager and distributed to processes
as duplicated handles (with appropriate permissions) via the process startup
protocol.  A process called the Timekeeper receives a handle to this clock with
write permissions.  It is responsible for maintaining the UTC clock using
available external references to make ongoing corrections to the local
representation of UTC.

Given the Linux requirements stated above, something in the system needs to
apply the same rate adjustments that Timekeeper applies to the system wide UTC
clock, to the version of Linux `CLOCK_MONOTONIC` which Starnix will provide to
its clients.

### Consistency

As noted by the Linux requirements, `CLOCK_MONOTONIC` needs to be both monotonic
and continuous.  So, a thread observing the clock can never be allowed to see it
either go backwards, or jump forwards.  In other words, observations of the
clock must be consistent with these monotonic/continuous properties.

While not explicitly stated in the Linux man page, we also take this requirement
to mean that the clock is not just consistent from the point of view of a single
observer, but it must also be "multi-observer" consistent.  In other words,
given an _ordered_ sequence of observations (O1, O2, O3 ... On) made by multiple
threads, the sequence of observations must be consistent with both the monotonic
and continuous properties.

A sequence of observations made by multiple observers only has a defined order
if there are interactions between the observers which establish such an order.
For example, if two threads (T1 and T2) make two observations (O1 and O2) and
send them to a third thread, there is no defined order.  T1 and T2 didn't
interact, so there is no way to say which observation "happened first", and the
sequence has no defined order.

As an example of code which produces an ordered sequence of clock observations,
consider a situation where there is a global list of observations (perhaps an
internal debug log) protected by a mutex.  Many different threads will
periodically lock the mutex, make an observation, append it to the global
list, and finally unlock the mutex.  The list now contains an ordered sequence
of observations made by multiple observers, with the order established by the
exclusive nature of the mutex.  This ordered sequence of observations must be
consistent with the monotonic and continuous properties of the clock.

### Readable from a Starnix client at an "affordable runtime cost".

Not a Linux requirement, but a requirement nonetheless.  Starnix restricted mode
processes must be able to observe the `CLOCK_MONOTONIC` clock in a fashion which
is "very cheap".  Note that Starnix clients currently use a Zircon kernel clock
object to provide UTC to its clients.  They originally would exit restricted
mode and enter the Starnix kernel, where a Zircon syscall which entered the
Zircon kernel would then be made.

Needing to both exit restricted mode and enter the Zircon kernel ended up being
an unacceptable amount of overhead, and the system was optimized to have Starnix
attempt to keep a cached copy of the clock object's transformation around,
allowing the clock to be queried without needing to enter the Zircon kernel for
every query.  While this was an improvement, it still required that Starnix
client exit restricted mode, and produced other complications when it came to
maintaining the cached copy of the clock transformation, and producing
consistent results when compared with observations from non-Starnix programs.

## Design

At a high level, the design here is to use
[Zircon Kernel Clock Objects](/reference/kernel_objects/clock.md) to manage the
Starnix-wide concept of the Linux `CLOCK_MONOTONIC` timeline, and to extend the
object API so that it is possible to make observations of the clock very
inexpensive, done in a way which makes it possible for Starnix clients to not
even exit restricted mode.

Aside from the performance goals of never leaving restricted mode, Zircon Kernel
Clocks already satisfy the requirements stated above.  Let's quickly review each
of them.

### Maintainability

Kernel clocks are kernel objects, managed with handles which carry permissions,
so they are inherently delegate-able.  This provides a number of options for
Starnix to use in order to maintain a clock representing the Linux
`CLOCK_MONOTONIC` definition.

1) The clock could be created by Component Manager and distributed to Timekeeper
   for maintenance, and to Starnix for use.
2) The clock could be created and maintained by Timekeeper, and fetched for use
   from Timekeeper by Starnix via a FIDL interface.
3) The clock could be created by Starnix and sent to Timekeeper for maintenance
   via a FIDL interface.
4) Something else entirely.

The main advantage here is that, because it is a kernel object, maintenance of
the clock can be delegated to Timekeeper who is currently also maintaining the
UTC clock, making it relatively simple to apply the same rate adjustments to the
Linux `CLOCK_MONOTONIC` clock that are made to the Fuchsia UTC clock.

### Consistency

Zircon kernel clocks already guarantee multi-observer consistency as defined
above.  The kernel will not allow any update operation which has the potential
to produce an inconsistent observation (as defined by the properties the clock
was created with) to take place.  So, a maintainer cannot cause observers to see
an inconsistent sequence no matter how buggy or malicious the maintainer is.

Likewise, the protocol used in the kernel to observe the clock guarantees a
consistent observation.  Provided that any additions to the API follow the same
protocol, consistency is guaranteed.

### Acceptable observation cost

This brings us to the most significant portion of the design, the ability to
observe a kernel clock object at an acceptable performance price.  The main
goals here are to devise an approach to reading the clock which does not require
entering the Zircon kernel (eliminating that overhead) and which can also be
implemented in a fashion which does not even require a restricted mode process
to exit restricted mode (which it would need to do in order to make a Zircon
syscall, regardless of whether that syscall enters the Zircon kernel or not).

#### Mappable State

Understanding how kernel clock objects do what they do can be helpful, details
are available in public docs [here](/reference/kernel_objects/clock.md).

The most important thing to understand, however, is that a kernel clock's state
is really only an affine function which transforms the selected reference clock
(either Zircon monotonic or boot) to the current value  of the synthetic
timeline that the clock represents.  Making a consistent observation of the
clock object requires conducting a successful "read transaction" where we record
the current state of the transformation along with the current value of the
reference clock, and being certain that no changes were made to the synthetic
clock's transformation state during the transaction.

As of today, the clock's state is stored in kernel memory and is not (directly)
accessible from user mode.  User mode may fetch a recent version of the clock's
state using a [`zx_clock_get_details`](/reference/syscalls/clock_get_details.md)
syscall, but this not only requires a syscall, but the information may already
be stale by the time the syscall returns introducing the potential for an
inconsistent sequence of observations.

There is nothing particularly special about this state, however.  Users with
read-access to the clock can already fetch the details of the clock's active
transformation.  It should be possible to eliminate the need for a program to
enter the Zircon kernel to observe the clock's details, we simply need extend
the clock object implementation to allow the clock's state to be stored in a
page of RAM which can be mapped (read-only) into a user mode process.  With this
approach, user mode can map the clock state into their process, and provided
that they follow the observation protocol, and provided that the reference clock
is directly accessible without entering the Zircon kernel, consistent
observations can be made without the extra overhead of entering the kernel.

#### Observing the clock in user mode

Observing a memory mapped clock in user mode requires following a very specific
protocol which *must* match the kernel's behavior at all time.  Additionally,
following the protocol requires knowledge of the specific layout of the clock
state data in the mapped page.  It is very important that Zircon retains the
ability to alter either the observation protocol or the shared memory layout
without coordinating with client programs and going through a full formal
deprecation cycle.

As such, users are *required* to treat their pointer to the mapped clock state
as a pointer to an opaque blob, and to pass it to Zircon syscalls implemented in
the Zircon vDSO.  Note that these syscalls will _almost_ never need to actually
enter the Zircon kernel in order to read the clock.  The _only_ time entering
the kernel would be needed is when the reference clock for the clock object can
only be read by entering the kernel (this is currently extremely uncommon).
Keeping the shared data format deliberately opaque and putting the read
implementation into a Zircon syscall implemented in the Zircon vDSO gives users
the performance benefits of not needing to enter the Zircon kernel, while
retaining the ability to change details of the structure and read protocol
without needing to alter the ABI contract.

This said (and as noted before), Starnix clients run in restricted mode, and
therefore cannot make Zircon syscalls, even those which won't enter the Zircon
kernel.  Note that this is _already_ a problem for restricted mode clients who
would like to access existing Zircon syscalls which typically do not ever enter
the Zircon kernel, the most obvious example being `zx_clock_get_monotonic()`.

For now, a special workaround for these situations is used in Starnix.  A common
library (`libfasttime`) was created, where the logic used to implement
`get_monotonic` was moved.  The Zircon vDSO implements
`zx_clock_get_monotonic()` using this library when fetching the monotonic
reference does not require entering the kernel, falling back on a full kernel
entry otherwise.

Starnix does a similar thing with its restricted mode clients in order to give
them access to the Zircon monotonic reference, using the code in `libfasttime`
to implement the actual operation in restricted mode without needing to exit
restricted mode to implement the functionality of `zx_clock_get_monotonic()`.
The plan here is to simply extend this technique by putting memory mapped clock
read operations into `libfasttime`, and then using this implementation
everywhere.

It is understood that this means that there is a dependency between Starnix and
the Zircon kernel.  Specifically, every time something in `libfasttime` changes,
both Starnix and Zircon need to be re-built and deployed together.  There are
longer term efforts to eliminate this dependency, and the new clock reading
operations will take advantage of these efforts when they are eventually
realized.  Extending the existing technique does not make the situation any
worse than it is today, and is therefore considered to be an acceptable
approach.

#### Clock read protocol details.

What follows is a description of how kernel clocks are currently observed in a
fashion which guarantees consistency, and will be initially implemented in
`libfasttime`.  It is included to help to provide some intuition about how this
is all working "under the hood", but should not be taken to be part of the
kernel's ABI contract.

Internally, kernel clock objects use a special type of lock called a
[`SeqLock`](https://cs.opensource.google/fuchsia/fuchsia/+/main:zircon/system/ulib/concurrent/README.md)
in order to protect their state.  These locks have some helpful properties when
it comes to implementing a syscall-free way to observe a clock.  Notably:

1) `SeqLock`s allow concurrent reads, but read transactions can never prevent a
    write transaction. So, user-mode programs cannot prevent other user-mode
    programs from reading a clock, nor can they stall code in the kernel which
    is updating a clock.
2) `SeqLock` read transactions do not need to mutate state.  So, if the lock
    state is part of the mapped clock state, the lock can be used in user mode
    even though the state is mapped read-only.
3) `SeqLock` read transactions do not ever block the reader thread, meaning that
    no futex syscalls are needed in user mode.  Additionally, they cannot
    prevent either read or write transactions from taking place concurrently, so
    there is no need to ever disable interrupts (something which cannot be done
    in user mode).

At a high level, performing a proper observation of the clock is simple.  The
sequence would be:

1) Begin a read transaction using the `SeqLock` protecting the mapped clock
   state.
2) Copy the clock's state to a local stack allocated buffer.
3) Read the reference clock value.
4) End the read transaction, re-trying if it was interrupted until we succeed.
5) Compute the synthetic time value using the saved reference clock value and
   clock state.

Digging deeper, however, this is not as quite simple as it seems.  `SeqLock`
transactions need to use atomic loads for both the lock's state as well as the
protected payload's state, and may require explicit fence instructions depending
on how the payload is handled (`AcqRel` atomic vs. `Relaxed` atomic).  As an
additional complication, the hardware counter register which is the basis for
the reference clock _is not memory_, and does not obey the C++ memory model when
it comes to ordering and pipelined execution.  Special architecture and timer
specific instructions need to be used in order to prevent the read of this
register from "slipping" outside of the transaction.  Finally, the method used
to transform the reference clock value to the proper synthetic clock value needs
to be consistent across all users, else rounding errors could result in a
sequence of observations from observers using different conversion techniques
which appear inconsistent.

In order to keep everything on the same page, the design here calls for the code
to be re-factored as follows:

1) The core of the `zx_clock_read` and `zx_clock_get_details` operations will be
   moved from kernel code into `libfasttime`, and the kernel code will be
   refactored to use the `libfasttime` implementation.
2) Versions of `zx_clock_read` and `zx_clock_get_details` which operate on clock
   state which has been mapped into the local address space will be added as
   Zircon syscalls to the Zircon vDSO.  The implementation of these vDSO calls
   will be based on the `libfasttime` implementation, ensuring that everyone is
   using a properly matched implementation.

### The API

The user-mode API to support mappable clocks consists of three new syscalls, two
of which will typically never enter the Zircon kernel. Additionally, a new
option will be introduced which will need to be passed when creating a clock,
and a new [`zx_object_get_info()`](/reference/syscalls/object_get_info.md) topic
will be introduced to allow users to know the mapped size of a clock instance.

#### Clock Creation

Creating a clock which can be mapped will require that a user pass a new
`ZX_CLOCK_OPT_MAPPABLE` flag to the options field of
[`zx_clock_create`](/reference/syscalls/clock_create.md).  When the call to
create a mappable clock succeeds, the clock object will allocate a VMO
internally and use this VMO to store its state instead of internal kernel
memory.  Additionally, the handle returned from this successful call will
include the `ZX_RIGHT_MAP` in addition to the default clock rights.  Initial
handles to clocks created without this option will not contain the right.

#### Determining the space needed to map a clock

At the time of this RFC draft the current size of a kernel clock's state,
including padding for alignment, is 160 bytes.  This will easily fit into a
single 4KB page, with significant room for future growth.

That said, simply assuming that the size of a mapped clock is now and will
always be exactly one 4KB page would not be prudent for a number of reasons.

1) The underlying page size of the system might someday change, perhaps from 4KB
   to 16KB or even 64KB.
2) The state of the clock object might actually need to grow significantly,
   perhaps even exceeding a single page (theoretical examples available on
   request).
3) A developer may need to temporarily extend the size of the mapped clock
   representation as part of a debugging effort which requires more state to be
   maintained.  Even if the larger clock representation is never shipped to any
   end users, growing the clock size as part of an experiment would ideally not
   break existing code.

So, the size of a mapped clock is not guaranteed to be constant, and users who
wish to map clock state into their address space need to know how large a
clock's state is in order to properly manage their address space.

In order to address this a new info topic (`ZX_INFO_CLOCK_MAPPED_SIZE`) will be
added.  When used in conjunction with `zx_clock_get_info()` on a clock which has
been successfully created with the `ZX_CLOCK_OPT_MAPPABLE` flag, the size of the
mapped clock state in bytes will be returned in a single `uint64_t`.

Attempting to fetch a clocks mapped size for a clock object which is not
mappable is considered to be an error and will return `ZX_ERR_INVALID_ARGS`,
while attempting to fetch the mapped-clock size of an object which is not a
clock type will return `ZX_ERR_WRONG_TYPE`.

#### Mapping and Unmapping the clock

A new syscall (`zx_vmar_map_clock`) will be added allowing users to actually map
the internal VMO into their address space so that it can be used for `read` and
`get_details` operations.

```
zx_status_t zx_vmar_map_clock(zx_handle_t handle,
                              zx_vm_option_t options,
                              uint64_t vmar_offset,
                              zx_handle_t clock_handle,
                              uint64_t len,
                              user_out_ptr<zx_vaddr_t> mapped_addr);
```

The operation is _almost_ identical to
[`zx_vmar_map()`](/reference/syscalls/vmar_map.md) or
[`zx_vmar_map_iob()`](/reference/syscalls/vmar_map_iob), but with a few
important differences.

1) Most, but not all, options which can be used with `zx_vmar_map()` are
   permitted.  In particular, users may not specify `ZX_VM_PERM_WRITE`,
   `ZX_VM_PERM_EXECUTE`, or `ZX_VM_PERM_READ_IF_XOM_UNSUPPORTED`.  The clock's
   state cannot be allowed to be altered by user mode clients (so, no WRITE),
   and certainly is not code (so no EXECUTE).  Any attempt to use any of these
   options will result in a `ZX_ERR_INVALID_ARGS` error.  All other options,
   especially those related to positioning of the mapping in a VMAR are
   supported.
2) `len` is required (as it is in other VMAR map operations) however the amount
   to map is not arbitrary or under user-mode control.  The length of the
   mapping for a given clock instance *must* match the size as reported when
   fetching info using the newly introduced `ZX_INFO_CLOCK_MAPPED_SIZE` topic.
   Any other value passed for `len` is considered to be an error.
3) No `vmo_offset` parameter is provided.  Users must always map all of the
   clock state.  Partial mappings of the clock state starting at an offset into
   the VMO are not allowed, making a `vmo_offset` useless.
4) No `region_index` parameter is provided, as it is with `zx_vmar_map_iob()`.
   IO Ring Buffer objects potentially have multiple regions which can each be
   mapped independently, but clock object do not, meaning that an index
   parameter would serve no purpose.

Keeping the clock's internal VMO internal to the kernel means that we do not
have to specify or test the behavior of a clock's VMO's behavior when passed to
any other VMO syscalls.

Unmapping a clock's state is identical to unmapping any other mapping made by
user mode.  Users simply call `zx_vmar_unmap()` passing the virtual address
returned by their mapping operation as well as the `len` used in the original
mapping.

Users are free to close their clock handle after mapping it, should they choose
to do so.  The mapping will remain, and can still be used to read the clock's
state.  In the event that all handles to a mapped clock are closed and the clock
object is destroyed, the internal VMO and any remaining mappings will remain.
At this point, however, there will be no ability for any program to continue to
"maintain" the clock's transformation as the object has been destroyed and there
is now no way for anything to update the transformation.  Conceptually, the
clock will simply continue to exist, and drift forever after.  Note that this is
no different from a clock maintainer closing their last handle to a clock that
they are responsible for maintaining while consumers of the clock retain their
handles.

#### Observing the Clock.

```
zx_status_t zx_clock_read_mapped(const void* clock_addr, zx_time_t* now);

zx_status_t zx_clock_get_details_mapped(const void* clock_addr,
                                        uint64_t options,
                                        void* details);
```

In order to perform an observation of the clock, users may call the appropriate
new syscall, either `zx_clock_read_mapped` or `zx_clock_get_details_mapped`.

These syscalls will be implemented in the Zircon vDSO, and will only enter the
Zircon kernel if accessing the underlying reference clock requires it.

The functions themselves will operate identically to their non-mapped counter
parts ([zx_clock_read](/reference/syscalls/clock_read.md) and
[zx_clock_get_details](/reference/syscalls/clock_get_details.md)), with the
following exceptions.

1) Instead of passing the clock handle as the first argument, users *must* pass
   the address of where their clock state was mapped in their current address
   space, while that state is still mapped.  Passing any other value, or passing
   an address to clock state which has been unmapped (either completely or
   partially) will result in undefined behavior.
2) No rights check will be made when calling either of these routines.  Users
   implicitly have the right to read a mapped clock by nature of the fact that
   its state has been successfully mapped into their address space.

As noted earlier, Starnix clients run in restricted mode, and will be
unable to use these syscalls directly.  In order to avoid the need to exit
restricted mode, Starnix may make use of `libfasttime` in a fashion similar to
how it is used today in order to implement the equivalent of
`zx_clock_get_monotonic()`, until a better technique which avoids Starnix
depending on `libfasttime` has been developed.

#### Clock mapping representation in `zx_info_maps_t` structures

Zircon currently provides a couple of different ways for developers to fetch
information about currently active mappings for diagnostic purposes.  Both are
accessed using [`zx_object_get_info()`](/reference/syscalls/object_get_info)
topics:

++ [`ZX_INFO_VMAR_MAPS`](/reference/syscalls/object_get_info#zx_info_vmar_maps)
++ [`ZX_INFO_PROCESS_MAPS`](/reference/syscalls/object_get_info#zx_info_process_maps)

The first of these topics enumerates the set of active mappings in a given
VMAR, while the second does the same but for a process object.  In either case,
the results are returned in an array of
[`zx_info_maps_t`](/reference/syscalls/object_get_info#zx_info_process_maps)
structures.  Now that clock objects themselves can create mappings, we need to
be able to answer some questions about what information will be present in an
enumerated record for a mapped clock.

1) What goes in the name field?  Kernel clocks currently do not have assignable
   names, so for now, when clock mappings are enumerated, the name will be set
   to the constant string "kernel-clock" to help humans looking at diagnostic
   data.  If (someday) kernel clocks are extended to allow user configurable
   names, the obvious next step would be to report the user assigned name
   instead of the constant name.
2) What is the "type" of the union in the record?  Following the example of
   [mapped IOB regions](/reference/syscalls/vmar_map_iob), the type will be
   `ZX_INFO_MAPS_TYPE_MAPPING`, and the members of `u.mapping` will be
   populated.
3) What value is reported in the `u.mapping.vmo_koid` field?  While kernel
   clocks internally use a VMO to gain access to the pages needed to share clock
   state with processes via a mapping, this VMO is not wrapped in a
   `VmObjectDispatcher`, and therefore has no KOID assigned to it.  Because of
   this, the KOID of the clock object itself will be reported instead.

## Implementation

The implementation of this feature will be landed as a sequence of Gerrit CLs,
mostly to keep code review size reasonable and manageable.  The CLs to land will
be as follows.

1) API stubs will be added to the Zircon vDSO for the 3 calls described above.
   The initial implementation of these stubs will simply return
   `ZX_ERR_NOT_SUPPORTED`.  They will initially be added under the `@next` API.
2) The `read` and `get_details` implementations will be moved into
   `libfasttime`, and the kernel code will be changed to use those
   implementation instead of using its own.
3) The updated behavior of `zx_clock_create`, the actual implementation of
   `zx_clock_get_vmo`, and support for the `ZX_INFO_CLOCK_MAPPED_SIZE` topic
   will be added to the kernel.
4) The actual implementations of `zx_clock_read_mapped` and
   `zx_clock_get_details_mapped` will be added to the Zircon vDSO.
5) C++ specific wrappers will be added to `libzx`.
6) Tests for the entire feature will be added to the existing Zircon kernel
   clock tests.  Documentation for the feature and new syscalls will be added to
   the existing public documentation.

At this point, the implementation is complete.  Additional language specific
bindings may now be added to Rust, and Starnix may now make use of the feature
and code in `libfasttime` to provide efficient access to zircon clocks for
their restricted mode clients.

After a period of usage and stabilization (during which changes may be made to
the API as appropriate), the API will be taken out of `@next` and promoted the
being a member of the current Zircon Kernel API.

## Performance

The cost of accessing a memory mapped kernel clock from a Zircon process is
expected to be moderately less than accessing it via a syscall, as all of the
cost of entering the Zircon kernel and performing handle validation will have
been eliminated.  This will also remove a small amount of pressure on the
process handle table as it will no longer need to be locked for read during
handle validation.

The price for this will be a small a increase in memory usage as we now need a
page of memory for the clock state instead of the ~160 bytes of kernel heap
memory used to track the clock state today.  Additionally, a small amount of
kernel bookkeeping will be needed for the kernel representation of the VMO
object itself, as well the page table overhead required to map the clock in
processes which choose to do so.

The effects on Starnix restricted mode processes is expected to be more
significant.  Clients will no longer have to exit restricted mode and enter the
Starnix kernel to access a clock object.  Additionally, the Starnix kernel will
no longer need to monitor the system-wide UTC for changes, and update its
internal cached version that it maintained in order to avoid making a syscall to
read the clock.  Instead, restricted mode clients can just directly read the
clock without ever having to exit their current context to either the Starnix or
Zircon kernel.

## Ergonomics

The overall API ergonomics should be virtually identical to the existing API.
With a small amount of setup (fetching and mapping the VMO), users can now read
Zircon kernel clocks in a fashion identical to how they already to it, simply
substituting their clock state pointer for the clock handle they would have
passed to the original read routines.

Here is a small example to demonstrate the API differences.

```
// An object which creates a non-mappable kernel clock and allows users to read it.
class BasicClock {
 public:
  BasicClock() {
    const zx_status_t status = zx::clock::create(ZX_CLOCK_OPT_AUTO_START,
                                                 nullptr,
                                                 &clock_);
    ZX_ASSERT(status == ZX_OK);
  }

  zx_time_t Read() const {
    zx_time_t val{0};
    const zx_status_t status = clock_.read(&val);
    ZX_ASSERT(status == ZX_OK);
    return val;
  }

 private:
  zx::clock clock_;
};

// And now the same thing, but with a mappable clock.  There are a few extra
// setup and teardown steps, but the read operation is virtually identical.
// An object which creates a non-mappable kernel clock and allows users to read it.
class MappedClock {
 public:
  MappedClock() {
    // Make the clock
    zx_status_t status = zx::clock::create(ZX_CLOCK_OPT_AUTO_START | ZX_CLOCK_OPT_MAPPABLE,
                                           nullptr,
                                           &clock_);
    ZX_ASSERT(status == ZX_OK);

    // Get the mapped size.
    status = clock_.get_info(ZX_INFO_CLOCK_MAPPED_SIZE, &mapped_clock_size_,
                             sizeof(mapped_clock_size_), nullptr, nullptr);
    ZX_ASSERT(status == ZX_OK);

    // Map it.  We can close the clock now if we want to, just need to remember to un-map our
    // region when we destruct.
    constexpr zx_vm_option_t opts = ZX_VM_PERM_READ | ZX_VM_MAP_RANGE;
    status = zx::vmar::root_self()->map(opts, 0, clock_vmo, 0, mapped_clock_size_, &clock_addr_);
    ZX_ASSERT(status == ZX_OK);
  }

  ~MappedClock() {
    // Un-map our clock.
    zx::vmar::root_self()->unmap(clock_addr_, mapped_clock_size_);
  }

  // The read is basically the same, just pass the mapped clock address instead of using the handle.
  zx_time_t Read() const {
    zx_time_t val{0};
    const zx_status_t status = zx::clock::read_mapped(reinterpret_cast<const void*>(clock_addr_),
                                                      &val);
    ZX_ASSERT(status == ZX_OK);
    return val;
  }

 private:
  zx::clock clock_;
  zx_vaddr_t clock_addr_{0};
  size_t mapped_clock_size_{0};
};
```

## Backwards Compatibility

For programs which can directly make Zircon syscalls, there are no backwards
compatibility issues which need to be considered.  No change is being made to
existing clock functionality, and the nature of the Zircon vDSO prevents us from
having to worry about making any long term commitments to either the layout of
the state structure, or to the required clock observation protocol.

Starnix, however, will have a dependency on the kernel (or more precisely,
`libfasttime`).  Any changes made to the structure layout or the observation
protocol will be updated in the `libfasttime` library, and Starnix will need to
be re-compiled.  This is not currently considered to be a significant concern
for two reasons.

1) This is _already_ the state of affairs given the way that syscall-free access
   to the Zircon monotonic and boot timelines are being provided to Starnix
   clients running in restricted mode.  In other words, this is not a new
   dependency, merely a continuation of what is already being done.
2) In the longer term, there are ideas being considered for how to eliminate the
   pre-existing dependency.  If and when these ideas become reality, this new
   API will benefit from the improvements, just like `zx_clock_get_monotonic`
   will.

## Security considerations

There are no known security implications to allowing users to map the clock
state (read only) into their address spaces.  The protocol used to observe the
clock does not allow user mode to lock the clock exclusively (which would expose
us to various DoS attacks) or to modify the clock state (potentially
invalidating clock properties guaranteed by the kernel).  All of the information
contained in the clock state is already accessible from user-mode using a call
to `zx_clock_get_details()`, this new functionality is just an optimization on
how we get to it.

No additional ability to access a high resolution timer is introduced by adding
the ability to access kernel clock state directly.  Clocks are nothing more than
transformations of existing references, so any potential future limitations of
underlying reference clock resolution available to a process will also (by
necessity) affect synthetic clocks derived from those reference.

## Privacy considerations

There are no known privacy implications of this proposal.  There is no user data
involved anywhere here.

## Testing

Testing core functionality will be achieved by extending the Zircon clock unit
tests in two ways.

1) Tests will be added to ensure that the rules (as stated above) involving
   creating, mapping, and unmapping a mappable clock are enforced.
2) Existing tests for `zx_clock_read` and `zx_clock_get_details` will be
   extended to test the `mapped` versions of these calls as well.  Their
   behavior should be virtually identical, so the tests should end up being so
   as well.

## Documentation

1) [Top level public docs](/reference/kernel_objects/clock.md) will be updated
   to include a general description of the feature with links to the newly added
   API methods.  Additionally, it should include at least one example of
   creating, mapping, reading, and unmapping a clock.
2) [`zx_clock_create`](/reference/syscalls/clock_create.md) will be extended to
   describe the new `ZX_CLOCK_OPT_MAPPABLE` flag.
3) New pages will be added to the clock reference docs to describe
   `zx_clock_get_vmo`, `zx_clock_read_mapped`, and
   `zx_clock_get_details_mapped`.

## Drawbacks, alternatives, and unknowns

### Alternatives to creating mappable clocks

Aside from explicitly disallowing all of the disallowed behaviors described
above (and write tests for that code), implementation costs are relatively low.
A small refactor to move the observation routines to a common location, as well
as a small amount of restructuring of the `ClockDispatcher` code in the kernel
to allow it to use a VMO for its storage in addition to internal storage.  Then
just functionality tests and documentation.

Alternatives exist, but involve significantly more work, and end up producing a
very specific solution instead of a general tool.

For example, Starnix could create and manage its own concept of Linux
`CLOCK_MONOTONIC` which it could then expose to its restricted mode processes,
either with a similar memory mapping technique, or by forcing its restricted
mode clients exit restricted mode into the Starnix kernel.  This still leaves a
number of issues which would need to be solved.

1) Locking for high read concurrency.  `SeqLock`s are very cool, but while using
   them for read transactions is easy in user-mode, locking them for exclusive
   write in user-mode is a very dicey proposition as there is no way to disable
   interrupts or otherwise prevent involuntary preemption which could lead to an
   update operation preventing forward progress on read operations while it has
   been preempted.
2) Obtaining clock correction information.  The clock Starnix wants to create is
   supposed to be subjected to the same rate adjustments that the system-wide
   UTC reference is subjected to, however those adjustments are being performed
   by the Timekeeper, not Starnix.  It would be a very large amount of work to
   replicate what Timerkeeper is doing (in addition to being redundant work), so
   it is likely that Timerkeeper would need to both forward correction
   information to the Starnix maintained clock, and Starnix would likely need to
   provide feedback information to Timekeeper in order to close the clock
   recovery loop.
3) Ensuring proper memory order semantics.  Depending on what solution is used
   for #1, care would need to be taken to ensure that the clock state payload
   could be observed without any formal data races while keeping the clock
   register observations contained in the read transaction.  This can be tricky
   stuff, and a bit of a pain to get right and maintain.

So, Starnix could do a lot of this if it wanted to, but it would amount to a
relatively large duplication of effort to do things that kernel clocks already
do today.  Moving forward, that would also mean additional tests, and additional
long term maintenance requirements.

### Alternatives to the `libfasttime` abstraction

In order to provide the promised performance benefits to Starnix restricted mode
clients, code needs to exist (somehow) in the restricted mode client programs
which both understands layout of the clock state, and follows the protocol
required for making consistent observations of the clock state.

This proposal satisfies this requirement by providing code in the form of
`libfasttime` which clients embed in their programs in order to observe their
memory mapped clocks.  The obvious (and significant) downside of this is that it
creates a dependency between clients who are unable to make Zircon syscalls, but
wish to make use of the memory mapped clock feature.  Any time the kernel
changes in a way where `libfasttime` changes, clients *must* rebuild against the
new library, or risk incompatibility.  The fact that this is a pre-existing
issue (already present in equivalent implementations of
`zx_clock_get_monotonic()`) does not change this fact.

The alternative to doing this _could_ be to formalize the ABI exposed by Zircon,
rigorously documenting both the in memory layout of the clock's state, as well
as the protocol required to make a consistent observation of the clock, thus
allowing clients to implement and maintain their own "mappable clock read"
routines.

The result of this, however, would be:

1) A significant increase in the difficulty required to make changes to either
   the layout of the clock state, or the protocol used to observe it.
   Basically, making changes to the Zircon side of this feature becomes more
   difficult.
2) A significant increase in the burden of responsibility for clients.  Instead
   of just making use of (presumably tested and maintained) library code, they
   are now responsible for reading and fully understanding the hypothetical new
   API specification.  Additionally, they would be required to implement, test,
   and maintain, their own version of the observation routines.

So while this is an alternative approach, it is not currently being seriously
considered at this time due to the two significant drawbacks just stated.  It is
anticipated that other approaches to solving this problem for clients similar to
Starnix restricted mode clients will eventually be introduced which allow for
kernel flexibility without requiring excessive implementation and maintenance
burdens from its clients.

### Alternatives to `ZX_CLOCK_OPT_MAPPABLE`

Instead of requiring the creator of a clock to decide if a given clock is or is
not mappable, why not simply create the clock state VMO on the fly, when it
becomes needed.  For example, if a user calls `zx_vmar_map_clock()` and no VMO
has been allocated yet, simply allocate the VMO on the fly and swap it in for
the kernel heap state.

Note that this would be a one way transition.  Once a clock has been made
mappable, we cannot reasonably ever go back to the clock being non-mappable
since we don't have much control of whether or not clients of the mapped
currently exist, nor any reasonable way to force them to change back.

It is certainly possible to write the code to do this, however there are race
conditions which would need to be solved.  Locks (or some other serialization
mechanism) would need to be introduced around `zx_vmar_map_clock` to prevent
concurrent requests which require VMO allocation from racing with each other.
Swapping in the VMO storage, replacing the in-kernel state, would effectively be
a clock update, requiring a sequence lock write transaction while the state is
moved from one location to another.

While this is _possible_, it is more code, more complexity, and more testing.
Additionally, the rigorous testing would imply testing that race conditions
resulting from concurrent requests are properly handled, something which is
not really possible to do deterministically with things like unit tests.

Overall, the requirement that users opt in to "mappability" at clock creation
time is currently considered to be a reasonable and not overly burdensome
requirement in order to avoid the complexity of needing to deal with a late
binding request to become mappable.

### Alternatives to `zx_vmar_map_clock()`

In this proposal, kernel clock objects gain the ability to be "mappable
objects", joining the ranks of VMOs and IOB Regions.  As with IOB regions, they
are assigned their own special syscall used to map their underlying VMO, however
the VMO itself is never made directly available to user-mode.

An alternative to this approach could be to skip the specialized mapping call,
and instead provide users direct access to the underlying VMO itself.  So,
remove the `zx_vmar_map_clock()` syscall, and add a `zx_clock_get_vmo()` call
instead.

#### Pros

Assuming that the same approach could be applied to IOB regions (the other
mappable, not-strictly-a-VMO objects in the kernel), this would mean that the
system would no longer have to consider anything but VMOs as mappable objects.
This consistency could provide benefits for users when it comes to understanding
and reasoning about the system.

++ "What objects are 'mappable' in Zircon?  Its easy!  VMOs, and nothing else".
++ "What can I do with a 'clock vmo'?  Its easy!  Anything the permissions
   granted to your handle allow you to do.

Questions about things like "What KOID to I report in a diagnostic mapping
query?" become moot.  One simply reports the KOID of the VMO itself.  Now that
it formally exists ("under the hood") as a kernel Dispatcher object, we can just
use the KOID assigned to the dispatcher.  Likewise, VMOs are name-able object,
meaning the name to report is now obvious as well.

Another benefit might be in making it a bit easier for developers to comply with
the "You must map exactly the size we tell you to map" requirement given
earlier.  The requirement to do this does not go away, however the
`zx_clock_get_vmo()` syscall could be specified in a way that returns the
required mapping size in addition to the VMO's handle.  This way, users are
forced to know the size in order to get the VMO (a hard requirement to map the
clock state in the first place) making it quite simple to follow the
requirement.

Finally, exposing the VMO directly gives alternate ways for the upper levels of
the system to share a clock.  They could duplicate and share a handle to the
clock itself (with appropriate rights), but they also now have the ability to
simply duplicate a handle to the underlying VMO (again, with appropriate rights)
and share that instead.

#### Cons

Directly exposing the VMO is not entirely without drawbacks, however.  The
purpose of allowing clock state to be mapped is _very_ specific.  From the
user's perspective, the contents mapped VMO is not even defined.  The only thing
users are supposed to do is use the mapped address as something like a token
they pass to specialized syscalls which have lower overhead than their
traditional counterparts.

Exposing the VMO itself, however, exposes a significant amount of additional
kernel API surface area.  A large number of the things that a user might attempt
to do given direct access to the underlying VMO are functionally pointless, but
are now possible.

++ Can a user create a CoW clone of a clock state VMO, or any child for that
   matter?  What would that mean?
++ Can a user control the cache policy of a clock state VMO?
++ What happens if a user attempts to set the size of a clock state VMO?  How
   about the "stream" size?
++ What about the various `op_range` operations a user can request?  Do they
   provide any value at all?  Do they open up any security threats?
++ Who controls the VMO object's properties?  Can anyone set the VMOs name, or
   should that be restricted?  If it should be restricted, how should that be
   done?

A few of the operations which would now be available to users are simultaneously
pointless as well as benign; `zx_vmo_read()`, for example.  There is absolutely
no point for a user to _ever_ need to do this as the contents which would be
read are not even defined for the user (by design).  That said, is there any
danger to allowing them to do this?  _Technically_ yes, as allowing them to read
the data without following the sequence lock protocol could lead to a formal
data race as the clock is concurrently updated, but practically speaking no.
They might get garbage back from their reads, but the system is not going to
halt and catch fire.

For many of the other operations, however, the answer is not as clear cut.  CoW
clones should almost certainly be disallowed, but what about the object's name?

Every one of these potential operations needs to be evaluated on a case by case
basis to ensure that allowing an operation is safe from a security and privacy
perspective.  It is anticipated (out of prudence, if nothing else) that the
majority of these operations would be disallowed.  If they serve no meaningful
purpose, the default stance should be to disallow the operations.

This now means that these restrictions need to be enforced.  Some of this can be
done using appropriate handle permissions, but others might need custom
enforcement mechanisms.  Regardless of the mechanism, there will exist code in
the kernel (some of it new) which needs to enforce these restrictions.  Tests
will need to be written to ensure that the restrictions are in place, and that
they are being properly enforced.  The tests and enforcement code will also need
to be maintained in perpetuity, not only for existing APIs, but any future VMO
APIs added to the system as well.

The vast majority of users of mapped clocks are not interested in any of this.
They just want optimized access to their clock objects; they are not looking to
set the cache policy of their mapping, or create child clones of their clock
state (whatever that would mean).  Someone might want to set the name of their
mapping, but that can be just as easily done by naming the clock object itself
as it can be by naming the VMO.

It is difficult to ignore the idea that giving users access to this API surface
area is of no benefit to legitimate users of mapped clocks, but _might_ be
beneficial to malicious actors attempting to find an exploit in the exposed API
surface area. The cost of analyzing these potential attacks, mitigating them,
and enforcing these mitigations as the system evolves in the future are not
entirely understood, but it is safe to say that they are almost certainly
non-trivial.

#### Conclusion

For now, the decision is to follow the precedent of IOB Regions and make clock
objects themselves "mappable", drastically curtailing the exposed API surface
are, and hopefully reducing the overall complexity of the feature and its
maintenance.

Moving forward, we may end up re-evaluating this decision, perhaps deciding to
unify both clocks and IOB regions with a model where underlying VMOs are
directly exposed instead of being "special things which can be mapped in
addition to VMOs", but for now the system will provide only an extremely limited
capability to control how the shared state can be used.

## Prior art and references

Current API

 - [Clock Overview](/reference/kernel_objects/clock.md)
 - [clock transformations](/docs/concepts/kernel/clock_transformations.md)
 - [`zx_clock_create()`](/reference/syscalls/clock_create.md)
 - [`zx_clock_read()`](/reference/syscalls/clock_read.md)
 - [`zx_clock_get_details()`](/reference/syscalls/clock_get_details.md)
 - [`zx_clock_update()`](/reference/syscalls/clock_update.md)

Sequence Locks

 - [`SeqLock` Overview](https://cs.opensource.google/fuchsia/fuchsia/+/main:zircon/system/ulib/concurrent/README.md)s
 - [`SeqLock` Memory Model Details](https://cs.opensource.google/fuchsia/fuchsia/+/main:zircon/system/ulib/concurrent/models),
