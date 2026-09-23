<!--
// LINT.IfChange
-->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-NNNN" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Problem Statement

Zircon provides a preemptive multithreaded programming model and supports
systems with multiple cores. Threads behave as if they were executed linearly
based on the rules of the underlying architecture even if they are interrupted,
preempted, or migrated to different cores. Zircon is responsible for ensuring
proper ordering of operations across these events to make them transparent
to the operational semantics of applications.

There are some situations where for reasons of performance or architectural
correctness this abstraction layer breaks down and applications must coordinate
more closely with Zircon on the details of how threads are scheduled on cores.
This RFC introduces an operation that insert logical barriers into the execution
stream of threads to ensure the ordering of certain operations on specific cores.

## Summary

The Zircon membarrier API inserts a barrier into the logical execution stream of
running threads within the system. The effects of these barriers must occur
before the membarrier call returns. From the application's perspective the
barrier operates on a set of threads. Zircon can implement the barriers by
considering what is running on active cores since a thread that is not on an
active core is by definition not running. As there are typically many more
threads than cores in a live system implementing the barriers in terms of cores
is significantly more efficient than considering threads.

### Motivation

Certain operations require issuing a barrier operation at specific times in
order to observe the effects of changes in the system. Applications performing
these operations may arrange for threads to issue these barriers at the
appropriate points in their code, or ask the system to insert these barriers
into the instruction streams of the cores running these threads on its behalf.
For some designs, it is more efficient to let the operating system issue the
barriers on the cores that need them. Many programs have more threads than
cores, and those cores may busy doing operations unrelated to the barrier.

### Linux `SYS_membarrier` implementation in Starnix

Linux programs running under Starnix issue `membarrier()` system calls [^1].
This system call must issue barriers of different types on some cores in the
system for correct operation of the program. These are used as parts of
performance sensitive operations such as garbage collection and just-in-time
code generation. Starnix is not in a position to efficiently implement these
barriers and requires assistance from the Zircon kernel.

### Instruction stream barriers

The architectures that Zircon support primarily use a Von Neumann memory
architecture with support for Harvard architecture caches. To support these, the
architectures require issuing instruction cache specific operations in between
storing data to a specific address in memory and loading that address for
execution. Zircon manages these operations transparently when mapping memory
with executable permissions or changing the permissions on mapped memory.
Applications that modify executable memory in ways that do not involve one of
these operations must ensure that the appropriate operations are executed on the
core loading the executable memory. Without Zircon assistance this operation is
either inefficient (on ARM) or impossible (on RISC-V).

## Stakeholders

- Starnix
- Zircon

_Facilitator:_
- abarth@google.com

_Reviewers:_

- travisg@google.com
- mcgrathr@google.com
- maniscalco@google.com

_Consulted:_

- abarth@google.com
- maniscalco@google.com
- travisg@google.com

_Socialization:_

A problem description was send to the Zircon discussion list. This issue has
also been discussed in https://fxbug.dev/297526152.

## Requirements

### Types of barrier

Two types of barriers are supported: data memory and instruction stream
barriers.  In this API barriers apply to all running CPUs in the system. Future
iterations may limit the scope of barriers, see the "Future work" section below
for more detail.

#### Data memory barriers

This barrier type provides an ordering of all data load and store operations
executed on other running threads in the system before and after the barrier.

The Linux `membarrier` API calls this simply "MEMBARRIER".

#### Instruction stream barriers

On weakly ordered memory architectures such as ARM and RISC-V the
procedure for modifying executable memory and ensuring that the modifications
are visible to cores executing that memory. The operations are distinct from
the ones used for pure data operations to support Harvard cache architectures
and speculative execution.

The Linux `membarrier` API calls this a "SYNC_CORE" barrier and the
barrier implies a data memory barrier as well. This is only supported at
the private scope (i.e., within a process).

###### ARM

Quoting from ARM's documentation [^2] for ARMv9:

The architecture considers the following as separate Observers:

> * The instruction interface of the core, typically called the Instruction Fetch Unit (IFU)
>
> * The data interface, typically called the Load Store Unit (LSU)
>
> * The MMU table walk unit
>
> As described in Who is an Observer?, an Observer is something that can make memory accesses. For example, the MMU generates reads to walk translation tables.
>
> AArch64 does not require ordering between accesses that are made by a different Observer, even if an addresses dependency exists.

The ARM Architecture Reference Manual defines a procedure for safely changing
executable memory and ensuring visibility across cores. Paraphrasing from
section B2.2.5 "Concurrent modification and execution of instructions" [^3]:

1. Ensure that no other core is currently executing the region to be modified.
Note - it is legal under the architecture's rules for a core to be speculatively
executing code from this region, so long as the side effects from this
speculative execution adhere to the rules of the architecture. In practice, this
will happen sometimes.

2. Modify the memory and issue cache flush operations over the modified range
to ensure coherence between caches and other observers.

3. Before another core executes any of the modified instructions, it must issue
an instruction barrier (`ISB`). This step ensures that even if a core had
fetched or speculatively executed from the modified memory the results will be
discarded and the core will fetch and execute the intended instructions.

