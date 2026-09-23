<!-- Generated with `fx rfc` -->
<!-- mdformat off(templates not supported) -->
{% set rfcid = "RFC-0278" %}
{% include "docs/contribute/governance/rfcs/_common/_rfc_header.md" %}
# {{ rfc.name }}: {{ rfc.title }}
{# Fuchsia RFCs use templates to display various fields from _rfcs.yaml. View the #}
{# fully rendered RFCs at https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs #}
<!-- SET the `rfcid` VAR ABOVE. DO NOT EDIT ANYTHING ELSE ABOVE THIS LINE. -->

<!-- mdformat on -->

<!-- This should begin with an H2 element (for example, ## Summary).-->

## Problem Statement

Starnix needs to implement efficient synchronization primitives, such as
Read-Copy-Update (RCU), in order to be performance-competitive with Linux.
Restartable sequences are a key mechanism for implementing efficient
synchronization primitives. For example, in order to implement RCU efficiently,
we need per-CPU counters to track the number of times a CPU has entered and
exited a read critical section. Restartable sequences allow us to implement
these per-CPU counters efficiently.

## Summary

This RFC proposes to add restartable sequences to Zircon. The design is based
on the restartable sequences interface in Linux, which lets us reuse algorithms
designed for the Linux interface rather than invent new algorithms for the
subtle synchronization primitives we need to implement.

### Preliminary Performance Data

To evaluate the benefits of Read-Copy-Update (RCU) synchronization in Starnix,
we implemented RCU with atomic operations and used that implementation for
the file descriptor table, replacing a pair of nested mutexes. We observed that
performance improved, even on an existing single-threaded benchmark.

To investigate further, we looked at a suite of benchmarks that measure the
performance of using `poll` and `select` with a large number of file
descriptors. These syscalls need to do a lookup in the file descriptor table for
each specified file descriptor.

As an experiment, we wrapped the loop that looks up each file descriptor in a
read critical section. Instead of using a pair of atomic operations for the
read critical section in each iteration of the loop, we used a single pair of
atomic operations for the entire loop. The nested read critical sections are
counted using thread-local storage. We observed the following performance
improvements in these gvisor benchmarks:

```
PollAllEvents/1024  improved  0.961-0.998  675201 +/- 6073 ns  661317 +/- 6534 ns
PollAllEvents/512   improved  0.964-0.980  268782 +/- 1324 ns  261265 +/- 977 ns
PollAllEvents/64    improved  0.962-0.991  36108 +/- 233 ns    35252 +/- 300 ns
SelectAllEvents/64  improved  0.946-0.995  30606 +/- 483 ns    29695 +/- 290 ns
```

With the restartable sequences interface, we will use per-CPU counters rather
than atomics to count top-level read critical sections. This should provide
performance improvements similar to the thread-local storage implementation we
currently use to count nested read critical sections. For this read, we expect
to see the above performance improvements without needing to fine-tune the
`poll` and `select` implementations.

## Stakeholders

Who has a stake in whether this RFC is accepted? (This section is optional but
encouraged.)

_Facilitator:_

The person appointed by FEC to shepherd this RFC through the RFC
process.

_Reviewers:_

- eieio@google.com
- jamesr@google.com
- maniscalco@google.com

_Consulted:_

List people who should review the RFC, but whose approval is not required.

_Socialization:_

This RFC was socialized by discussing the need for restartable sequences with
the Zircon team while the author worked on an implementation of Read-Copy-Update
based on atomic operations.

## Requirements

The design must let userspace implement per-CPU counters with minimal
overhead. The design must not introduce noticeable overhead for programs that do
not use the feature. The design must be efficient when used by thousands of
threads simultaneously.

## Design

The core idea behind a restartable sequence is to let userspace specify a
sequence of operations that it wants to execute without being preempted. If the
kernel preempts the userspace thread while the userspace thread is executing
the sequence of operations, rather than resuming the userspace thread at the
point of preemption, the kernel will resume the userspace thread at an abort
handler. Typically, the abort handler will jump to the beginning of the
sequence of operations and retry the sequence of operations.

### Example: Per-CPU Counter

To prepare to run the restartable sequence, userspace should use a volatile read
to read the current CPU ID into a local variable. Userspace should then look up
the address of the per-CPU counter for that CPU. Userspace should then use
volatile writes to inform Zircon about the location of the code for the
restartable sequence. Finally, userspace should jump to the start of the
restartable sequence.

The restartable sequence itself is typically written in assembly and performs
the following sequence of operations:

 1. Read the current CPU ID into a register.
 2. Check that the current value matches the value read earlier. If not, abort
    the sequence.
 2. Read the current value of the per-CPU counter from the address determined
    earlier into a register.
 3. Add or subtract one from that register.
 4. Write that register back to the address determined earlier.

If these steps all operate on the same CPU, then they will implement a per-CPU
counter with minimal overhead. If the kernel preempts the userspace thread
while the userspace thread is executing the sequence of operations, the kernel
will resume the userspace thread at the abort handler and retry the sequence. As
long as the write operation is the last assembly instruction in the sequence,
interrupting and restarting the sequence will not have any observable
consequences.

### Minimizing overhead

To minimize the overhead of a restartable sequence, we need to let userspace
specify the sequence of operations without issuing a syscall because the cost
of the syscall would overwhelm the performance benefits we seek to gain. Linux
solves this problem by sharing memory between userspace and the kernel.

### Shared data structure

Before using restartable sequences, a thread registers a region of memory with
the kernel. This memory has the following layout:

```c
typedef struct zx_rseq {
  uint32_t cpu_id;
  uint32_t reserved;
  uint64_t start_ip;
  uint64_t abort_ip;
  uint64_t post_commit_offset;
} __ALIGNED(32) zx_rseq_t;
```

At most one instance of the `zx_rseq_t` structure can be registered with the
kernel for a given thread.

The kernel uses the `cpu_id` fields to tell userspace which CPU the thread is
currently executing on. Userspace typically reads the `cpu_id` field before
entering the restartable sequence to prepare for the sequence, for example by
looking up addresses for a per-CPU data structure. Upon entering the restartable
sequence, userspace reads the `cpu_id` field again and checks that the thread is
still running on the expected CPU. If the values do not match, userspace will
typically retry the restartable sequence.

Userspace uses the `start_ip` and `post_commit_offset` fields to define the
critical section for the restartable sequence by telling the kernel where
the instructions for the restartable sequence start and end. If the kernel
preempts the thread while the thread is executing the restartable sequence, the
kernel will resume the thread at the `abort_ip` rather than at the instruction
at which the thread was preempted.

Userspace should initialize the `zx_rseq_t` data structure to zeroes. When the
data structure is registered with the kernel, Zircon will populate the `cpu_id`
to ensure the value is always valid.

Details:

 * The `cpu_id` is the CPU ID of the CPU on which the thread is currently
   executing. This field must never be written by userspace and must not be read
   on any thread other than the thread that registered the restartable sequence.
 * The `reserved` field is reserved for future use. Userspace must initialize
   this field to zero.
 * The `start_ip` is either zero or the address of the first instruction in the
   restartable sequence. This field must not be accessed on any thread other
   than the thread that registered the restartable sequence.
 * The `post_commit_offset` is either zero or the offset from the `start_ip` to
   the address after the last instruction in the restartable sequence. This
   field must not be accessed on any thread other than the thread that
   registered the restartable sequence.
 * The `abort_ip` is instruction pointer at which the kernel should resume
   the thread if it preempts the thread while the thread is executing the
   restartable sequence. This field must not be accessed on any thread other
   than the thread that registered the restartable sequence.

### Restricted mode

We do not currently have a need for supporting restartable sequences when a
thread is executing in restricted mode. We can simplify the design by not
supporting restartable sequences in restricted mode. However, we still need to
consider how Zircon should behave when preempting a thread that is running in
restricted mode.

When a thread is running in restricted mode, the thread cannot be in the
critical section for a restartable sequence because restartable sequences are
not supported in restricted mode. However, the kernel still needs to update the
`cpu_id` field before the thread exits restricted mode.

Rather than update the `cpu_id` field immediately, Zircon defers updating the
field until the thread exits restricted mode. A thread can exit restricted mode
for a variety of reasons, but one reason a thread might exit restricted mode is
due to an exception. While processing this exception, Zircon is not prepared to
handle another nested exception, which means Zircon needs to be able to update
the `cpu_id` field with interrupts disabled.

### Registration

To let Zircon read and write the `zx_rseq_t` data structure with interrupts
disabled, Zircon will create a kernel mapping for the `zx_rseq_t` data structure
when a thread registers a restartable sequence. Userspace indicates the location
of this data structure by specifying the VMO, offset, and length of the data
structure using the `zx_thread_set_rseq` syscall:

```cpp
zx_status_t zx_thread_set_rseq(zx_handle_t vmo, uint64_t offset, uint64_t size);
```

To register a restartable sequence, userspace passes a handle to the VMO that
contains the `zx_rseq_t` data structure. The `offset` parameter is the offset
of the start of that data structure within the VMO. The `offset` must be aligned
to `alignof(zx_rseq_t)`. The `size` parameter must be `sizeof(zx_rseq_t)`.

To unregister a restartable sequence, userspace can pass `ZX_HANDLE_INVALID` for
the `vmo` parameter to `zx_thread_set_rseq`. In this usage, the `offset`
and `size` parameters must be zero.

The syscall returns the following errors:

**ZX_ERR_INVALID_ARGS**  *offset* is not a multiple of `alignof(zx_rseq_t)`,
*size* is not `sizeof(zx_rseq_t)`, or the region of memory defined by
*offset* and *size* spans a page boundary.

**ZX_ERR_INVALID_ARGS**  *vmo* is not directly writable, has been
marked as uncached, or is backed by a user pager.

**ZX_ERR_ALREADY_EXISTS**  The thread already has a restartable sequence
registered.

**ZX_ERR_BAD_HANDLE**  *vmo* is neither a valid handle nor `ZX_HANDLE_INVALID`.

**ZX_ERR_WRONG_TYPE**  *vmo* is not a VMO.

**ZX_ERR_ACCESS_DENIED**  *vmo* is missing `ZX_RIGHT_READ`, `ZX_RIGHT_WRITE`, or
`ZX_RIGHT_DUPLICATE`.

## Implementation

### Registration

To register a restartable sequence for a thread, Zircon will create a kernel
mapping for the page in the VMO that contains the range specified in the
`zx_thread_set_rseq` syscall. Notice that `alignof(zx_rseq_t)` and
`sizeof(zx_rseq_t)` are arranged such that a properly aligned `zx_rseq_t` will
never straddle a page boundary. Zircon will remove the kernel mapping when the
restartable sequence has been unregistered or the thread that registered the
mapping terminates.

Rather than creating a separate kernel mapping for every restartable sequence,
Zircon should reuse mappings of the same page from the same VMO. Other Zircon
interfaces that require similar kernel mappings should use the same mapping
cache for efficiency.

### Critical sections

When the Zircon scheduler switches a CPU from one thread to another, the
scheduler will check whether the new thread has a restartable sequence
registered. If so, the scheduler will raise the `THREAD_SIGNAL_CHECK_RSEQ`
signal on that thread.

When the thread is processing its signals, if the `THREAD_SIGNAL_CHECK_RSEQ`
signal is set:

 1. Check if the thread still has a restartable sequence registered. If not,
    skip the remaining steps.

 2. Write the current CPU ID to the `cpu_id` field of the `zx_rseq_t` via the
    kernel mapping.

 3. Read the `start_ip`, `post_commit_offset`, and `abort_ip` fields via the
    kernel mapping. If the userspace instruction pointer for the current thread
    is within the range specified by the `start_ip` and `post_commit_offset`,
    update the userspace instruction pointer to the `abort_ip`.

## Performance

The overhead added by this functionality should be minimal, but we will need to
evaluate the impact using existing Starnix benchmarks.

## Ergonomics

This interface is very difficult to use correctly. We do not expect many
programs to use the interface directly. Instead, we expect that most users will
use higher level abstractions built on top of this interface, such as RCU.

## Backwards Compatibility

This proposal does not affect backwards compatibility. The proposal is designed
to be forward compatible with a future requirement to implement Linux
restartable sequences in Starnix.

## Security considerations

This proposal does not have any security implications. The net result is that
a thread can manipulate its own instruction pointer. The feature will need to
have the same safeguards as `zx_thread_write_state` to prevent userspace from
setting the instruction pointer to invalid values.

The interface provides more visibility into the scheduling semantics of the
system, but those semantics are already visible by way of the high resolution
clocks Zircon provides to userspace.

If we were to let code running in restricted mode register a restartable
sequence, we would need to ensure that `zx_rseq_t` data structure that can be
manipulated in restricted mode cannot be used to manipulate the instruction
pointer of a thread running in normal mode. For example, we might need to have
separate `zx_rseq_t` data structures for normal and restricted mode.

## Privacy considerations

This proposal does not have any privacy implications.

## Testing

We will test this feature using core tests and by stress testing the RCU
implementation used by Starnix.

## Documentation

This interface will be documented in the Zircon interface documentation.

## Drawbacks, alternatives, and unknowns

The main drawbacks of this proposal are the additional complexity it adds to the
scheduler and the difficulty of using the interface correctly.

The primary alternative to this proposal is to not implement restartable
sequences at all. This would mean that Starnix would continue to use its current
implementation of RCU based on atomic operations and operate at lower
performance. At the moment, RCU is not used very extensively by Starnix.
However, given our initial implementation experience, we anticipate using RCU
more extensively and in performance-critical parts of Starnix, such as the
SEStarnix access vector cache.

Additionally, we are likely to use restartable sequences to implement more
per-CPU data structures, such as a per-CPU cache for SEStarnix access vectors,
in cases where parallel performance is critical.

The primary unknown is whether we will need to implement the Linux restartable
sequence interface in Starnix. This proposal does not contain all the
functionality required to implement that interface.

## Prior art and references

The main prior art for restartable sequences is the Linux interface.
Additionally, there is prior art implementing RCU using restartable sequences
by the person who designed and implemented restartable sequences in Linux.
