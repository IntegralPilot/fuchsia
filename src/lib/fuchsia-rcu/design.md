# Fuchsia RCU Design

This document describes how the Fuchsia RCU crate uses atomics and the C++20
memory model to synchronize access to data.

## Usage Model

You can use `RcuBox` to create a `Cell<Box>` equivalent that can be used
concurrently from multiple threads. Many threads can read from the cell
concurrently and do not block on writers. When the cell is written, reads may
continue to see the old value of the cell for some period of time.

```rust
struct MyStruct {
  a: usize,
  b: usize,
}

struct SharedStruct {
  foo: RcuBox<MyStruct>
}
```

### Read

To read the value of the cell, use the `read()` method:

```rust
fn my_reader(x: Arc<SharedStruct>) {
  let guard = x.foo.read();
  println!("foo.a: {}", guard.a);
  println!("foo.b: {}", guard.b);
}
```

The values of `a` and `b` will be consistent with each other as long as their
are read from the same `RcuReadGuard` returned from `read()`. However, another
thread running concurrently might see different values for `a` and `b` when
reading this cell.

### Write

To write the value of the cell, use the `set()` method:

```rust
fn my_writer(x: Arc<SharedStruct>) {
  x.foo.set(MyStruct { a: 42, b: 24 });
}
```

After `set` returns, future calls to `read()` will observe the new values for
`a` and `b`, but concurrent readers can still observe the old values. The
storage for the old values will not be reclaimed until all the concurrent
readers have dropped their read guards and the RCU state machine has made
sufficient progress.

### Progress requirements

In order for the RCU state machine to make progress, the program using
`fuchsia-rcu` must periodically call `rcu_synchronize`. Otherwise, memory
allocated during `RcuBox::set` will never be freed.

## Low-level interface

`RcuBox` and similar high-level data structures are built on top of a low-level
interface to the RCU state machine. This interface synchronizes access to
objects referenced through `std::sync::atomic::AtomicPtr`.

The RCU library also provides `RcuPtr`, which is a thin wrapper around an
`AtomicPtr` and the low-level interface described below. Clients that require
low-level access to the RCU state machine should use `RcuPtr` rather than
calling the low-level interface directly.

### Read

The interface for reading objects managed by RCU is as follows:

```rust
fn rcu_read_lock()
fn rcu_read_unlock()
fn rcu_read_pointer(ptr: &AtomicPtr<T>) -> *const T
```

To read from an object, a client must first call `rcu_read_lock()` and then call
`rcu_read_pointer()`. The returned pointer remains valid until the balancing
call to `rcu_read_unlock()`, after which time the client must not dereference
the pointer.

A given thread can acquire nested RCU read locks. Threads running concurrently
might receive pointers to different objects when calling `rcu_read_pointer()`
with the same `AtomicPtr`.

## Write

The interface for writing objects managed by RCU is as follows:

```rust
fn rcu_assign_pointer(ptr: &AtomicPtr<T>, new_ptr: *mut T)
fn rcu_replace_pointer(ptr: &AtomicPtr<T>, new_ptr: *mut T) -> *mut T
fn rcu_call(callback: impl FnOnce() + Send + Sync + 'static)
fn rcu_drop<T: Send + Sync + 'static>(value: T)
```

To write an object, a client first creates a new instance of the object and then
uses either `rcu_assign_pointer()` or `rcu_replace_pointer()` to store a pointer
to that object in an `AtomicPtr`. The `rcu_assign_pointer()` operation discards
whatever value was previously stored in this pointer, whereas the
`rcu_replace_pointer()` operation returns the previous value of the `AtomicPtr`.

Typically, the caller wishes to clean up whatever object was previously stored
in the `AtomicPtr`. However, the caller cannot clean up that object immediately
because there might be other threads that are currently reading from that
object. Instead, the caller uses `rcu_call()` to schedule a callback that will
run after all the currently in-flight readers have completed.

The RCU library provides `rcu_drop()`, which is a convenience function that
schedules the given object to be dropped once all the currently in-flight
readers have completed.

### Progress

The interface for progressing the RCU state machine is as follows:

```rust
fn rcu_synchronize()
```

Clients must call at least one of these functions periodically to ensure that
callbacks scheduled with `rcu_call()` eventually happen.

The `rcu_synchronize()` function blocks until the RCU state machine has advanced
sufficiently to call all the callbacks that were scheduled prior to calling
`rcu_synchronize()`.

## State Machine

To show that the RCU state machine provides the necessary synchronization
properties, we need to show two properties about a given object `o` protected by
the RCU mechanism:

 1. All the writes to `o` happen before all the reads from `o`.
 2. All the reads from `o` happen before we run the RCU callback (i.e., the
    callback scheduled with `rcu_call()`) associated with `o`.

First, we will state the forms of the relevant operations and then we will show
that they have desired synchronization properties.

### States

#### Idle

The RCU state machine starts in the *Idle* state. In this state, readers can
begin read operations, which are counted using the `read_counters`. The state
machine remains in this state until the next call to `rcu_synchronize`.

