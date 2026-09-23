<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0261" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Summary

This document proposes a set of features to enable Fuchsia programs to act as
secure, efficient supervisors of untrusted guest code without using hypervisor
technology.  The primary use case is to enable a faster and more efficient
[Starnix][starnix] runner.

These features are sometimes referred to collectively as Restricted Mode.  In
the context of Starnix, these features are the basis for the Starnix Execution
Model.

This document focuses on the Zircon kernel and the introduction of new kernel
features.  The details of how exactly Starnix builds on these features is out of
scope.

## Motivation

For background,
see [RFC-0082: Running unmodified Linux programs on Fuchsia][rfc-0082].

We want to make it possible to develop Fuchsia programs that efficiently emulate
other operating systems' interfaces.  In particular, we want to run unmodified
Linux programs on Fuchsia with near native performance.

## Stakeholders

_Facilitator:_ davemoore@google.com

_Reviewers:_ abarth@google.com, adanis@google.com, cpu@google.com,
jamesr@google.com, lindkvist@google.com, mvanotti@google.com

_Consulted:_ abarth@google.com, adanis@google.com,
rashaeqbal@google.come, lindkvist@google.com, mcgrathr@google.com,
mvanotti@google.com

_Socialization:_ This proposal has been socialized with the kernel and Starnix
teams over email, chat, and multiple Kernel Evolution Working Group (KEWG)
sessions.

## Goals

The following goals guide the design of this system.

*Fast and efficient emulation* - This new mechanism must provide faster and more
efficient emulation than the Zircon exception based mechanism.

*Strong security boundary* - This mechanism must act as a strong security
boundary and allow safe execution of untrusted code.

*Natural and simplified programming model* - The programming model presented by
this mechanism should not only lend itself to emulating the Linux kernel
interface, but also handle much of the heavy lifting.  As for natural, most
kernels place kernel code and data in one shared address space so it would be
convenient if this new mechanism provided a programming model with a shared
address space (more on this in the Shared State section).  As for simple, we'd
prefer to build an OS on top of higher level OS abstractions, rather than on
lower level hardware abstractions.

*Compartmentalized complexity* - We want to compartmentalize changes to the
Zircon kernel to better manage complexity and remain flexible.  Concretely, this
means minimizing the degree to which this mechanism cuts across kernel
subsystems.

## Scope

The scope of this proposal includes the model and general approach (described in
the Design section) as well as a system call interface (described in the
Implementation section).

## Design

The system consists of three design elements intended to be used together.

The main elements of this design are restricted mode, direct access, and shared
state.  Together, they provide a multi-process, shared-memory programming model
for implementing a user space kernel on top of Zircon abstractions.

With this architecture, each guest thread will be modeled by one Zircon thread
and each guest process will be modeled by one Zircon process.  Each of the
Zircon processes will have a region of their address space that's common to all.
To a guest kernel developer, the shared region will make the guest kernel
look and feel more like a single multi-threaded program.

### Element 1: restricted mode

This is the core element.

Today, when a thread is running, it's running in one of two modes, user mode or
kernel mode.  When a thread issues a syscall it transitions, via mode switch,
from user mode up to kernel mode.  Upon completing the syscall, it transitions
back down to user mode.

> **Note:** For simplicity we'll use ARMv8a terminology in this document unless
> otherwise specified.

<!-- diagram 1 source: https://docs.google.com/drawings/d/1HTrceIdVWHSIfinMQh8iX-mX_kO2C8pSw6Xgn2sGuV0/edit?usp=sharing&resourcekey=0-blylkgDX6YtFfiti78EZKA -->
![Mode switch via syscall][diagram-1-mode-switch]

Of course, syscalls aren't the only events that result in a mode switch.  Any
event that the kernel might need to handle will trigger a mode switch to kernel
mode.  This includes page faults, exceptions, and interrupts.

We introduce an optional third mode called restricted mode.  This new mode is
optional because not all threads in the system will enter restricted mode.
Similar to how user mode is a more constrained execution environment than kernel
mode, restricted mode is more constrained than user mode.

While kernel mode and user mode map to hardware features (e.g. EL1 and EL0 in
ARMv8-A), restricted mode does not.  It's a software-only construct.  The
hardware knows nothing of restricted mode.  From the hardware's perspective it's
just user mode.

> **Terminology:** To disambiguate, when talking about a thread with all three
> modes, we'll refer to the non-restricted user mode as Normal Mode.

Conceptually, restricted mode sits below normal mode.  Just as the code
executing in kernel mode acts as the supervisor to a user mode program, the code
executing in normal mode acts as a guest kernel, or user mode supervisor, to a
restricted mode program.

<!-- diagram 2 source: https://docs.google.com/drawings/d/1tmiHqi8HjVrUdtw-xuWLEtq7taB9bxrQHDhABAqBF7s/edit?usp=sharing&resourcekey=0-WKDGRameQzJLQX894KRwEg -->
![Restricted mode below normal mode][diagram-2-restricted-mode]

#### Isolation

A key feature of restricted mode is isolation.  Isolation is achieved by
changing the accessible address space range and register state when switching
between normal and restricted modes.

Typically, a process's address space is split in two, with one half (the kernel
address space) accessible only when the thread is in kernel mode and the other
half (the user address space) accessible at all times.  With Restricted Mode,
the user address space is further divided to provide a total of three regions.

The uppermost region is only accessible when the thread is in kernel mode.  The
middle region is only accessible from kernel mode or normal.  And the lower
region is accessible at all times (more on this in the Direct Access section).
This lower region is referred to as the restricted region because it is the only
region a thread in restricted mode can access.

