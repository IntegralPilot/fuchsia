<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0264" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Problem Statement

[Secure and efficient](0261_fast_and_efficient_user_space_kernel_emulation.md)
[foreign ABI support](0082_starnix.md) enables AArch64 Linux programs to run
unmodified.  As we have continued to expand the universe of software we wish to
run on Fuchsia, we have encountered 32-bit ARM (AArch32 ISA) Linux programs that
need to be able to run without recompiling or would benefit sufficiently from
running against a AArch32 Linux ABI.

This problem does not expand to enabling any other ABI support across
Fuchsia programs.


## Summary

This document proposes the addition of AArch32 foreign ABI support to run
unmodified 32-bit ARM ISA Linux programs.  Building on Zircon's [efficient user
space kernel emulation](0261_fast_and_efficient_user_space_kernel_emulation.md)
exceptions from a 32-bit AArch32 userspace will be redirected to
[starnix](0082_starnix.md).  Starnix will be responsible for implementing all
AArch32/Linux foreign ABI functionality necessary.

## Stakeholders

This proposal impacts several areas of the project which is reflected in the
reviewers:

* Zircon: As the kernel must safely and sustainably support AArch32 \
exceptions.
* Starnix: As it is responsible for most of the foreign ABI compatibility.
* Toolchain: As this adds a new, but limited, need to compile for \
AArch32 targets.
* Infrastructure and Testing: As this will add new hardware and/or emulation \
permutations.


_Facilitator:_


* hjfreyer@


_Reviewers:_


* maniscalco@
* lindkvist@
* mcgrathr@
* jamesr@
* mvanotti@


_Consulted:_


* travisg@
* phosek@
* abarth@
* tkilbourn@
* olivernewman@
* ajkelly@


_Socialization:_

A technical prototype of this proposal was first pursued to validate its
technical feasibility and enable initial cost assessments.  The prototype was
shared with representatives from the impacted components alongside a draft of
this document.  The prototype implementation enabled a more accurate
assessment of the potential short and long term impact, and the design review
resulted in expanded discussion of the design choices and brought to light a
number of now addressed issues.

## Requirements

There were many potential guiding requirements, but once the problem statement
was selected, the following requirements were applied to the subsequent design:

AArch32 support in Starnix must:


* Be able to parse and load modern armv7-linux binaries
* Be able to manage separate AArch32 system call paths
* Be reasonably contained, not impacting code throughout Starnix
* Be able to be disabled (e.g., via a build-time flag)
* Minimally impact existing AArch64 code paths
* Be able to execute AArch64 and AArch32 programs on the same starnix instance

It is worth noting that while AArch64 and AArch32 software may be executing in
parallel on the same starnix kernel instance, it is a non-goal at this point to
provide behavior guarantees on the interaction between those two environments.

AArch32 support in Zircon must:


* Only function for restricted mode threads
* Allow a shared process to host both AArch32 or AArch64 restricted mode code
* Not create undue long term maintenance burdens
    * Minimally change API surfaces
    * Be easy to remove and/or disable
    * Not impact every task running under Zirecon
    * Not require extensive changes to Zircon