There are no preconditions for leaving the *Idle* state. The post condition for
leaving the *Idle* state is that the `callback_chain` has been moved to the
`waiting_callbacks` queue, the `generation` counter has been increased, and the
state machine is in the *Waiting* state.

#### Waiting

In the *Waiting* state, existing readers complete and decrement their
`read_counter`. New readers can begin read operations, and increment a different
`read_counter`. The state machine remains in this state until the precondition
for leaving the *Waiting* state has been obtained.

The precondition for leaving the *Waiting* state is that the `read_counter` for
the previous generation has reached zero. This condition indicates that all the
read operations that were in flight when the state machine entered the *Waiting*
state have completed.

The postcondition for leaving the *Waiting* state is that the front entry in the
`waiting_callbacks` queue has been removed (advancing all the callbacks in the
queue) and the state machine is in the *Idle* state.

After leaving the *Waiting* state, the set of callbacks removed from the
`waiting_callbacks` queue run, potentially on a different thread.

### Operations

#### Writes

All the writes to `o` have the following form:

```rust
[create and initialize o]
let p = [address of o];
rcu_assign_ptr(&ptr, p); // or rcu_replace_pointer()
```

#### Reads

All the reads from `o` have the following form:

```rust
rcu_read_lock();
let p = rcu_read_pointer(&ptr);
[read from o via p]
rcu_read_unlock();
```

#### Callbacks

All the callbacks associated with `o` have the following form:

```rust
let another_pointer = [address of another object or null];
let p = rcu_replace_pointer(&ptr, another_pointer);
rcu_call(|| {
  [operate on p, e.g., freeing o]
});
```

### Writes Happen Before Reads

Readers call `rcu_read_pointer()` before reading from `o`, which contains an
`Ordering::Acquire` load of the `AtomicPtr` to `o` at synchronization point
`[D]`. This load _synchronizes-with_ the `rcu_assign_ptr()` operation performed
by the writer because `rcu_assign_ptr()` contains an `Ordering::Release` store
of the same `AtomicPtr` at synchronization point `[E]`. For this reason, the
write to `o` _happens-before_ the read from `o`.

### Reads Happen Before Callbacks Run

Assume, without loss of generality, that the RCU state machine starts in the
*Idle* state and that the `generation` is even. Consider a given reader thread
reading from `o` and another thread scheduling a callback associated with `o`.
Before we run that callback, the state machine will need to advance through the
*Idle* and *Waiting* states twice.

In this sequence, there are three atomic operations with `Ordering::SeqCst`:

 1. Synchronization point `[A]` in `rcu_read_lock()`.
 2. Synchronization point `[C1]` the first time through the *Waiting* state in
    `rcu_synchronize()`.
 3. Synchronization point `[C2]` the second time through the *Waiting* state in
    `rcu_synchronize()`.

The memory model guarantees that these three operations happen in a single total
order. The mutex that protects the `rcu_control_block` ensures that `[C1]` is
always before `[C2]` in the single total order. The argument for correctness is
different depending on whether `[A]` is before or after `[C1]` in the single
total order.

#### `[A]` is before `[C1]`

When the reader loads the `generation` count, the reader will either load an
even or an odd generation number. If the `generation` count is even, then the
load at `[C1]` will _synchronize-with_ the decrement in `rcu_read_unlock()`
(synchronization point `[B]`) because both operate on `read_counter[0]` (recall
that we assumed the `generation` count was originally even). If the loaded
`generation` count is odd, then the load at `[C2]` will _synchronize-with_ the
decrement in `rcu_read_unlock()` because both operate on `read_counter[1]`.
Either way, the read from `o` _happens-before_ the state machine exits
the *Waiting* state the second time.

#### `[C1]` is before `[A]`

We will show the _happens-before_ relation for the following synchronization
points, in order: `[F]` in `rcu_replace_pointer()`, `[G]` in `rcu_call()`, `[H]`
in the *Idle* state of `rcu_synchronize()`, `[C1]` in the *Waiting* state
of `rcu_synchronize()`, `[A]` in `rcu_read_lock()`, and `[D]` in
`rcu_read_pointer()`:

 * `[F]` _happens-before_ `[G]` because `[F]` is _sequenced-before_ `[G]`.
 * `[G]` _happens-before_ `[H]` because `[G]` _synchronizes-with_ `[H]`.
 * `[H]` _happens-before_ `[C1]` because of the mutex that protects the
   `rcu_control_block`.
 * `[C1]` _happens-before_ `[A]` because `[C1]` precedes `[A]` in the single
   total order.
 * `[A]` _happens-before_ `[D]` because `[A]` is _sequenced-before_ `[D]`.

Therefore, the pointer to `o` stored in the `AtomicPtr` is replaced with another
value before being loaded by the reader, which means the reader does not
actually read from `o`. The callback runs after all the reads from `o` because
there are no reads from `o`.