Upon entering restricted mode, the Zircon kernel will load general purpose
registers with values provided by normal mode.  Clearing or loading registers
beyond the general purpose registers is the responsibility of normal mode code.
For example, the Zircon kernel will not clear or load floating point or vector
registers.  x86's `fs.base` and `gs.base`, arm64's `tpidr_el0`, and riscv64's
`tp` are all are considered to be general purpose registers.  See section
[Enter](#Enter) for details.

Once a thread has entered restricted mode, it only has access to its register
state and the mappings in the restricted region.

When in restricted mode, the Zircon kernel will route all syscalls to normal
mode, regardless of whether they were issued from the Zircon vDSO or not (more
on this in the next section).  This means that even if the vDSO were mapped into
restricted mode, the syscalls will be redirected to normal mode and not handled
by the Zircon kernel.

<!-- diagram 3 source: https://docs.google.com/drawings/d/1MplskpWSE-td7qW5SMRfgn6qCVCK5u-DljKKLwfvsdU/edit?usp=sharing&resourcekey=0-4ei5CJx4fSYruKKikf8HUg -->
![Address space accessibility by mode][diagram-3-accessibility]

#### Mode switching and interception

This section describes how a thread transitions between normal mode
and restricted mode.

##### Communicating mode state on transition

In order to improve performance, the Zircon kernel and the guest kernel use
shared memory, the mode state VMO, to communicate register state and other
details about a thread when it transitions between normal mode and restricted
mode.  See [Prepare](#Prepare) for details.

##### Entering restricted mode

The Zircon kernel keeps track of whether a thread is in restricted mode or
normal mode using a flag on the kernel's thread structure.  By knowing whether a
thread is in restricted or normal mode, the kernel can correctly "route"
syscalls, faults, and exceptions to the right place (to the kernel or to normal
mode).

Entering restricted mode from normal mode is performed with a new dedicated
syscall ([zx_thread_enter_restricted](#Enter)).  Before calling, the user mode
supervisor first fills in a struct containing the register values to be used in
restricted mode.  The struct is located in the mode state VMO.

Entering consists of changing the thread's accessible address space from the
process's full address space to just the process's restricted region (the lower
half of the user address space), loading the register state from the mode state
VMO, and simply resuming execution (think branching to the restored PC).

In order to improve performance, the Zircon kernel does not automatically save
the thread's normal mode register state.  It's up to the caller to save the
state they care about.  Furthermore, the Zircon kernel does not attempt to
restore any restricted mode register state other than the general purpose
(integer) registers.  Restoring more is up to the caller.  For exactly what is
included in the general purpose registers, see `zx_restricted_state_t` in the
[Enter](#Enter) section.

##### Leaving restricted mode

Before discussing how a thread may leave restricted mode and return to normal
mode, let's establish some context by briefly discussing how three kinds of
thread events are handled when they occur during restricted mode execution:

*Interrupts and resolvable faults* - Interrupts and resolvable faults do not
cause a thread to leave restricted mode.  Hardware and software interrupts, and
resolvable page faults that occur when a thread is in restricted mode are
essentially invisible to the thread (including its user mode supervisor).  For
example, say the CPU's platform timer fires and the CPU enters kernel mode.  Or
a device interrupt fires.  Or the CPU takes a page fault that is resolved by the
Zircon kernel.  None of these events will be observed by the thread (normal mode
code or restricted mode code).  Threads in restricted mode are fully preemptible
in the same way they would be when running in normal mode.

*Task suspend* - Task suspend operations do not cause a thread to leave
restricted mode.  Restricted mode provides no "protection" against suspend
operations (like `zx_task_suspend`).  If a thread happens to be in restricted
mode when the suspend signal arrives, it will be suspended in restricted mode
and upon resuming, will continue execution in restricted mode as if nothing
happened.

*Debugger exceptions* - If there is a debugger attached and a thread executing
in restricted mode generates an exception (e.g. hits a breakpoint) a Zircon
debug exception is generated as usual without forcing the thread out of
restricted mode.  However, if no debugger is attached, the exception will cause
the thread to leave restricted mode where it can be handled by the user mode
supervisor.  See also [Leaving by exception](#Leaving-by-exception).

With that context out of the way, let's discuss how a thread leaves restricted
mode and returns to normal mode.  There are two ways, leaving by syscall and
leaving by exception.  Common to all, the Zircon kernel will provide a copy of
the restricted general purpose register state and a reason code indicating why
the thread returned.  Depending on the reason for leaving restricted mode,
additional information may be provided by the Zircon kernel.  This state will be
written to the mode state VMO prior to the thread returning to normal mode.

##### Leaving by syscall

Executing a syscall instruction while in restricted mode will force the thread
to return to normal mode so that the syscall may be handled by the user mode
supervisor.

When the syscall instruction is executed, the CPU will first trap into kernel
mode.  The usual syscall path in the kernel will test if the thread is in
restricted mode.  If so, it will take an alternate path that involves saving the
restricted general purpose register state and directly returning back to normal
mode to an address passed in the original syscall.  Normal mode must then
recover its saved register state and then process the syscall.  Logically,
control will pass from restricted mode to normal mode, however, it will actually
bounce through kernel mode on each leg of the journey.  Here's an example:

<!-- diagram 4 source: https://docs.google.com/drawings/d/15SJlj54qafTp2Pe08EumpG7EyoVsLY3EKT7q7sv0LpI/edit?usp=sharing&resourcekey=0--x3PTbIQdlwl6iEHeyQyHQ -->
![Restricted mode transitions][diagram-4-transitions]

1. restricted mode issues a syscall
1. syscall came from restricted mode so kernel returns to normal mode
to allow it to handle the call
1. normal mode handles the call and (logically) returns to restricted
mode via syscall
1. kernel handles "enter restricted mode syscall" and jumps back to restricted
mode

As mentioned earlier, the saving and restoring of register state performed by
the Zircon kernel is kept to the bare minimum to improve performance by allowing
normal mode to save/restore only what it needs to.  The cost floor of
transitions to/from restricted mode depends on the architecture.  For example,
on x86 we need to swap the page table root (`CR3`), while on arm64 we can
perform a cheaper "mask/unmask" operation on the normal half of the address
space using `TCR_EL1.T0SZ`, thereby preserving more TLB entries common to both
normal and restricted mode.

##### Leaving by exception

As mentioned earlier, when executing in restricted mode, resolvable page faults
will be transparently handled by the Zircon kernel.  That is, while the thread
may actually enter kernel mode to resolve the fault, it won't return to normal
mode.  From the process's perspective it'll be as if the fault never occurred.

However, when no debugger is present, handling architectural exceptions,
including unhandled page faults, is the responsibility of the user mode
supervisor and will not be handled by the Zircon kernel.  For example, if while
running in restricted mode, the thread accesses an address for which there is no
active mapping, a Zircon exception will be generated and the thread will return
to normal mode.  From the perspective of the Zircon exception handling
mechanism, returning to normal mode "handles the exception" and no further
exception handlers (e.g. thread/process/job handler) will be consulted.  A
standard `zx_exception_report_t` object will be generated and placed in the mode
state VMO so that upon return, the user mode supervisor will have the exception
details.

##### Leaving via kick

In order to support implementing features like graceful task killing and POSIX
signals, we add a new Zircon syscall, `zx_thread_kick_restricted`, that forces a
thread in restricted mode to return to normal mode.  Kick is a latching
operation in that if a thread is kicked while in normal mode, its next attempt
to enter restricted mode will cause it to immediately return with a status code
indicating that it was "kicked out", `ZX_RESTRICTED_REASON_KICK`.  Returning
this status code effectively unlatches the kick.

Kicks do not "stack".  For example, say the target thread is in normal mode and
another thread issues five kicks.  When the target thread enters restricted mode
it will immediately return effectively handling all five kick operations.

In general, callers of `zx_thread_enter_restricted` should be written to handle
spurious kicks.


### Element 2: direct access

Since OS kernels frequently need to access user mode data it's important it can
be done efficiently.  It's common for an OS kernel to have direct access to the
code and data of its user processes.  For example, both the Linux kernel and the
Zircon kernel have direct access to the address space of a user process.

Direct access is a key element of this design.

As mentioned earlier, when in normal mode, a Zircon thread has access to its
process's full address space and when in restricted mode, it has access to only
a subset of the address space.  So when running in normal mode a Zircon thread
can act as the guest kernel and will have access to the guest user code and data
that resides in the restricted region.

The user address space is effectively split in two.  By dividing the address
space in two, we can leverage some significant address space switching
efficiencies on some architectures (i.e. masking half instead of actually
switching, `TCR_EL1.T0SZ`).

Because programs running under Starnix may require that their address space be
"near" the zero address (think `MAP_32BIT`), we chose to make the lowest region
the restricted region.

This approach has some limitations.  The main one is that because the restricted
region is a contiguous subset of the full region, there must be no conflicting
mappings.  The addresses used for the guest code and data must be disjoint from
the addresses used for the user mode supervisor's code and data.  Since the
restricted region is in the "lower half" of user address space, Fuchsia runtimes
used to implement the user mode supervisor must be flexible and not require any
of their mappings to be in the lower half.  To accomplish this, we had to
make some changes to the Fuchsia process builder.  Sanitizers sometimes have
additional requirements on location of mappings so future work may be required
to support certain sanitizers.

### Element 3: shared state

Kernels often have data structures that are shared among user tasks and
manipulated by system calls.  For such systems the kernel programming model
resembles a multi-threaded program where each user task is a thread with access
to a shared (kernel) address space and access to some private,
user-task-specific region.  The code and data in the shared region forms the
shared system image.

In order to make it natural to develop a user space kernel with a shared system
image, we will expand the Zircon process model to support a new kind of process
that shares a portion of its address space with other processes.  A collection
of these processes will host both guest user tasks and the guest kernel.

Earlier, we described how a process's address space can be masked, full
vs. restricted.  The non-restricted region will be shared among all Zircon
processes that are hosting guest tasks of the same system image.  Each process
will have its own independent restricted region that is private to that process.
So when a thread in one of these processes is executing in normal mode it will
have access to the full address space including the shared mappings as well as
its own private mappings in the restricted region.

In addition to sharing these non-restricted mappings, the Zircon processes
hosting guest tasks will all share a single handle table and a single futex
context.  Any handle values (think `zx_handle_t`) in the shared region will be
usable by any of the processes without the need for explicit handle duplication
or transfer operations.  By sharing (some) mappings, handles, and futex context,
a set of processes will look and feel more like a set of threads within a single
process, simplify the programming model.

When accessing mappings in either the shared or restricted regions, care must be
taken to avoid creating data races just as you would when accessing memory in a
multi-threaded process.

While threads in a shared process group have distinct process identities, they
all share the same handle table, futex context, and shared region of address
space.  One implication of this approach is that, for Starnix, a normal mode
process crash is similar to a kernel panic because the crashing process may have
left some shared data in an inconsistent state.

Of course, a restricted mode crash is very different because restricted mode
cannot access normal mode state.  Since the code executing in normal mode is
acting as the supervisor of the restricted mode code, in most cases (like
Starnix) it is expected to gracefully handle restricted mode "crashes" like
architectural exceptions and unhandled page faults.

## Implementation

We first built out x86 support since that's what Starnix initially targeted.
Later we added arm64 and riscv64 support.

While the design and implementation are focused on the Starnix use case, the
features are developed in way they can be used (and tested) completely
independent of Starnix.

New syscalls have been added to the `@next` vDSO.

### Syscall interface and semantics

#### Process creation

In order to enter restricted mode, a thread must be part of a "shared process"
and have a restricted region of address space.  A shared process is one that,
optionally, shares a portion of its address space (the shared region), its
handle table, and futex context, with other processes in its group.  We say
optionally because a shared process can exist in a group of one.

Whether a process is a regular process or a shared one is determined when the
process is created.  To create a shared process we extend the existing
`zx_process_create` syscall and introduce a new one, `zx_process_create_shared`.

The existing `zx_process_create` syscall is changed to accept a new option,
`ZX_PROCESS_SHARED`.  When present, the resulting process's address space, as
modeled by the returned VMAR, will only cover the shared region (the upper
half).  A process created with `ZX_PROCESS_SHARED` acts as a prototype for
creating a set of processes that all share a portion of their address space.

A new syscall, `zx_process_create_shared` is added and will work in conjunction
with the `ZX_PROCESS_SHARED` option.

```
zx_process_create_shared(zx_handle_t shared_proc,
                         uint32_t options,
                         const char* name,
                         size_t name_size,
                         zx_handle_t* proc_handle,
                         zx_handle_t* restricted_vmar_handle);
```

This new call is similar to `zx_process_create`.  The ability to create a
process via `zx_process_create_shared` is governed by the caller's job policy
and the `shared_proc` argument.

`zx_process_create_shared` creates a new process, `proc_handle`, and a new VMAR,
`restricted_vmar_handle`.  The new process's address space consists of two
regions, shared and private, backed to two separate VMARs.  The shared region is
backed by the existing VMAR returned by the earlier `ZX_PROCESS_SHARED` create
call.  The private region is backed by the newly created VMAR and is returned
via `restricted_vmar_handle`.

In this way the Starnix runner can create a collection of Starnix processes that
share mappings in half of their address space but each have an independent lower
half to host restricted mode code and data.

#### Entering/leaving restricted mode

Only threads in a shared process with a restricted region may enter restricted
mode.

Prior to entering, a thread must prepare by creating and binding a mode state
VMO to itself using `zx_thread_prepare_restricted`.  The mode state VMO will be
used by the thread and the Zircon kernel to save/restore restricted mode general
purpose register state.  The VMO is retained by the thread (and any explicitly
created user mappings) until the thread terminates.

After a successful prepare call, the thread may enter restricted mode using
`zx_thread_enter_restricted`.  Upon leaving restricted mode, the thread may
re-enter without needing to re-prepare, regardless of whether it left for
`ZX_RESTRICTED_REASON_SYSCALL`, `ZX_RESTRICTED_REASON_EXCEPTION`, or
`ZX_RESTRICTED_REASON_KICK` (`zx_thread_kick_restricted`).

##### Prepare

```c
zx_status_t zx_thread_prepare_restricted(uint32_t options, zx_handle_t* out_vmo);
```

Prepare creates and bind a mode state VMO to the current thread.  The resulting
VMO will be used to save/restore restricted mode state upon entering/leaving
restricted mode.

The returned VMO is similar to one created with `zx_vmo_create` and the
resulting handle has the same rights as it would if the VMO were created by
`zx_vmo_create` with no options.  Note, decommitting, resizing, or creating
child VMOs are not supported.  Mapping, unmapping, and reading/writing via
`zx_vmo_read`/`zx_vmo_write` are supported.

Only one mode state VMO may be bound to a thread at a time.  Attempting to bind
another one will replace the already bound VMO.  A bound VMO will be destroyed
only after the last user handle is closed, the last user mapping is removed, and
either it is replaced by another call to `zx_thread_restricted_prepare` or the
thread terminates.  Like any other VMO, once the VMO has been mapped it will be
retained by its mapping so the caller may close the handle and access the memory
directly via the mapping.

The caller's job policy must allow `ZX_POL_NEW_VMO`.

This call may fail with:

* `ZX_ERR_INVALID_ARGS` - `out_vmo` is an invalid pointer or NULL, or `options`
  is any value other than 0.
* `ZX_ERR_NO_MEMORY`  - Failure due to lack of memory.

> **Note:** If a handle to the newly created VMO cannot be returned because
> `out_vmo` is an invalid pointer, the VMO may still be bound to the thread even
> when the call returns `ZX_ERR_INVALID_ARGS`.  A policy exception
> (`ZX_EXCP_POLICY_CODE_HANDLE_LEAK`) will be generated.  Assuming the exception
> is handled, a caller can recover from this state by calling prepare again with
> a valid `out_vmo`.

##### Enter

```c
status_t zx_thread_enter_restricted(uint32_t options,
                                    uintptr_t return_vector,
                                    uintptr_t context);
```

Enters restricted mode by changing the calling thread's accessible address space
to the restricted region of its shared process and loading the register state
contained in the mode switch VMO.  Prior to calling, the thread must have
prepared a mode state VMO using `zx_thread_prepare_restricted` and populated it
with a `zx_restricted_state_t` struct at offset zero.

Unlike a normal function call, a successful `zx_thread_enter_restricted` call
does not return to the calling context.  Instead, upon leaving restricted mode
the thread will simply jump to the address specified in `return_vector` with
`context` and a reason code loaded in two general purpose registers.  All other
registers, including the stack pointer, will be in an unspecified state.  It is
the responsibility of the normal mode code at `return_vector` to reconstruct the
pre-syscall state using `context`, the reason code, and the content of the mode
state VMO.

The possible reason codes are, `ZX_RESTRICTED_REASON_SYSCALL`,
`ZX_RESTRICTED_REASON_EXCEPTION`, and `ZX_RESTRICTED_REASON_KICK`.  Upon return,
offset 0 of the mode state VMO will contain, depending on the reason code, one
of the following structs: `zx_restricted_syscall_t`,
`zx_restricted_exception_t`, or `zx_restricted_kick_t`.

On x64, `context` is placed in `rdi` and a reason code is placed in `rsi`.

On arm64, `context` is placed in `x0` and a reason code is placed in `x1`.

On riscv64, `context` is placed in `a0` and a reason code is placed in `a1`.

A failed call may return:

* `ZX_ERR_INVALID_ARGS` - `return_vector` is not a valid user address or options
  is not valid.
* `ZX_ERR_BAD_STATE` - the restricted mode register state in the mode state VMO
  is not valid or there is no mode state VMO bound to the calling thread.

The definitions of the structs are as follows:

```
typedef struct zx_restricted_syscall {
  zx_restricted_state_t state;
} zx_restricted_syscall_t;

typedef struct zx_restricted_exception {
  zx_restricted_state_t state;
  zx_exception_report_t exception;
} zx_restricted_exception_t;

typedef struct zx_restricted_kick {
  zx_restricted_state_t state;
} zx_restricted_kick_t;
```

Notice that the first element of all three structs is a `zx_restricted_state_t`
object.  Upon return, this state object will contain the restricted mode general
purpose register state at the time the thread return to normal mode.  Saving any
other restricted register state (e.g. debug or FPU/vector registers) is the
responsibility of the normal mode at `return_vector`.

The restricted mode general purpose register state, `zx_restricted_state_t`, is
defined as follows:

On arm64,

```
typedef struct zx_restricted_state {
  uint64_t x[31];
  uint64_t sp;
  uint64_t pc;
  uint64_t tpidr_el0;
  // Contains only the user-controllable upper 4-bits (NZCV).
  uint32_t cpsr;
  uint8_t padding1[4];
} zx_restricted_state_t;
```

On x64,

```
typedef struct zx_restricted_state {
  uint64_t rdi, rsi, rbp, rbx, rdx, rcx, rax, rsp;
  uint64_t r8, r9, r10, r11, r12, r13, r14, r15;
  uint64_t ip, flags;
  uint64_t fs_base, gs_base;
} zx_restricted_state_t;
```

On riscv64, it's simply a typedef to `zx_riscv64_thread_state_general_regs_t`.

```
typedef zx_riscv64_thread_state_general_regs_t zx_restricted_state_t;
```

Where `zx_riscv64_thread_state_general_regs_t` is,

```
typedef struct zx_riscv64_thread_state_general_regs {
  uint64_t pc;
  uint64_t ra;   // x1
  uint64_t sp;   // x2
  uint64_t gp;   // x3
  uint64_t tp;   // x4
  uint64_t t0;   // x5
  uint64_t t1;   // x6
  uint64_t t2;   // x7
  uint64_t s0;   // x8
  uint64_t s1;   // x9
  uint64_t a0;   // x10
  uint64_t a1;   // x11
  uint64_t a2;   // x12
  uint64_t a3;   // x13
  uint64_t a4;   // x14
  uint64_t a5;   // x15
  uint64_t a6;   // x16
  uint64_t a7;   // x17
  uint64_t s2;   // x18
  uint64_t s3;   // x19
  uint64_t s4;   // x20
  uint64_t s5;   // x21
  uint64_t s6;   // x22
  uint64_t s7;   // x23
  uint64_t s8;   // x24
  uint64_t s9;   // x25
  uint64_t s10;  // x26
  uint64_t s11;  // x27
  uint64_t t3;   // x28
  uint64_t t4;   // x29
  uint64_t t5;   // x30
  uint64_t t6;   // x31
} zx_riscv64_thread_state_general_regs_t;
```

##### Kick

```c
zx_status_t zx_thread_kick_restricted(zx_handle_t thread, uint32_t options);
```

Kicks a thread out of restricted mode if it is currently running in restricted
mode or saves a pending kick if it is not.  If the target thread is running in
restricted mode, it will exit to normal mode through the `return_vector`
provided to `zx_thread_enter_restricted` with a reason code set to
`ZX_RESTRICTED_REASON_KICK`.  Otherwise, the next call to
`zx_thread_enter_restricted` will not enter restricted mode and will instead
dispatch to the provided entry point with reason code
`ZX_RESTRICTED_REASON_KICK`.

Multiple kicks on the same thread object are collapsed together.  Thus if
multiple threads call `zx_thread_kick_restricted` on the same target while it is
running or entering restricted mode, at least one but possibly multiple
`ZX_RESTRICTED_REASON_KICK` returns will be observed.  The recommended way to
use this syscall is to first record a reason for kicking in a synchronized data
structure and then call `zx_thread_kick_restricted`.  The thread calling
`zx_thread_enter_restricted` should consult this data structure whenever it
observes `ZX_RESTRICTED_REASON_KICK` and process any pending state before
reentering restricted mode.

The `thread` handle must have the right `ZX_RIGHT_MANAGE_THREAD`.

This call may fail:

* `ZX_ERR_WRONG_TYPE` - `thread` is not a thread.
* `ZX_ERR_ACCESS_DENIED` - `thread` does not have ZX_RIGHT_MANAGE_THREAD.
* `ZX_ERR_BAD_STATE` - `thread` is dead.

#### Other syscall changes

VMAR oriented syscalls - Syscalls like `zx_vmar_map` and `zx_vmar_op_range` are
unaffected by this proposal.  While not a syscall, `zx_vmar_root_self` is
affected by this proposal.  See the discussion in the subsequent section for
details.

`zx_process_read_memory` and `zx_process_write_memory` - These syscall take a
process handle and provide access to the process's memory.  Currently (prior to
this proposal), these calls will not cross a mapping boundary.  That is, if you
ask to read an 8KiB region that spans two mappings, you'll only get back the
first 4KiB.  Under this proposal, the API and semantics of these call will not
change.  A small change to the implementation is needed to perform the
`FindRegion` call on the appropriate `VmAspace` (the shared one or the private
one) depending on the supplied `vaddr`.  Callers will continue to be able to
read and write the process's full address space including both shared and
private regions.

`ZX_INFO_TASK_STATS` and `ZX_INFO_PROCESS_MAPS` - The implementation is updated
to include both normal mode and restricted mode mappings, however, the API
remains unchanged.  That is, the results will not differentiate between normal
and restricted.

#### Other changes

`zx_vmar_root_self` - One of the biggest semantic changes of this proposal is
that processes created via `zx_process_create_shared` will not have a single
root VMAR that encapsulates their entire address space.  Instead, they will have
two VMARs.  One for the shared region and one for the private region.  Whether
and how this is semantic changed is exposed to programs is left up to the
language runtime.  `zx_vmar_root_self` is used in the C, C++, and Rust runtimes.
Under this proposal, `zx_vmar_root_self` behaves more like a "get default vmar",
returning the shared VMAR so that existing libraries (e.g. scudo) will operate
on the shared region "by default".

`zx_process_self` - Because `zx_process_self` returns the value of a global
variable, and in a shared process that global variable is backed by the shared
region, it's not really returning a handle to self.  Instead, it returns the
handle of the first shared process in the group (the prototype process).  We
explored the idea of changing `zx_process_self` to use thread local storage
instead of a global variable, but ultimately decided to punt for now (see
[https://fxbug.dev/352827964]) and instead change problematic uses of
`zx_process_self`.  See below.

`pthread_create` - This is the most interesting case of a problematic use of
`zx_process_self`.  In order to ensure `pthread_create` creates threads in the
right process, it needs a handle to the calling process.  To resolve this, we
expanded the `pthread` struct to include the process handle and changed
`pthread_create` to use the handle from the struct rather than the one returned
by `zx_process_self`.

Debuggers - Debuggers (e.g. zxdb and fidlcat) may need to change to understand
that the active mappings for a thread are not necessarily the same for all
threads in its process.  In other words, we may need to create a way for a
debugger to query a suspended thread for its address space.

Fuchsia process builder - Minor changes to the process builder are necessary
since we've changed some assumptions about which parts of a process's
address space are accessible.  For example, the prototype process won't have a
lower half.

Starting the Starnix component - the Starnix component is started by Component
Manager.  Like any ELF component, `zx_process_create` is used to create the
first process in the galaxy.  Unlike other ELF components, the first Starnix
process's VMAR must be a shared VMAR.  To accomplish this, we define a new
component manifest option, `is_shared_process`, that instructs Component Manager
to pass `ZX_PROCESS_SHARED`.  This new process acts as a sort of prototype and
is responsible for creating the subsequent processes in the galaxy.  This
component manifest option minimizes changes to Component Manager, while avoiding
the need for Starnix itself to get into the business of Fuchsia process
building.  Each process created by the prototype simply shares the prototype's
code, data, and handles, as is.

## Performance

### Minimal impact to existing syscalls

There may be a small, but non-zero, performance impact to certain syscalls.  The
Zircon microbenchmark suite will be used to ensure any impact is negligible.  In
addition to runtime performance impact, some kernel objects may grow slightly,
resulting in a small increase in memory usage.  As with runtime performance, we
anticipate the increase in memory usage to be negligible, but will measure and
verify.

#### Mode switching performance

The mode switching performance of restricted mode will be more expensive than
native mode switching because the CPU must actually transit through EL1 on its
way between normal and restricted modes.  However, we don't anticipate this cost
being significant in the context of Starnix.

New Zircon microbenchmarks will be added to measure and track the mode switching
cost.

## Security considerations

This mechanism is intended to act as a security boundary and will need to be
thoroughly reviewed by the Fuchsia Security team.

### General

A user mode program making use of these new Zircon facilities will itself be
acting as a kernel so it must take care to guard its own data and execution
against the code executing in restricted mode in much the same way the Zircon
kernel guards against buggy or possibly malicious user mode Fuchsia code.

While discussed in detail elsewhere in this document, it's worth highlighting
some key points:

* Shared handles - All processes in a shared process group use a single handle
table, so any thread executing in normal mode, in any of the processes in the
group, may use any handle in the table.
* Shared mappings - All processes in a shared process group share a portion of
their address space with one another (the shared VMAR).  The mappings in this
shared region are accessible to all threads in the group when executing in
normal mode.
* Restricted region accessible in normal mode - When executing in normal mode,
the process's restricted region is accessible.  Because this region contains
untrusted code and data, care must be taken to never trust any code or data in
the restricted region (e.g. copy out and then verify to avoid TOCTOUs).
* Must clear/restore on entry - When entering restricted mode, the program must
take care to clear, restore, or otherwise replace register state beyond what is
specified by `zx_restricted_state_t`.  This includes floating-point, vector, and
debug register state.  Failure to do so can leak data to untrusted code, or
worse.
* Must clear/restore on return - When returning to normal mode, the program must
take care to clear, restore, or otherwise replace register state beyond what is
specified by `zx_restricted_state_t`.  Failure to do so could allow untrusted
code to influence or control execution of the user mode supervisor, leading to
an escape.

### Speculative execution vulnerabilities

A user mode kernel running untrusted guest code needs to defend against
speculative execution attacks just like the Zircon kernel does.  However,
because every transition to or from restricted mode involves a trip through the
Zircon kernel, we have an opportunity to make the guest kernel simpler and
potentially agnostic of some speculative execution vulnerabilities.  The idea is
that the guest kernel can lean on the speculative execution mitigations
performed by the Zircon kernel at kernel entry and exit points.  Of course, this
strategy will need to be reviewed by the Fuchsia Security team and owners of
guest kernels will need to be kept in the loop for each newly discovered
speculative execution vulnerability.

### Limited access

When in restricted mode, a thread only has access to the restricted address
space and cannot issue Zircon syscalls.  There are two mechanisms that prevent
a restricted mode thread from issuing a Zircon syscall.  First, the syscall
interception implementation will route all syscalls to normal mode.  Second,
even if a syscall were routed to the Zircon kernel, the lack of a Zircon vDSO
in the restricted region would prevent it from being serviced.

It's worth pointing out that prior to entering restricted mode, the caller
should take care to not inadvertently leak information "into" restricted mode
via registers.

Of course, if code in restricted mode is somehow able to compromise the normal
mode code in its same process (e.g. via bug in the normal mode code), then it
might also be able to compromise all the Zircon processes that form the shared
system image because they all share some of their code and data via shared
mappings.

### Supervisor mode access and execution protection

x86's SMAP and SMEP, and arm64's PAN and PXN are hardware features that place
limits on what mappings kernel code can access or execute.  They are designed to
make it more difficult to exploit kernel bugs.  Because restricted mode is a
software construct and because the guest kernel runs in user mode, we cannot
directly leverage these features in a user mode supervisor.  See also the
discussion of alternate mappings in [Alternatives](#Alternatives).

## Privacy considerations

These new kernel features themselves do not have privacy considerations any more
than any other kernel features.  However, the application of these features in
order to run Linux binaries on a Fuchsia-based product may require privacy
review in the future.

Once we have a specific, end-to-end product use case that involves Linux
binaries, we will need to evaluate the privacy implications of that use case.

## Future directions

There are several aspects of this design that will likely change over time.
This section describes some of the aspects we are likely to revisit in the near
future.  Future documents will describe them in greater detail.

### API decomposition

This proposal integrates multiple features behind APIs focused on the Starnix
use case.  For example, in its current form, a thread must live in a shared
process in order to restrict its address space.  While it's fine for Starnix,
this coupling limits the applicability of this system.  Once we have more use
cases for this technology, we should explore decomposing the API and feature set
into more independent building blocks.

### One root VMAR

Related to the note about Decomposition, in a future proposal, we should revisit
the notion that a process may have multiple root VMARs.  Under the current
proposal, the shared and private VMARs are both considered root VMARs because
neither one is contained by another VMAR.  This multi-root approach is a
pragmatic design decision motivated by the existing implementations of
`VmAspace`, `VmAddressRegion`, and `VmMapping` classes.  Redesigning/refactoring
those classes would allow us to eliminate the need for a process to have more
than one root VMAR (though we may ultimately want to support multiple roots for
other reasons).

### Improved register saving

One area we may need to improve is how FPU/vector register state is saved,
restored, or zeroed on entering/leaving restricted mode.

Starnix code, or one of its dependencies, might make use of FPU/vector registers
(think memcpy) so Starnix must take care to save/restore the restricted
FPU/vector state.  Currently, Starnix saves/restores all FPU/vector state on
each restricted entry and exit.  In order to improve performance, we may want to
compile some of the Starnix logic that runs on restricted exits to defer or
reduce the set of registers that need to be saved.

Even thought Starnix takes care to save/restore FPU/vector register state,
technically, the Zircon vDSO could clobber the state that the caller has
established just prior to entering restricted mode (see
[https://fxbug.dev/42062491]).  This fragility is currently mitigated by tests
that verify restricted mode FPU/vector register state.

Additionally, we might be able to improve performance in some cases by providing
special syscalls that take advantage of privileged save/restore instructions
available on some architectures, or perhaps make use of a hardware-assisted lazy
save/restore scheme.

## Testing

As with other aspects of the kernel interface, core-tests and in-kernel unit
tests will be implemented to verify correctness. Zircon microbenchmarks will be
used to measure performance.

## Documentation

In-tree documentation for new and modified syscalls will be added/updated.

In-tree documentation for Zircon exceptions will be updated.

An in-tree overview doc will be written describing this new mechanism.

## Drawbacks, alternatives, and unknowns

### Drawbacks

These changes affect several kernel components and increase the
complexity of the kernel.  While the design strives to compartmentalize and
minimize this new complexity, this is still a non-zero cost.

### Alternatives

*Hypervisor* - An alternative to this software-only design is to leverage the
hardware's hypervisor features to allow a "guest kernel" to issue Zircon
syscalls directly.  Sort of like an extreme version of paravirtualization.  See
[RFC: Direct Mode for Virtualization][direct-mode] for details.  One reason to
pursue a software-only approach is to ensure we can apply it on architectures
and/or devices where we do not have access to hardware assisted virtualization.

*All in one process* - The design presented models each guest process with one
Zircon process.  One nice property of this approach is that each guest process
is "visible" to Zircon tools like `ps` or `kill`.  An alternative design would
model each guest process as a collection of threads within a single Zircon
process.  This alternative design would "hide" guest processes from the rest of
a Zircon user mode, but is otherwise not all that different.

*Supervisor-supervisor isolation* - An alternative to the proposed design
involves eliminating the shared region and requiring that each supervisor
process coordinate with each other via IPC or some "manually managed" explicit
mappings.  This alternative has the advantage of making it more difficult to
compromise one process using another (improved isolation), but would make it
more difficult to implement many aspects of a guest kernel (see [Goals](#Goals),
"Natural and simplified programming model").  The coordination overhead across
process would also likely impact performance.

*Guest and supervisor in separate processes* - An alternative to introducing a
new mode is to put the untrusted code into a separate process that has no Zircon
vDSO.  A guest syscall would then consist of a context switch from the untrusted
process to the Zircon process implementing the guest kernel.  This approach
would work fine, however, it would likely require a "more complete" context
switch for each syscall and be more expensive in terms of runtime performance.
Additionally, it would likely result in needing two Zircon threads for each
guest task as opposed to restricted mode's one Zircon thread for each guest task
(one Zircon thread to run the guest task and one to implement syscalls on its
behalf).  Compared with restricted mode, using a separate process for the
untrusted code/data has some performance disadvantages when it comes to IPC and
context switching between the guest and the supervisor.

*Dedicated read/write vs. mode state VMO* - Earlier iterations of this design
used dedicated syscalls for the purpose of accessing mode state
(`zx_restricted_state_read` and `zx_restricted_state_write`).  However, those
quickly became a performance issue so they were eliminated in favor of the mode
state VMO.

*Unbind* - In a previous iteration, there was an explicit unbind syscall that
would "undo" the bind operation performed by `zx_thread_prepare_restricted`.  We
found that unbind was unnecessary and only used in some test code.  We removed
it to simplify the API.

*Alternate mappings for access and execution protection* - The set of exploit
mitigation tools available to a user mode supervisor differs from what's
available to a kernel mode supervisor.  Generally speaking, both can make use of
various software based techniques (like CFI), but user mode supervisors cannot
make use of hardware features like x86's SMAP or SMEP, or arm64's PAN or PXN.
These hardware features provide a way to mark which mappings may be accessed in
different contexts when executing in kernel mode.  There are some potential
alternatives to SMEP/PXN and SMAP/PAN that could be further explored if
necessary.  The idea is to maintain a set of alternate mappings with different
protection bits and transitioning between these mappings when switching between
normal and restricted modes, or when performing a "user copy" operation between
the shared and restricted regions.  Of course, there are some significant
performance and complexity costs in maintaining and using alternate mappings so
this alternative may or may not prove viable.

### Unknowns

Changes to core kernel object relationships - This proposal changes some core
kernel object relationships.  For example, it creates a one-to-many relationship
between handle table and process, processes no longer have exactly one address
space (or root VMAR), futexes can be shared between processes, a given handle
may "exist in multiple processes" simultaneously, etc.  There is a risk that
these changes create a latent design flaw.

Debugging and diagnostics - This proposal impacts the model for debuggers and
other process-centric tools like the memory-monitor (`fx mem`).  We'll need to
work with the owners of zxdb and memory-monitor to think through the
implications.  Related, we'll need to work through the details of existing
syscalls that assume a process has exactly one active address space
(e.g. `zx_process_read_memory`)

## Prior art and references

See [RFC-0082: Running unmodified Linux programs on Fuchsia][rfc-0082] and its
"Prior art and references" section.

This design is based on several Google internal docs and lots of conversations.
See also

* [go/zx-unified-aspace]
* [go/zx-restricted-exceptions]
* [go/zx-restricted-kick]
* [go/zx-restricted-roadmap]
* [go/zx-starnix-faults]
* [go/zx-starnix-state-transitions]
* [go/zx-multi-aspace-processes]
* [go/zx-restricted-mode]

Restricted mode is inspired by ARMv8-A Exception Levels ("what if we had an
EL-minus-1?").

<!-- Links -->

[starnix]: /docs/concepts/components/v2/starnix.md
[rfc-0082]: 0082_starnix.md
[direct-mode]: https://fuchsia-review.googlesource.com/c/fuchsia/+/684845
[diagram-1-mode-switch]: resources/0261_fast_and_efficient_user_space_kernel_emulation/diagram_1_mode_switch.svg
[diagram-2-restricted-mode]: resources/0261_fast_and_efficient_user_space_kernel_emulation/diagram_2_restricted_mode.svg
[diagram-3-accessibility]: resources/0261_fast_and_efficient_user_space_kernel_emulation/diagram_3_accessibility.svg
[diagram-4-transitions]: resources/0261_fast_and_efficient_user_space_kernel_emulation/diagram_4_transitions.svg
[https://fxbug.dev/42062491]: https://fxbug.dev/42062491
[https://fxbug.dev/352827964]: https://fxbug.dev/352827964
[go/zx-unified-aspace]: https://goto.google.com/zx-unified-aspace
[go/zx-restricted-exceptions]: https://goto.google.com/zx-restricted-exceptions
[go/zx-restricted-kick]: https://goto.google.com/zx-restricted-kick
[go/zx-restricted-roadmap]: https://goto.google.com/zx-restricted-roadmap
[go/zx-starnix-faults]: https://goto.google.com/zx-starnix-faults
[go/zx-starnix-state-transitions]: https://goto.google.com/zx-starnix-state-transitions
[go/zx-multi-aspace-processes]: https://goto.google.com/zx-multi-aspace-processes
[go/zx-restricted-mode]: https://goto.google.com/zx-restricted-mode