In a multiprocessor system with a preemptive operating system like Zircon, the
responsibility for adhering to this protocol in user space code modifications
such as JITs is split between the user space application and the kernel. The
application performing modifications is responsible for step (1) and for issuing
modifications and cache invalidations in the correct program order for (2). If
the operating system preempts a thread in the middle of these operations and
migrates to a different thread, it is responsible for inserting barriers and
synchronization mechanisms at least as strong as the program itself issues.
Step (3) can be assisted by the barrier mechanism proposed in this RFC, or it
can be performed by the application with care. The operating system has an
advantage in that it can track which cores are executing code that has the
relevant memory mapped in and thus can issue barriers on only the cores that
actually need it.

On ARMv8 exception entry and exit are defined as context serializing events
and thus migrating a thread from one core to another will necessarily include
such an event. This means that single threaded user space applications which
modify and execute memory can safely insert an instruction barrier between
these operations to ensure correctness. If the thread executes continuously
on a single core then that core will execute an instruction barrier before
jumping to the modified memory location. If the thread is interrupted or
rescheduled to another core at any point in the operation, either the barrier or
an exception exit will occur on the core before it can execute any modified
memory.

ARMv8.5 defines an extension FEAT_ExS which controls whether exception entry
and exit are context serializing events. Zircon does not support using this
feature and doing so would require consideration from applications similar to
the ones described for RISC-V below.

###### RISC-V

On RISC-V, the `FENCE.I` instruction is only standard architecturally defined
mechanism for ensuring that modifications to memory are visible to execution on
a core (or "hart" in the RISC-V terminology) [^4]. Exception entry and exit are
not guaranteed to provide this synchronization by the architecture so context
switches and thread migration are not inherently instruction fences. This means
that even single threaded user space applications that modify and then execute
memory require assistance from the kernel to ensure that an instruction fence is
executed on the hart that will executed the modified memory.

On Linux, user space applications are by default prohibited from issuing
`FENCE.I` in favor of issuing a system call to avoid this issue.

## Design

### zx_membarrier_sync_process_data(uint32_t options)

This synchronizes data memory accesses among all running threads within the
calling thread's process. If the process is part of a shared process group,
this will synchronize with all threads within the process group.

`options` must be zero.

### zx_membarrier_sync_process_insn(uint32_t options)

This synchronizes data memory accesses and the instruction stream among all
running threads within the calling thread's process. If the process is part of a
shared process group, this will synchronize with all threads within the process
group.

`options` must be zero.

## Implementation

The initial implementation will synchronize across all running CPUs in the
system by sending each an IPI to perform the appropriate synchronization.

## Ergonomics

This is a niche API for specialized high performance and subtle use cases.

## Backwards Compatibility

## Security considerations

Issuing redundant barriers will generally have no negative impact beyond
performance loss. Failing to issue a barrier when it is required can result in
loading or storing of unexpected data or execution of unexpected instructions
and so the logic for deciding where to issue barriers should be considered
security critical.

## Privacy considerations

## Testing

This feature is difficult to test directly as the absence of a barrier may
result in unpredictable behavior depending on microarchitectural details. A
barrier's existence is not directly visible and the absence of a barrier can
be hard to spot.

## Documentation

New Zircon system calls will be accompanied with standard system call
documentation. As the requirements for applications vary significantly between
the architectures that Fuchsia supports we should provide documentation of
recommended patterns and appropriate use of `zx_cache_flush()` on different
architectures.

## Drawbacks, alternatives, and unknowns

Starnix could emulate support for PRIVATE barriers by forcing all threads in
scope to exit restricted mode and issue a barrier instruction from normal mode
before allowing the calling thread to continue. This would be significantly
slower than a Zircon implementation in typical usage. Starnix does not have
insight into which threads are actually running on cores or which address
spaces are mapped in and the number of threads in a typical system is much
higher than the number of cores.

## Future work

### Scope

Barriers are required in conjunction with modifications to memory and the
required scope of the barrier is tied to the visibility of the modified memory.
Modifications to memory which is private to a single process or group of threads
requires issuing barriers on only the running threads which have access to that
memory. Thus, system-wide barriers are often broader that is necessary. The
Linux `membarrier` API provides several mechanisms for limiting the scope of
barriers to only threads that could be accessing the data or executing
instructions in the modified range. We will likely want to add similar
mechanisms to limit the scope of barriers in order to improve performance.

### Restartable sequences

Some algorithms using restartable sequences [^6] need to issue barriers that
interrupt restartable sequences running on either a specific core or on any
core. When we need to implement one of these algorithms we will need to
introduce a corresponding barrier operation.

## Prior art and references

[^1]: [man membarrier](https://man7.org/linux/man-pages/man2/membarrier.2.html)

[^2]: [ARM Learn the architecture - Memory Systems, Ordering, and Barriers](https://developer.arm.com/documentation/102336/0100/Instruction-barriers)

[^3]: arm/v8: B2.2.5 Concurrent modification and execution of instructions

[^4]: [“Zifencei” Instruction-Fetch Fence, Version 2.0](https://five-embeddev.com/riscv-user-isa-manual/Priv-v1.12/zifencei.html)

[^5]: [Linux membarrier-sync-core](https://www.kernel.org/doc/Documentation/features/sched/membarrier-sync-core/arch-support.txt)

[^6]: [RFC-0278: Restartable sequences](0278_restartable_sequences.md)

<!--
// LINT.ThenChange(//tools/rfc/test_data/rfc.golden.md)
-->