*Note:* The logic described above suggests that the sequential consistency of
`[C1]` and `[A]` combined with the synchronization between `[G]` and `[H]` is
sufficient. However, this reasoning relies on transitivity across three
different threads (Writer -> Synchronizer -> Reader), which is not automatically
guaranteed by the memory model. A model checker (see
https://fxbug.dev/484397559) has shown that this sequence does not strictly
guarantee that the reader observes the new pointer. As a result, `rcu_call` now
includes explicit synchronization with `rcu_read_lock` to cover this case.

## RSEQ Backend

When the `rseq_backend` feature is enabled, the RCU implementation uses
Restartable Sequences (RSEQ) and membarriers to optimize the read-side critical
path. This approach avoids atomic read-modify-write operations on shared cache
lines, which significantly improves scalability on many-core systems.

### RSEQ Primitives

RSEQ allows us to perform per-CPU operations atomically with respect to other
threads on the same CPU. We use this to maintain per-CPU read counters.

The per-CPU state consists of two pairs of counters (one pair for each
generation):

- `begin`: Incremented when entering a read-side critical section.
- `end`: Incremented when exiting a read-side critical section.

A given read-side critical section might begin on one CPU and end on a different
CPU. In that case, the `begin` counter will be incremented on the first CPU, and
the `end` counter will be incremented on the second CPU. For this reason, the
absolute value of the `begin` and `end` counters is not meaningful, only the
difference between the `begin` and `end` counters for a given generation is
meaningful.

To determine if there are any active readers for a given generation, we sum the
negated `end` counter and the `begin` counter over all CPUs. If the sum is
zero, then there are no active readers.

### Barrier Pairing

1.  **Read-Side**: `rcu_read_lock()` and `rcu_read_unlock()` issue a
    `compiler_fence(Ordering::SeqCst)`. This barrier prevents the compiler from
    reordering the critical section outside the counter increments.

2.  **Writer-Side**: `rcu_synchronize()` (specifically `has_active_readers`)
    issues a system barrier (`zx_membarrier_sync_process_data`).

The pairing works as follows:

-   `rcu_read_lock()`:
    1.  Increment `begin` (RSEQ).
    2.  `CompilerBarrier`.
    3.  Critical Section.

-   `rcu_read_unlock()`:
    1.  Critical Section.
    2.  `CompilerBarrier`.
    3.  Increment `end` (RSEQ).

-   `rcu_synchronize()` (checking for quiescence):
    1.  Sum all `end` counters (negated).
    2.  System barrier (`zx_membarrier_sync_process_data`).
    3.  Sum all `begin` counters (positive).

If `Sum(begin) - Sum(end) == 0`, then all readers that started before the RSEQ
barrier have completed.

The critical ordering is reading `end` *before* the barrier and `begin` *after*
the barrier. There are four cases to consider:

 1. The advancer observes the increment to `begin` but not to `end`. This
    observation implies that there is an active reader and the advancer will
    continue to wait.
 2. The advancer observes the increment to both `begin` and `end`. This
    observation implies the read critical section is complete and the advancer
    can proceed.
 3. The advancer observes the increment to neither `begin` nor `end`. This
    observation implies that the read critical section belongs to the next
    generation and the advancer can proceed.
 4. The advancer observes the increment to `end` but not to `begin`. This
    observation would be problematic because the advancer would incorrectly
    think there were fewer active readers than there actually were.

The correctness of this barrier pairing relies on the total order `S` of memory
operations guarantees by the memory model. Specifically, the
`compiler_fence(Ordering::SeqCst)` in `rcu_read_lock()` ensures that the
increment to `begin` is _sequenced-before_ the critical section, and the
critical section is _sequenced-before_ the increment to `end`.

The `zx_membarrier_sync_process_data()` in `has_active_readers` ensures that if
the advancer observes the increment to `end` (at step 1), it must also observe
the increment to `begin` (at step 3). This is because the store to `begin`
_happens-before_ the store to `end` in the reader thread (due to program order
and the compiler fence). When `zx_membarrier_sync_process_data` acts as a
memory barrier, it ensures that all stores sequenced-before the barrier
interruption point in the reader are visible to the advancer after the barrier.
Since `begin` is sequenced-before `end`, if `end` is visible, `begin` must also
be visible.  Therefore, Case 4 is impossible: the advancer will never
underestimate the number of active readers.

### Update Visibility Barrier

In addition to the barrier in `has_active_readers`, the RSEQ backend requires a
barrier to ensure that pointer updates made by writers are visible to readers
that start *after* the grace period begins.

Without this barrier, a race could occur:
1. A writer updates a pointer and queues a callback.
2. The state machine advances the generation (e.g., from 0 to 1).
3. A reader starts on another CPU, reads the new generation (1), and registers
   in the new generation's counters. However, because it doesn't use memory
   barriers, it might still see the *old* pointer value if the store hasn't
   propagated.
4. The state machine checks for active readers for generation 0. Since the
   reader registered in generation 1, it is ignored.
5. The grace period for generation 0 ends, and the callback runs, freeing the
   old data while the reader is still accessing it.

To prevent this race, `rcu_grace_period()` issues a
`zx_membarrier_sync_process_data()` before advancing the generation counter.
This forces all prior stores (including the pointer updates) to be visible to
all CPUs. Consequently, any reader that observes the new generation is
guaranteed to see the new pointer value.