While other Fuchsia-supported architectures may be desirable in the future, the
current set of programs we wish to run target AArch32 environments, such as
[Debian](https://wiki.debian.org/ArmHardFloatPort) or
[Android](https://developer.android.com/ndk/guides/abis#v7a).  It is a non-goal
to provide perfect emulation of armv7-linux, but instead build upon starnix's
API layer to support the exposed processor AArch32 functionality.

## Design

Starnix provides compatibility with the Linux kernel.  The Linux kernel already
supports what is called "compat" mode. When enabled, CONFIG\_COMPAT allows a
64-bit Linux kernel to run 32-bit or 64-bit ISA targeted binaries supported by
the host CPU architecture.  For Arm processors, this is AArch64 and AArch32.

In general, CONFIG\_COMPAT enables 32-bit architectures to be run.  This design
is meant to enable the same behavior on Starnix such that Android on Starnix can
run 32-bit (AArch32) or 64-bit (AArch64) ARM binaries.  Realizing this goal on
Fuchsia looks differently than it does on Linux and requires changes to Zircon
as well as Starnix.  This document will start from the hardware and work its way
up.

### Zircon

Zircon is Fuchsia's kernel and enables Starnix through _restricted mode_.

The approach taken to enable AArch32 support is to (1) determine the
capabilities and functionality of the processor, (2) ensure that restricted mode
can enable AArch32 execution on entry, and (3) ensure that all exceptions, error
or otherwise, are routed to the restricted mode supervisor (starnix) while
minimizing any other impact to Zircon.


#### Processor support

As described earlier, the ARM architecture offers an execution state,
called
[AArch32](https://developer.arm.com/documentation/100048/0002/programmers-model/armv8-a-architecture-concepts/aarch32-execution-modes?lang=en),
which provides backwards compatibility with prior 32-bit ARM ISAs on processors
that are on newer architectural releases (ARMv8+).

The ARM architecture provides a relatively clear path for implementing support
for AArch32 without penalizing modern kernels and enabling a small set of
additional functionality beyond the last 32-bit architectural release, ARMv7.
Each section below will cover the necessary processor functionality to enable
AArch32 execution.


> The remainder of this section largely quotes and paraphrases sections of the ARM
> technical guides and reference manual relevant to the subsequent changes in
> Zircon and Starnix.

##### Detecting AArch32 support

Recent ARM architecture revisions expose a processor feature register,
_[ID\_PFR0\_EL1](https://developer.arm.com/documentation/ddi0595/2020-12/AArch64-Registers/ID-PFR0-EL1--AArch32-Processor-Feature-Register-0)_,
for AArch32 support.  The value in this register determines which AArch32
features are supported, if any.  Many 64-bit ARM chips targeting consumer
devices provide support for A32 (State0) and T32 (State1) instruction sets (as
ARMv7 support).   To support Linux's 32-bit compatibility user space, these are
the two instruction sets necessary.  It's important to note that many
server-oriented 64-bit ARM chips do not support any AArch32 functionality, but
where it is supported, [both sets will be
available](https://developer.arm.com/documentation/100961/1101-00/Introduction/ARMv8-64-bit-architecture-overview).


##### Core Registers Mapping



[AArch32 to AArch64 register
mapping](https://developer.arm.com/documentation/102412/0103/Handling-exceptions/Taking-an-exception):

| AArch32          | AArch64          |
| ---------------- | ---------------- |
| R0-R12           | X0-X12           |
| Banked SP and LR | X13-X23          |
| Banked FIQ       | X24-X30          |


For the most part, the architectural mapping of registers is seamless.  AArch32
has [16 pre-declared core
registers](https://developer.arm.com/documentation/dui0801/a/Overview-of-AArch32-state/Predeclared-core-register-names-in-AArch32-state?lang=en)
while AArch64 has [32 pre-declared core
registers](https://developer.arm.com/documentation/dui0801/a/Overview-of-AArch64-state/Predeclared-core-register-names-in-AArch64-state?lang=en).
The AArch32 registers are mapped to the first 16 registers of the AArch64
general purpose bank: x0-15 are r0-15.  As per the [64-bit architectural
overview of
ARMv8](https://developer.arm.com/documentation/100961/1101-00/Introduction/ARMv8-64-bit-architecture-overview),
when accessing the registers from AArch32, the upper 32-bits will either be
ignored or zeroed:


* The upper 32 bits of the source registers are ignored.
* The upper 32 bits of the destination registers are set to zero.
* Condition flags, where set by the instruction, are computed from the lower
  32 bits.

When intentionally accessing these registers as AArch32 registers from AArch64,
it is common to use W instead of X to avoid stale values.

It is important to note that the [program counter
(PC)](https://developer.arm.com/documentation/dui0801/a/Overview-of-AArch32-state/Program-Counter-in-AArch32-state?lang=en)
is mapped to register R15, the stack pointer (SP) is mapped to R13, and the link
register (LR) is mapped to R14.

Along with the core registers, the current program state register (CPSR) mapping
plays a vital role in AArch32 and AArch64 interactions. Its mapping is covered
in the next section.

##### Current Program State Register (CPSR)

In ARMv7 and earlier, there was [a single
register](https://developer.arm.com/documentation/den0013/d/ARM-Processor-Modes-and-Registers/Registers/Program-Status-Registers),
the current program state register (CPSR), which contained:


* The
    [APSR](https://developer.arm.com/documentation/dui0801/a/Overview-of-AArch32-state/Application-Program-Status-Register?lang=en)
    flags – which are
    * N, Z, C, and V flags.
    * Q (saturation) flag.
    * GE (Greater than or Equal) flags.
* The current processor mode.
* Interrupt disable flags.
* The current processor state, that is, ARM, Thumb, ThumbEE, or Jazelle.
* The endianness.
* Execution state bits for the IT block.

In AArch64, this information is split up and accessed differently (though PSTATE
is essentially CPSR renamed).  However, when an exception is taken, the
information is presented in the [saved program status register
(SPSR)](https://developer.arm.com/documentation/dui0801/a/Overview-of-AArch64-state/Saved-Program-Status-Registers--SPSRs--in-AArch64-state)
which stores:


* N, Z, C, and V flags.
* D, A, I, and F interrupt disable bits.
* The register width.
* The execution mode.
* The IL and SS bits.

Architecturally, the AArch32 CPSR is [mapped to the
SPSR](https://developer.arm.com/documentation/dui0801/a/Overview-of-AArch32-state/Saved-Program-Status-Registers--SPSRs--in-AArch32-state?lang=en)
on exception for AArch64. This leaves the AArch32 CPSR's AArch32-only bits being
stored in reserved regions of the SPSR:


| 31       27 | 26 25 | 24 | 23    20 | 19    16 | 15    10 | 9 | 8 | 7 | 6 | 5 | 4   0 |
| ----------- | ----- | --- | -------- | -------- | -------- | --- | --- | --- | --- | --- | ----- |
|  N Z C V Q  |  IT   | J  | Reserved |    GE    |    IT    | E | A | I | F | T |   M   |
|             | \[1:0\] |    |          |   \[3:0\]  |   \[7:2\]  |   |   |   | |   | \[4:0\] |


In AArch64 state, the IT, Q and GE flags [cannot be read or written
to](https://developer.arm.com/documentation/dui0801/a/Overview-of-AArch32-state/Application-Program-Status-Register?lang=en).
In AArch32, GE flags are set by parallel add and subtract instructions, and Q is
set [to indicate an overflow or
saturation](https://developer.arm.com/documentation/dui0801/l/Overview-of-AArch32-state/The-Q-flag-in-AArch32-state).
The
[IT](https://developer.arm.com/documentation/ddi0406/b/Application-Level-Architecture/Instruction-Details/Alphabetical-list-of-instructions/IT?lang=en)
[bits](https://developer.arm.com/documentation/ddi0406/b/Application-Level-Architecture/Application-Level-Programmers--Model/Execution-state-registers/ITSTATE?lang=en)
are used by Thumb instructions for conditional execution and should be 0 for A32
mode and should be persisted across exceptions in T32 mode.


##### Execution mode and processor state

In ARMv8 and ARMv9,  execution mode, M\[4\] in the above SPSR/CPSR diagram, may
be set to indicate if the program is in AArch32 mode.  [Execution modes can only
be
changed](https://developer.arm.com/documentation/102412/0103/Execution-and-Security-states/Execution-states?lang=en)
on reset or exception level changes.

When in AArch64 execution mode, only A64 instruction sets may be used. In
AArch32, only A32 and T32 instructions are supported.  Which instruction set is
enabled is called the processor state and may be switched using the T bit, the
[Thumb execution state
bit](https://developer.arm.com/documentation/ddi0406/b/System-Level-Architecture/The-System-Level-Programmers--Model/ARM-processor-modes-and-core-registers/Program-Status-Registers--PSRs-).
In user space, this change is
[performed](https://developer.arm.com/documentation/dui0801/a/Overview-of-AArch32-state/Changing-between-A32-and-T32-state)
using a branch (BX/BLX). It is expected that any kernel software will ensure
the PC alignment matches the current instruction set alignment.


##### Floating point and Advanced SIMD registers

Beyond the core registers, [advanced SIMD and floating-point
instructions](https://developer.arm.com/documentation/dui0801/a/Advanced-SIMD-and-Floating-point-Programming/Architecture-support-for-Advanced-SIMD-and-floating-point?lang=en)
share the same register bank. It consists of thirty-two 64-bit registers, and
smaller registers are packed into larger ones, as in ARMv7 and earlier – based
around VFPv4 for AArch32.

In line with the stated requirements, the goal is to support ASIMD and VFP
extensions exposed to ARMv7 but supported in AArch32 mode.  Additional states
in AArch64 and future AArch32 ISA extensions will not necessarily be
supported.


##### Additional register mappings: counters and threads

There are many other register mappings beyond those already discussed for both
[AArch32](https://developer.arm.com/documentation/ddi0601/2024-09/AArch32-Registers)
and
[AArch64](https://developer.arm.com/documentation/ddi0601/2024-09/AArch64-Registers).
However, there are two that are relevant to the remainder of this design – tick
counters and thread storage.

On modern operating systems, all processes have a virtual dynamic shared object
mapped into their address space which attempts to provide access to time
checking functionality without entering the kernel.  This is often done making
use of tick or cycle counting.  The AArch64 registers used for this are
mapped one to one to AArch32.

Another long standing operating system feature is the [ELF thread local storage
ABI](https://www.akkadia.org/drepper/tls.pdf).
It's a storage model for variables that allows each thread to have a unique copy
of a global variable.   More on Fuchsia's use can be found
[here](/docs/development/kernel/threads/tls.md).
More on Android's usage can be found
[here](https://android.googlesource.com/platform/bionic/+/HEAD/docs/elf-tls.md).
The register used on AArch64 by these interfaces is
[TPIDR\_EL0](https://developer.arm.com/documentation/ddi0601/2024-09/AArch64-Registers/TPIDR-EL0--EL0-Read-Write-Software-Thread-ID-Register?lang=en).
While this is mapped to AArch32 as
[TPIDRURW](https://developer.arm.com/documentation/ddi0601/2024-09/AArch32-Registers/TPIDRURW--PL0-Read-Write-Software-Thread-ID-Register?lang=en),
ARMv7 software does not expect to have read/write access to the TLS register. It
expects the kernel to update
[TPIDRURO](https://developer.arm.com/documentation/ddi0601/2024-09/AArch32-Registers/TPIDRURO--PL0-Read-Only-Software-Thread-ID-Register?lang=en)
on its behalf.  This is mapped to
[TPIDRRO\_EL0](https://developer.arm.com/documentation/ddi0601/2024-09/AArch64-Registers/TPIDRRO-EL0--EL0-Read-Only-Software-Thread-ID-Register?lang=en)
on AArch64.


##### Exceptions and calling conventions

As indicated earlier, exception level changes provide the means to change
execution state.  Once the AArch64 kernel, running at EL1, has set the M\[4\]
bit and then returned to userspace, EL0, the processor is running in AArch32.
This means that any exception triggered will be an [AArch32
exception](https://developer.arm.com/documentation/100026/0100/system-control/register-summary/aarch32-exception-and-fault-handling-registers).

When an AArch32 exception is triggered, the processor will switch back to
AArch64 state to execute the [exception handling
code](https://developer.arm.com/documentation/100403/0301/AArch32-System-registers/VBAR--Vector-Base-Address-Register).
As the registers are mapped directly, minimal changes are necessary [to handle
an AArch32
exception](https://developer.arm.com/documentation/102412/0103/Handling-exceptions/Taking-an-exception).
Returning from exceptions is similarly uncomplicated.  However, it is necessary
to preserve the APSR/CPSR bits to ensure the execution mode and processor mode
flags are the same. It is worth noting that the program counter may not be set
directly on AArch64 – instead the exception link register (ELR) will be set to
value the PC needs to be set to on [return from
exception](https://developer.arm.com/documentation/100933/0100/Returning-from-an-exception)
(ERET).  For a return to AArch32 from AArch64 (via ERET), R15 will be restored
from the exception link register (ELR\_EL0).

It is also worth noting that the Linux calling convention for system calls
differs between AArch64 and AArch32.  In AArch64, x8 must contain the system
call number, but for AArch32 it is r7.  There will be other nuances which
need to be covered to ensure reliable operation.  For instance, cross-checking
that restored registers and APSR/CPSR condition bits are restored in a similar
fashion to Linux will avoid hard to diagnose failures.  In the initial review,
the restored state matches, functionally, starnix's AArch64 behavior, but given
the number of potentials paths entering and returning from exception states,
this an area that should receive careful review during the implementation.

#### Enabling restricted mode

Restricted mode is a feature that provides kernel-assisted sandboxing to Zircon
threads and processes.  In particular, when a process, or thread, enables
restricted mode, it provides the desired "restricted" register state to Zircon,
a place to return to (vector), and then makes the
[zx\_restricted\_enter](https://fuchsia.dev/reference/syscalls/restricted_enter)()
system call.  When Zircon enters restricted mode, it will configure the
processor registers as directed and then run the sandboxed thread with a
restricted address space.  When any exception occurs in that restricted mode
thread, Zircon will save the restricted mode processor state, restore the
calling thread's state from when it entered restricted mode, and then resume at
the supplied vector in the calling thread with a pointer to the state and
exception information.  Other than filtering and passing along the exceptions,
Zircon itself does not handle most exceptions raised by restricted threads.
(It's worth noting that page faults are handled transparently by Zircon.)  As
such, it is up to the supervisor portion of the restricted thread to handle both
the configuration entering restricted mode as well as handle the various states
coming out of restricted mode.

We'll first explore what's needed in Zircon to support entering AArch32
restricted mode and then what it takes to support exiting it.


##### Entry

Entry into restricted mode may occur when zx\_restricted\_enter() is called.  If
the supplied parameters and [bound
state](https://fuchsia.dev/reference/syscalls/restricted_bind_state) are
appropriately configured for the host architecture, Zircon will re-enter
userspace in the context of the restricted thread.   It's important to note that
entry into restricted mode isn't a one time operation.  While there is an
initial entry for any given thread, any restricted exception will be serviced by
the supervisor portion of the thread which must call zx\_restricted\_enter() to
resume the thread.


###### Transitioning the processor to AArch32 execution state

Given this and the above details about the ARM architecture, there are a small
number of changes necessary to enable AArch32 use of restricted mode.  Namely,
we know that we need to allow Normal Mode, the restricted mode supervisor,  to
set the M\[4\], T, IT, GE, and Q bits.  On initial entry, the CPSR will need to
be able to set M\[4\] and T bits to have the thread interpret A32 or T32
instruction sets.  IT, GE, and Q would need to be preserved across exit and
re-entry.

In the current AArch64 restricted mode entry code path
(RestrictedState::ArchValidateStatePreRestrictedEntry), the CPSR is filtered –
allowing the condition flags only.  The mask will need to be changed to allow
any of the above and to ensure it only does so with valid configurations. This
approach works well for AArch32 as it is a natural extension of AArch64, but it
may not be if different ISAs are supported in the future.  It may be that a
longer term approach will be moving the transition decision to the restricted
entry syscall, but there is no need to make that change at this point.

As the upper 32-bits of the registers are not needed in AArch32 mode and stale
values may be unexpectedly interpreted, it would make sense to clear those
bits proactively either on entry to AArch32 or on return.

There are a number of ways to approach this, from saving/restoring the registers
via the 'w' register accessor (32-bit) in the assembly exception path to zeroing
upon entry and/or return in the C++.  Zeroing during entry is the easiest, due
to a single control flow path, but with debugger access to the registers, there
would be no guarantee the upper bits remain cleared.

To limit the breadth of the changes to Zircon, initially zeroing in the
ArchSave* restricted mode functions.  This will ensure any stale or unexpected
values in the upper 32-bits of the registers will not be consumed by a
restricted mode supervisor, like starnix, while leaving future optimizations on
the table, if needed.  Additionally, the unused registers will not need to have
their state saved, beyond any default AArch64 updates.

###### Enabling the ELF Thread Local Storage ABI

Some implementations of the thread local storage ABI rely on the set\_tls()
system call and reading the TPIDRURO register.  However, modern armv7-linux
binaries tend to prefer to use the TPIDRURW register, which does not require a
system call to modify. (For AArch32, TPIDRURW is mapped to TPIDR\_EL0 and
TPIDRURO is mapped to TPIDRRO\_EL0.) Unfortunately, we are likely to encounter
binaries we want to run that also
[hardcode](https://android.googlesource.com/platform/bionic/+/main/libc/platform/bionic/tls.h)
the use of TPIDRURO, or, in the case of glibc, default to its use if the [Linux
kernel user
helpers](https://www.kernel.org/doc/Documentation/arm/kernel_user_helpers.txt)
are not present.

Setting TPIDRURO is not possible from the restricted mode supervisor today.
AArch64's zx\_restricted\_state\_t struct exposes access to TPIDRURW
(TPIDR\_EL0), but not TPIDRRO\_EL0.  The options are then to explicitly add
interface to set the register, such as adding an entry to the struct for
TPIDRURO (TPIDRRO\_EL0), or to implicitly mirror the value set for TPIDRURW to
TPIDRURO.  At present, it does not seem worthwhile to change the
zx\_restricted\_state\_t interface for this purpose as there are no known
binaries which use both TPIDRURW and TPIDRURO, and there has been no need to
support that register for AArch64 binaries.

If implemented, mirroring should only occur on entry into restricted mode.  If
TPIDRURW is changed by the thread, TPIDRURO will not be updated until the next
re-entry.  This should be acceptable because the ARMv7 userspace does not expect
to be able to mutate TPIDRURO except through a system call. If we see a Linux
binary accessing both TPIDRURO, via syscall, and TPIDRURW directly, then we may
need to add explicit support for setting TPIDRURO (TPIDRRO\_EL0).


##### Exit

Returning from restricted mode occurs on system call or other exception, and
it's handled in two stages.  In the first stage, an exception is raised in
AArch32 EL0 and then handled in AArch64 EL1 by Zircon which bundles the
exception state up and then passes it back to the AArch64 EL0 restricted mode
supervisor thread.

As highlighted earlier, AArch32 exceptions will be serviced by the AArch64
exception vector.  Presently, Zircon intentionally does not handle AArch32
exceptions.  There are three additional handlers required for synchronous
exceptions, asynchronous exceptions, and (asynchronous) system error exceptions.
Zircon's exception handlers are largely implemented using assembly macros. This
makes adding new handlers quite straightforward.  The other simplification is
that the inbound exceptions are meant to be handled by the restricted mode
supervisor.   This allows both asynchronous exceptions and system errors,
serrors, to be handled by the same handlers as the AArch64 counterpart.  Only
synchronous exceptions need special treatment and, of those, only system calls.
For incoming system calls, we can pass along any calls, using r7 as the number,
to the restricted mode exit handling code.  If a system call arrives and the
thread is not marked as restricted, we will panic Zircon as it should not be
possible to reach that code path.  This same behavior will be applied in
wrappers around each of the other 32-bit handlers as well.

This approach should minimize the changes needed to Zircon while still ensuring
that AArch32 exceptions are properly preserved and passed to the restricted
supervisor thread without accidentally enabling AArch32 exception handling
outside of restricted mode.


#### Debugging support

Initially, debugging support will be split across using in-restricted mode
software, such as gdb over ptrace, or using zxdb without AArch32 ISA Linux ABI
support.  As the register state is extended from AArch32 to AArch64 and the
converse, the register information will be correct, but handling stack traces
and A32 and T32 instructions will not function.  Debugging access to read
and write register state and flags will match the access given to the restricted
mode supervisor.  Debuggers will be able to determine which execution mode the
thread is in by checking the CPSR bits and should function for third party
debuggers.  zxdb will need to be extended to support AArch32.  The details
of that work aren't covered here and will require some additional research
and experimentation.


### Toolchain

With the above changes, Zircon is able to support executing 32-bit ARM
instructions but delivering instructions to be executed cannot be done without
toolchain support.  In order to compile any testing for AArch32 restricted mode,
we will need a toolchain that can target ARMv8 AArch32 or ARMv7 – the version of
Arm AArch32 is aiming for compatibility with.  Furthermore to support Starnix,
we'll need a toolchain which can handle Linux and bionic targets.


#### Enabling linux\_arm

Our toolchain currently includes a Linux ARM clang target along with the
required system root (sysroot) needed to compile static and dynamic targets.  It
is not, however, configured in our build system.  To do so, we must add a
configuration for the target architecture tuple as well as add the necessary
build system targets.


##### Linux ARM toolchain and targets

In the build/config/BUILDCONFIG.gn, we must define the new toolchain E.g.,

```
    linux_arm_toolchain = "//build/toolchain:linux_arm"
```

As well as add the target tuple to build/config/current\_target\_tuple.gni.
E.g.,

```
     . . .

    } else if (current_cpu == "arm" && is_linux) {

      current_target_tuple = "armv7-unknown-linux-gnueabihf"

    } . . .
```

Conveniently, the toolchain already present in Fuchsia is the gnueabihf.  All
AArch64 processors with AArch32 support will support hard float (VFPv4 as per
early discussion).

We will also need to add the toolchain target we defined first in
build/toolchain/BUILD.gn:

```
    clang_toolchain_suite("linux_arm") {

      toolchain_cpu = "arm"

      toolchain_os = "linux"

     use_strip = true

    }
```

Since Fuchsia is not intended to be built or run on legacy ARM cores or in the
AArch32 state, we do not need to enable this support for Fuchsia-specific
build target macros.

There are likely a few other configurations, such as rust, which will need to be
updated for the builds to proceed.


##### Rust

Aside from the vDSO, very little is built to run in Starnix in the Fuchsia tree.
Most of it is provided via the Android build system.  However, there are [some
tests](https://source.corp.google.com/h/turquoise-internal/turquoise/+/main:src/starnix/tests/syscalls/rust/BUILD.gn)
which target Starnix and are built using Rust.  Supporting these targets will
require runtime library support for armv7-linux-gnueabihf in our toolchain.
Supporting both this and LLVM above will increase the support burden on our
toolchain team, upfront and for as long as we plan to maintain the AArch32 build
environments.

If the cost is untenable, then we should consider alternative strategies to test
the same code paths via the normal toolchain and runtime (e.g., adding a custom
personality to AArch64 starnix which enables access to the compat system call
table, or similar).


##### Starnix targets

Starnix includes its own build targets for both Linux and Linux/Bionic targets.
Both can be built upon the existing work of the toolchain team.  In
src/starnix/toolchain/config/BUILD.gn, the correct tuple and header
path must be configured for bionic targets along with the declaration of the
same toolchains in src/starnix/toolchain/BUILD.gn.


### Starnix

The simplicity of the Zircon changes are possible because the complexity is
passed to Starnix.  Starnix services all of the kernel interfaces Linux
userspace software rely on. It must be able to execute ELF binaries and handle
system calls issued by that code as well as other exceptions that may occur –
32-bit or 64-bit.  While there are some differences that cannot be avoided, such
as pointer size or system call numbers, much of the AArch32 functionality can be
handled seamlessly from the existing code.

The approach taken in this design is to handle AArch32 functionality as close to
its interface point as possible such that the majority of the Starnix code does
not need to consider 32-bit architectural implications. These interaction points
are at the processor level, then system call entry, userspace copying (to and
from), and executable loading. The only other areas are ones where registers,
instruction length, or pointer sizes matter, like signal handling, ptrace, and
core dumps.

The sections below will work our way through Starnix in a constructive fashion.


#### Task support

While it may seem strange to start with "Task" support rather than at the
interface points, as indicated, there is a compelling reason.  Starnix support
for AArch32 will not be a binary state – it will support both AArch32 and
AArch64 tasks.  As such, the task is the logical unit where the execution mode
can be annotated. Additionally, the execution mode is thread specific so this
design assumes that the ThreadState struct on the current task will indicate if
the thread is an 'arch32' thread.

Note, that 'arch32' is used rather than 'AArch32' to avoid tying the naming to
a specific processor implementation.

While it may be possible to avoid annotation at all, such as by always checking
the register values in the CPSR, a dedicated annotation provides two useful
properties.  The first is future-proofing the starnix code.  For instance, if
riscv32 is added, all the code using an 'arch32' check will function
appropriately without architecture-specific changes.

The second property is in declaring the state of the task at initialization.
This is important as the task's memory manager, at the least, is configured
for 32-bit layout and enforcement.  If a task changes the CPSR execution
mode bit, it will change what instructions it can execute and the type of
exceptions it raises, but it will not change the pre-configured task state.

While confusing Starnix about the execution mode should not have any immediate
security implications, arbitrary switching between modes violates the normal
assumptions that Starnix developers may make as they work on any 64-bit or
32-bit specific code paths. This sort of
[confusion](https://github.com/deroko/switch/blob/master/switch.md) has happened
on Linux before.  It would make sense to harden this area, either at the
'arch32' accessor or at the exception handling layer -- to ensure that a task's
arch32() value matches the system call or exception handler for their exception
mode.


#### Processor support

At a processor level, register handling is the primary interface point.  The
ARM64 RegisterState struct will need to be updated to ensure that updates to PC,
SP,  LR, CPSR, and TPIDR accurately update the AArch32 registers and do not
clear state that must be maintained (e.g., in the CPSR).  In addition to the
starnix/kernel/arch/arm64 code, this will also need to be addressed in the
Zircon rust code (e.g., src/lib/zircon/rust/fuchsia-zircon-types/src/lib.rs).
For the registers handling struct, it will be necessary to ensure that updates
are applied to the correct registers and that the naming matches the underlying
register, e.g., syscall\_register() uses r7.  In both the Starnix arm64 code and
the Zircon rust libraries, it'll be necessary to ensure that all From/Into
implementations are updated to properly map between restricted state structs and
register structs.

In some places, the code will only be able to rely on the CPSR value to
determine whether to perform register changes.  If there is access to the
current task, then the arch32 boolean will be used.


#### UAPI Support

The Linux kernel interfaces for each architecture are called the Linux UAPI, or
userspace API.  Starnix generates its rust bindings using bindgen and its own
python script which drives it.   As the UAPI includes the structs passed in and
out of kernel interfaces, a AArch32 UAPI will be necessary for Starnix to
properly handle them.

There are three main areas that need to be addressed:



1. 32-bit pointers and types
2. Accessing two UAPIs in parallel
3. System calls naming


##### Pointer and type sizes

For the first, there are two problems.  The first is that we will need to
replace all pointers with an AArch32-sized pointer type.  Starnix normally uses
uaddr and uref.  For 32-bit sized pointers, adding a uaddr32 and uref32 and
ensuring they are put in place of pointers and references should suffice.
Additionally, if it is possible to trivially convert, via From/Into, to uaddr
and uref, then uaddr32 should largely become invisible except when explicitly
needed.

The second problem is that the same type names have different sizes.  For
instance, a long is 64 bits wide on AArch64 and 32 bits wide on AArch32. This
means that the default type mapping cannot be shared between the two UAPIs and a
specific set of kernel C types must be defined and injected for each UAPI.


##### Parallel UAPI access

With a clear path to generating the ARM UAPI bindings, Starnix must be able to
access both AArch64 and AArch32 at the same time.  Thankfully, this can be dealt
with by mapping a generated "arch32" UAPI under a nested namespace in the
linux\_uapi's lib.rs.  E.g., "pub use arm as arch32" along with the
correct cfg decorators.  (Note, that in Linux "arm" represents the legacy ARM
32-bit ISAs as well as AArch32.)

This will allow any code in starnix to access the appropriately sized types and
generated bindings using the arch32 namespace.


##### System call numbers

With a dedicated namespace, it is possible to separate the AArch64 mmap system
call from the AArch32 mmap using the arch32 prefix.  However, wherever possible
we will share system call implementations.  There are some system calls where
the parameters and behavior are arch32-specific.  In those cases, it may make
sense to shadow the definitions with specific naming, like \_\_NR\_arch32\_open
so that the later construction of system call tables can provide more readable
names.

As Starnix drives all the changes as part of the UAPI generation step, the same
can be done here.  The addition of a new arch32 header for stubs and
redefinitions will allow any custom definitions to be pulled into the arch32
namespace without requiring any on-going maintenance of a separate Rust-based
mapping.


#### Loader support

With the above three pieces in place, there is still no effective way to
[launch](https://lwn.net/Articles/631631/) a AArch32 task or thread.  As the
M\[4\] bit in the CPSR can only be changed on return from an exception (EL1->0),
there needs to be a way for userspace to ask Starnix to set the bit before
returning to EL0.  There is no need for specific support, instead this
functionality can occur only on call to the execve system call or on first
process, init, launch.  In both cases, a file must be loaded and its file format
supported by Starnix.  Currently, Starnix supports ELF64 format files which it
can parse, load into memory, and finally run.  The same functionality will be
needed for ELF32 formatted files – ideally with minimal changes throughout the
Starnix kernel.


##### ELF parsing

ELF parsing is implemented in src/lib/process\_builder/elf\_parse.rs.  All the
necessary ELF64 definitions are present along with the ability to parse ELF64
file headers from a VMO.  The same definitions will need to be duplicated for
ELF32.  However, Starnix expects to be able to pass around Elf64\* structs (not
Elf64\* traits, etc).  As such, ELF32 support can be added similarly to the
uaddr32/uref32: define as much as is needed to directly interact with the
32-bit types and data, then provide From&lt;&gt; implementations to convert to
the 64-bit representation.  While this approach will cause an e\_ident class
mismatch in the resulting Elf32-\>Elf64 instance, the struct will be compatible
with all existing interfaces and calls.

Additionally, loading arch32 compatible ELF files can be supported through a
secondary entry point, such as from\_vmo\_with\_arch32() versus from\_vmo(), to
ensure the caller is aware of the possible output.  Similarly, a helper
function, like arch32()-&gt;bool, would enable easy checking of the ElfIdent
class for any subsequent callers in the loader to determine if arch32 behavior
is needed.

Other than the decision to convert to Elf64 compatible instances, the work is
largely mechanical (much like UAPI and other topics discussed already).  It
is worth considering upleveling the Elf64 interface such that it handles the
specific format without file format specific expectations, much like
[elfldltl](https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/src/lib/elfldltl/).


##### ELF resolution and the MemoryManager

Starnix considers an ELF resolved when it, and its interpreter, have been parsed
successfully.  This happens before it commits to finish the exec call.  While
AArch32 support can proceed without changes here, this is a critical time in
task setup.  It's at this point that the virtual memory area (VMAR) for the user
process is created.  Normally, this VMAR is the full available restricted
address space.  However, by extending the call to the MemoryManager (mm.exec()),
Starnix can establish a user\_vmar that is appropriate for a process that has at
most a 32-bit addressable memory space (e.g., 4Gb).

As all memory-oriented operations use the task's "mm", an appropriately sized
VMAR will guarantee that mappings will only occur in an address space
addressable to the arch32 task.  Additionally, it will allow any randomly, or
automatically, selected memory addresses to come from the available range
without the need for a special flag to limit the mapping offset.  The use of a
user\_vmar is a new addition to Starnix, but it greatly simplifies the process
of enforcing restrictions on where a restricted thread can interact with memory.
This simplification is used in the next section.

Beyond memory configuration, resolution also allows tracking of the execution
mode (arch32) to be performed before the full loading of the executable allowing
subsequent steps to ensure no mismatches occur between memory configuration and
the resulting loaded ELF files.


##### ELF loading

Once the file has been resolved, the kernel will know which sections should be
mapped into memory and with what permissions. Generally, Starnix will attempt to
determine the skew from where the mm's base pointer is and the lowest address
listed in for a region.  It does this with wrapping\_sub and wrapping\_add
operations.

The general idea is that a wrapped value will wrap back when the virtual address
is computed (or the wrap is so severe that the region can be mapped on the
higher end of the 64-bit address range).  It appears that most ELF64 executables
have a lower address equivalent to the mm base 0x20000, so it is likely that
this functionality is not used extensively.  ELF32 executables often have a file
base that is less (0x10000) which results in a 64-bit wrap that is often not
wrapped back.

One option to address this problem is to use a 32-bit wrapping operation.
However, the resulting placement of any 0x0 offset segment is still likely to
fail.  As such, it makes more sense to ensure the file base for ELF32 will not
underflow.  This can be done by setting the file base for the executable to
either the mm base pointer or the lowest ELF info value, whichever is higher.
With that done, the wrapping\_add/sub can be ignored entirely, leaving it intact
for 64-bit use if it is used.  For most binaries, this is not relevant as many
Linux and Android binaries are position independent executables (PIE) – which
means they will get a randomized base – which will always be at least at mm's
base pointer or higher.  Once the relative base is set, the rest of the ELF
segment loading will proceed normally – converting the mm/file offsets to a
virtual address and then mapping from there, as it's done for ELF64 files.

Note, any non-relocatable ELF binaries with low offsets will not perform as
expected if they are not mapped as specified in the file.  While this may be a
problem that needs to be tackled, it is not a priority as most modern
armv7-linux binaries will be either PIC or PIE.


##### Mappings and ASLR

While ASLR applies both above, to the PIE base address, and below, to the stack
and heap starting points, it's worth reviewing how the change to the user VMAR
above impacts subsequent mapping.  In the current nascent ASLR implementation, a
certain number of bits of entropy are drawn and applied to the desired target
address – moving it higher or lower in memory than the starting point.  By
default, this will fail on 32-bit threads because of the large difference in
addressable memory.   AArch64 expects to be able to add 28-bits of entropy.  On
Linux, AArch32 is lucky to achieve 18 bits of entropy.

While the current approach will cost the least in terms of computation, the
ability to flexibly size the user VMAR means that the randomization strategy can
be similarly adjusted.  For each location that needs randomization, the process
of randomizing an address, and its direction of randomization, will be
centralized into a MemoryManager function.  There it will compute an ASLR
entropy mask based on a supplied maximum size, or range.  The mask will be one
less than one left shifted the number of bits in the range size minus the number
of bits in the page size.  This means that at most for a 4Gb addressable region,
19 bits of entropy can be applied. For a 64-bit addressable region, it would be
much higher, but the original AArch64-specific bits of entropy are used as a
cap.

In practice, the randomization range for any given attempt is not the full
address space – it will be a region between other mapped objects, like the top
of heap versus the stack.  This is why randomization entropy ends up being [much
lower in
practice](https://gist.github.com/thestinger/b43b460cfccfade51b5a2220a0550c35).
This design is focused more on enabling AArch32 support – a separate effort
should be made to measure entropy in the ASLR implementation and determine if it
should be modified separately from any performance concerns.

If performance becomes a challenge, we can cache the entropy mask and move to
hard coded masks, one for arch32 tasks and one for normal tasks.  Initially, the
goal is to streamline the caller's experience, but optimizations can be
introduced as needed – for instance, if it negatively impacts program start
times.


##### vDSO

The virtual dynamic shared object is a synthetic shared library that the kernel
maps into processes when they are executed. Its purpose is to aid in smoothing
the interface between userspace and the kernel.  The vDSO is code supplied by
the kernel meant to be run in userspace by the process.  For instance, a system
call helper is usually present to ensure the kernel calling conventions are
used.  Similarly, there are often system time related calls which allow any
architecture-specific optimizations to be used when a userspace process attempts
to check the system time.

Details of how the vDSO time is managed in Starnix can be found
[here](https://fuchsia.googlesource.com/fuchsia/+/main/src/starnix/kernel/vdso/).
At present, the vDSO relies on functionality presented as a library by Zircon,
fasttime.  However, the vDSO must be built targeting AArch32 and Zircon does not
plan to support building for AArch32 architectures.  As such, the fasttime logic
may be extracted for inclusion in an AArch32-specific vDSO helper.  It's
important to acknowledge that this approach is a stopgap while the longer term
Zircon vDSO comes to fruition.  By relying on the same interfaces as AArch64
Starnix without creating additional overhead for Zircon, this model should
provide AArch32 with similar reduction in context switches without creating
a large amount of technical work for a temporary solution.

Beyond fasttime, today's Starnix vDSO cannot depend on other shared objects.
Time computation is usually done, at least partially, with 64-bit integers.
Clang and other compilers provide helpers, like compiler-rt, for ARM to perform
64-bit modulus and division operations.  Initially, this effort will simply use
binary division in C++ and export the expected symbols.  It is very likely these
will need to be replaced with optimized assembly code.  There may also be other
symbols and functions that need to be added to the AArch32 vDSO that may deviate
from the AArch64 vDSO.  These can be added as needed.  Similarly, time-related
functionality can be removed from the AArch32 vDSO if the need is not justified.

The AArch32 vDSO build itself must also be done in parallel to the AArch64 vDSO.
As such, a new target must be added that can be forcibly built with the arch32
toolchain and lacks dependence on the Zircon headers.  A new field will be added
to the Kernel struct which mirrors the 'vdso' field, 'arch32\_vdso'.  It will be
an Option&lt;&gt; to allow for graceful failure of only arch32 executables if
the arch32 vDSO is lacking.


##### Stack preparation

One of the last steps before an executable can be launched is setting up its
stack in a way that the code at the ELF entry point knows how to handle.
Starnix already handles these steps, ranging from setting up the [auxiliary
vector](https://lwn.net/Articles/519085/) (AUXV) to placing the arguments and
environment variables.  On a 64-bit platform, pointers for both arguments,
environment variables, and integers for the AUXV are all 64-bit wide.
Additionally, the stack must be 16-byte aligned.

This means that at a minimum, the sizing of the pointers and values must be
adjusted for ELF32 binaries. In the current Starnix implementation, a stack
vector is built up and then copied into place in memory.  The easiest path to
support AArch32 would be to duplicate the code path and handle it separately.
However, the stack vector is a vector of unsigned 8-bit bytes.  This means that
it is possible to encapsulate writes to the stack to select the correct width, 8
byte or 4 byte.  With a change like that in place, the final stack should
automatically be the correct size, as the values to be written must already be
sized correctly.  Additionally, since the 32-bit stack must be 8 byte aligned,
the 16 byte alignment already satisfies the requirement.

One missing piece in the current Starnix stack preparation is in the a features
which can be uncovered via probing, but may be disclosed at process execution,
via the HWCAP entry – such as if thumb mode is supported or which type of
floating point operations are allowed.  These entries are used as early as the
AArch32 glibc and bionic loaders.  A new function will need to be added to each
kernel/arch/processor which returns the list of hardware capabilities supported
by the processor.  Each implementation can use the 'zx_cpu_get_features' call
to determine how to populate the 'AT_HWCAP' field.  Initially, this can be
added exclusively for arch32 support as the currently supported architectures
are working without any support.


##### Entry point

Finally, the executable file, or its interpreter, will have supplied the entry
point for the Starnix or Linux kernel to start the thread at.  Normally, this
entry point must be instruction aligned.  However, ELF32 on ARM use an unaligned
entry point to indicate that the processor should be started in Thumb
instruction set execution mode.   If the entry point bit wise ANDed with 0x1 is
0x1, then the T-bit must be set on the CPSR prior to code execution or the
processor will attempt to interpret T32 instructions as A32. (Note, this
alignment approach is taken on [exception return at the processor
level](https://developer.arm.com/documentation/ddi0406/b/System-Level-Architecture/The-System-Level-Programmers--Model/Exceptions/Exception-return)
as well.)

As the entry point for the new thread must be the program counter (and exception
link register value), it will be populated via the ThreadStartInfo's conversion
to  zx\_thread\_state\_general\_regs\_t.  The From&lt;&gt; implementation in
kernel/arch/arm64/loader.rs can check the alignment of the PC and
then set the Thumb mode appropriately.  In all other cases, the Thumb value will
be enabled or disabled via branch instructions in user space and its state will
need to be kept intact across calls into Starnix.

With this configured, an AArch32 executable can be launched, but it will fail
immediately due to invoking an AArch32 system call with a AArch64 system call
number.


#### System Calls

Other than error conditions, system calls are the primary interface between
userspace software and the (Starnix) kernel.  This means that there are two main
challenges to deal with:



1. Calling convention and routing
2. Userspace memory reading and writing


##### AArch32 System Calls

As indicated earlier, Linux AArch32 system calls use different numbers than
AArch64 system calls.  Additionally, AArch32 uses a different system call number
register than AArch64, and the parameters for system calls may differ between
the two as well.

From a parameter perspective, there is no need to deal with pointer size
differences because the registers will be extended on entry to Starnix. The
difference in numbers and parameters pose a more interesting challenge.  Starnix
must be able to lookup the system call by number and map that number to an
implementation.  While it is possible to force every system call implementation
to perform some arch32 checking, this work can be done at system call handling
time.  Namely, Starnix can setup a secondary system call table which is only
used for arch32 tasks.  This will result in a modest amount of additional memory
usage, but it will provide a means to enforce the AArch32 versus AArch64 user
space ABI and provide a simple method for sharing system call implementations
directly or indirectly via an AArch32 aware wrapper.

This approach will require some duplication of macros, at least initially,
though it may be feasible to minimize duplication there as well.


##### AArch32 Data Structures

While the pointers in the system call parameters do not pose a challenge, this
is not the case when copying structures from or to AArch32 user space.  AArch32
software will use the structs defined in the AArch32 Linux UAPI, not the AArch64
UAPI.  For instance, IO vectors, iovecs, are structures that are simply an array
of pointer and size pairings.  On AArch32, iovecs are two 32-bit fields while on
AArch64, they are both 64-bit fields.  This means that any system calls using
iovecs must adjust appropriately.

This problem doesn't impact every system call, though.  About a third of the
AArch32 system calls have different structures compared to their AArch64
counterparts.  For specific calls,  the specific struct can be handled in an
arch-specific system call implementation.  For instance, stat system calls
expect a stat64 structure to be returned which does not match the struct
returned from AArch64 stat calls.  This can be tackled with a new stat
entry point under kernel/arch/arm64/syscalls.rs along with a
conversion between the types.

Common structs, like iovec, may be dealt with more generically.  Starnix's
MemoryManager provide a helper for reading iovecs, read\_iovec().  It depends on
read\_objects\_to\_smallvec() where type polymorphism takes a SmallVec of a
UserBuffer alias as a type to read to.  For all existing architectures, a
UserBuffer is a Rust version of an iovec.  By adding a UserBuffer32 to Starnix's
UAPI, it is possible to create a parallel AArch32 compatible iovec SmallVec
type.  This can then be exposed via a read\_iovec32() function which itself can
return a common UserBuffer (thanks again, into()/collect()!). While it doesn't
hide the AArch32 iovec read, it limits the exposure of AArch32 specific types to
the rest of the calling code.  There is an open design question which is whether
it would make sense to pass in the task's arch32 field to the read\_iovec() call
versus calling a separate function.  It is likely that flagging like that will
simplify adding support for arch32 to common system call implementations, like
sys\_writev().


#### Signals

Signal handling and ptrace() in general are areas that have close interaction
with the underlying architecture.  For the most part, mechanical updates to
signal handling and ptrace will need to be added to appropriately wire up the
correct signal structure contents and architecture information.

System call restarting will be slightly more complex because resetting the
program counter/instruction pointer will be dependent on the mode: A32 or T32,
as T32 instructions are 2 bytes and A32 are 4 bytes.

This area will need more attention, but it will be easier to explore once the
rest of the arch32 support is functional.


### Testing

As with most software, it is expected that the lines of code in the tests should
outstrip the lines needed to implement AArch32 support – especially for Zircon.
This section covers the testing code planned.


#### Toolchain

The Fuchsia Clang toolchain and other portions of the toolchain have adequate
support and testing outside of the Fuchsia builds.  There are two aspects to
consider, build configuration testing and intrinsic toolchain testing.  Often
both are entangled, and the goal is to ensure that the build for
armv7-linux-gnueabihf is in working order on a continuous basis.  One current
example is with the "--allow-experimental-crel" flag.  Enabling this flag for
Linux/ARM results in broken binaries.  The failure is due to, potentially, a
code generation error in the toolchain, but the build configuration is what
determines whether it will break any on-going builds.

It's important that we catch those changes early.  If we were to support this
feature without testing, we would only catch this failure in crashes with Zircon
testing (discussed later) or in failures accessing the AArch32 Starnix VDSO.  We
will need to enable the core/essential LLVM tests for any toolchain targets
tuples that are enabled to ensure operation and correct code generation.


##### Zircon testing support

Zircon testing is challenging because the tests should exercise restricted mode
explicitly and not depend on a fully functional supervisor/kernel.
Additionally, if the tests do not load binaries from alternative locations, like
a full container, the code must be self contained.

For the Zircon utests, we will take advantage of the build system's
loadable\_module() support to build architecture-specific shared libraries that
can be dynamically loaded and mapped using the
[elfldltl](https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/src/lib/elfldltl/)
library.  This allows integration test to run on the host architecture of
AArch64 while loading and resolving symbols from an AArch32 targeted
(armv7-linux) shared library.  These shared libraries would be loaded, and
mapped as executable, as any other executable code on BOOTFS.

The remaining requirements revolve around what is supported by the host and what
is accessible by the restricted AArch32 shared library.  The shared library must
be mapped into addressable memory (below 4Gb) as well as any shared pointers,
such as std::atomic or TLS storage addresses.  With that in place, the tests
must also determine if the host architecture supports executing AArch32 and, if
supported, run the test suite against both the host-targeted restricted mode and
the AArch32-targeted restricted mode.


##### Starnix testing support

Initial validation will be done against a minimal Linux system running under
Starnix.  This will ensure minimal dependencies when validating the toolchain
and build system changes for Starnix.  To enable this, the ARM64 Debian
container will have the AArch32 sysroot added to its image, and the
example/hello\_debian examples will be expanded to include AArch32 examples,
both dynamically linked C++ and statically linked assembly.  The target support
and toolchain sysroot must be roughly synchronized.

The initial testing environment consists of Starnix executing one binary
(hello\_debian) against either AArch64 libraries or AArch32.  No other processes
or work will be underway.  However, regular testing will be needed to cover the
differences from AArch64:


* System call support coverage
* Signals and ptrace behavior
* Mixed execution mode environments (shared sysroot, container, ...)
* Tests per supported ELF file types
* ASLR entropy measurement


Some of these tests will need to be written for Starnix use, but the ideal
state would be to use existing projects, like the [Linux Test
Project](https://android.googlesource.com/platform/external/ltp/+/refs/heads/main),
which already aim to ensure on-going Linux compatibility.


#### Zircon

There are number of tests which should be added that are AArch32 specific:


* CPSR:M\[4\]: A32 execution
* CPSR:T|M\[4\]: T32 execution
* Register setting and clearing
* &gt;32 bit register setting, clearing behavior
* System call handling
* Non-system call synchronous exception handling
* Asynchronous exception handling
* SError handling
* CPSR:Q: Saturation bit setting and persistence through a system call
* CPSR:GE: GE bit persistence through a system call
* IT bit usage
* read\_state and write\_state APIs for threads


While the basic conditions are easy to validate, like the execution mode bits
for A32 and T32 as well as register manipulation, more complex exception
handling may be more challenging.  For instance, inducing a SError from EL0 may
require providing the test access to unmapped memory to interact with.  IT bit
tests will require the ability to exercise intermediate states, which should be
doable using in-thread testing, but may need some additional debugger-driven
test cases.


The existing tests should also be exercised with a restricted aarch32 target:


* FloatingPointState
* Bench
* ExceptionChannel
* InThreadExecution
* EnterBadStateStruct
* KickBeforeEnter
* KickWhileStartingAndExiting
* KickJustBeforeSyscall

At present, the tests select the compiler target arch to test. While it is
possible to hard-code arch32 checking and specific testing, it would be nicer to
fit 32-bit architecture in as another test target. In order to do that, we will
need to be able to reuse some of the functionality across the shared AArch64
architecture while defining the test fixture for the AArch32 ABI.  Refactoring
these tests to use a common base class with architecture-specific class
implementations will make it easier to derive arch32 subclasses.  With a
reliable per-arch type, the TEST\_P macro may be used to parameterize the tests
based on the arch-specific class instance to be used.  Common tests for the same
functionality will avoid missing coverage due to errors induced by testing
complexity.  However, TEST\_F() may still be used where needed for non-generic
testcases.


#### Starnix

As mentioned in the toolchain section, the initial integration testing will be
done using hello\_debian.  This allows the isolation of functionality: loading
and system calls.  From a unit test perspective, existing class testing will be
expanded to cover added functionality.

Beyond that, new testing will be needed to exercise AArch32 specific behavior,
like memory limits and system calls.  The Linux Test Project and other system
call test frameworks will be used to ensure coverage and correctness alongside
regular unit and functional testing.


#### Testing in Infrastructure

Automated testing in infrastructure must be wary of two main issues:


* Lack of AArch32 support
* Hardware variation

Any hardware accelerated virtualization used in testing on ARM64 processors
is unlikely to work with AArch32 testing as server-oriented processors often
do not include support.  Fully emulated virtualization will function, but
run more slowly.

Additionally, testing should occur across any hardware which is expected to use
this functionality as exception handling quirks may exist across different
processor implementations.


## Implementation

The implementation strategy is as follows:


* Update Debian arm image in CIPD
* Toolchain changes and testing to enable arm clang
* Zircon changes to enable and test 32-bit restricted mode
* Starnix:
    * New task flags
    * Elf parsing and loading
    * AArch64 registers
    * Resizable user vmar changes (with ASLR)
    * Linux UAPI changes
    * AArch32 vDSO
    * Loader changes
    * arch32 system call table and support
    * hello\_debian examples

From there we will need to determine what system calls are needed and continue
to implement them.  Additionally, debugger and extra toolchain investigation
can proceed.

## Performance

It is important that the changes add minimal overhead to normal 64-bit operation
of tasks in Fuchsia or Starnix.  That can be evaluated with microbenchmarks
between Zircon versions, though we can also just evaluate full system benchmarks
to determine if there is a material loss.

For AArch32 operation, the restricted mode benchmark will be the first path to
compare the performance to AArch64 restricted mode.  After that, it will be most
beneficial to benchmark with realistic workloads against similar Linux
systems.

On-going system call optimization, memory utilization, and power monitoring will
all assist in confirming the viability of this functionality.

## Ergonomics

In Zircon, the current approach does not result in a specific set of API
changes.  Instead, AArch32 support is enabled by supporting the existing
architectural ABI in restricted mode.  This approach does not significantly
increase the complexity or decrease the ergonomics for developers.  Any
developer making use of this functionality will need to implement the remainder
of the ABI themselves.

In Starnix, the changes will initially be flag protected.  Code paths that must
be differentiated should be done so as close to the locations where
differentiation happens and minimize code forks occurring throughout starnix.
For instance, if the memory restrictions can be enforced in the MemoryManager,
then all system calls and other functionality that relies on the MM will be
unchanged from the developer perspective.

## Backwards Compatibility

AArch32 user space support is essentially a backwards compatibility feature. It
is expected that AArch32 usefulness will fade over time.

## Security considerations

This proposal changes how Zircon handles exceptions and the ABI.  This means
that it should be evaluated carefully.  Additionally, the code should be written
defensively -- disallowing unintended code paths proactively.  Optimization can
follow if the benchmarks indicate the need.

From the Starnix perspective, additional system calls and signal handling paths
are being added, as well as additional file parsing.  Address space
randomization will also be directly impacted by the reduced memory footprint.
These are all security relevant changes.

We should reuse existing Linux security testing tools wherever it makes
sense, e.g., [syzkaller](https://github.com/google/syzkaller).

## Privacy considerations

This change should not have any specific privacy changes.

## Documentation

Restricted mode API documentation will be updated to indicate the additional
allowed register flags.

Starnix documentation should be updated to include information about how to
use and develop with AArch32 support.  Additional documentation should focus
mostly on the impact to developers, such as managing two system call tables
or testing changes on both AArch64 and AArch32 paths.

Example AArch32 binaries running in Starnix will be found in
src/starnix/examples/hello\_debian.


## Drawbacks, alternatives, and unknowns

This proposal increases long term maintenance as well as system complexity.  The
trade off is enabling access to the world of AArch32 Linux software for the
remaining years that it is supported on consumer focused SoCs.

There are no performant alternatives for supporting AArch32 programs unmodified
and the alternative is to recompile all code to AArch64.  These less performant
alternatives are transpiling or selective user mode emulation.

The primary area of continued investigation is centered around enabling a high
quality debugging experience when analyzing a thread which has both AArch64
and AArch32 calling conventions and instruction sets in the same backtrace.

In particular, current debugging workflows rely on libunwind for unwinding
through Linux call stacks on AArch64 and zxdb for analyzing state within a given
call frame.  Support for similar behavior will need to be added to both zxdb and
libunwind.  libunwind should already support the AArch32 Linux ABI, but it is
unlikely to support transitioning cleanly between ABIs in the same thread.
The intention is to achieve parity for both tools, but it will require
additional investigation and experimentation.

Beyond debugging, developer workflows may also suffer from comprehensive
testing.  Developers using powerful Intel-based environments will be required to
fully emulate the AArch64/AArch32 environment. However, that is true today if
they are targeting AArch64 already.  As mentioned earlier, the added complexity
here is the lack of AArch32 support on server-oriented ARM SoCs.  Where
Intel-based developers may have been relying on accelerated, server-side AArch64
testing, they will now need to use on-desk or hosted devices which support both
execution modes.  It may be that AArch32-specific call paths may be enabled or
disabled using a thread personality in starnix to further reduce the amount of
code that cannot be reached at scale without hardware support of AArch32.


## Prior art and references

References have been embedded as links.  Linux refers to AArch32 support on
AArch64 as compatibility mode, and it provides the prior art.
