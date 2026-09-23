# Starnix Read-Copy-Update (RCU)

This document describes the read-copy-update (RCU) synchronization approach used
by the Starnix kernel.

## Background

The Starnix kernel manages a vast amount of state accessed concurrently by many
threads. Any thread executing Linux userspace code can trigger a syscall or
receive an exception, causing that specific thread to transition into the
Starnix address space to execute kernel code. To handle this high level of
concurrency efficiently, Starnix employs various synchronization mechanisms,
including Read-Copy-Update (RCU).

RCU allows many threads to read shared data concurrently without blocking each
other or writers. To modify shared data, writers make a copy, modify that copy,
and then publish it to other threads—hence the name: read, copy, update. Because
threads might still be reading the old copy when the update happens, the memory
cannot be reclaimed immediately. Starnix waits until all active readers have
finished accessing the old data before freeing it; this waiting interval is
known as the "grace period."

RCU comes with these notable trade-offs:

*   **Weaker consistency guarantees**: RCU has weaker consistency than
    synchronization primitives like Mutexes or Read/Write locks. With RCU,
    readers may see stale data for a short time after a writer modifies it. This
    inconsistency is prevented in Mutexes and Read/Write locks because writers
    must wait for active readers to finish before modifying data, and readers
    must wait for active writers to finish before they can read.

*   **Deferred resource reclamation**: Resources associated with modified data
    are not freed immediately upon writing. Instead, they occupy memory until
    the grace period expires — that is, until all outstanding readers have
    finished. In this way, RCU resembles a garbage collector where object
    finalizers are deferred.

RCU was popularized by the Linux kernel and is now widely used in operating
system design. It is a good fit for Starnix because the kernel workload is
read-mostly, allowing the system to capitalize on the efficiency of RCU readers.
Additionally, Starnix rarely requires strict consistency because it aims to
replicate the semantics of the Linux UAPI. Since the Linux kernel already
implements these interfaces using RCU, Starnix can adopt the same weaker
consistency guarantees while correctly matching the expected behavior.

### `RcuHashMap` and `RcuCache`

Most RCU usage in Starnix should rely on high-level data structures like
[`RcuHashMap`][rcu-hash-map] and [`RcuCache`][rcu-cache]. These types leverage
Rust's type system to safely encapsulate the read-copy-update pattern, making
them significantly easier to use than low-level primitives. In most cases, these
structures should be preferred over wrapping a standard `HashMap` in a `Mutex`
or `RwLock`.

#### Example: Using `RcuHashMap`

`RcuHashMap` enables wait-free concurrent reads, while writes are synchronized
with an internal mutex. For example:

```rust
{% includecode gerrit_repo="fuchsia/fuchsia"
   gerrit_path="examples/rcu/src/rcu_hash_map_example.rs"
   region_tag="rcu_hash_map_example"
   adjust_indentation="auto" %}
```

### `RcuArc`, `RcuDroppableArc`, and `RcuOptionArc`

RCU also provides [`RcuArc`][rcu-arc], [`RcuDroppableArc`][rcu-droppable-arc], and [`RcuOptionArc`][rcu-option-arc],
which allow an `Arc` to be read and mutated concurrently with high efficiency.
These data structures are particularly efficient because they introduce no
additional storage overhead beyond the `Arc` itself. They should generally be
used as a replacement for wrapping an `Arc` in a `Mutex` or `RwLock`.

#### Example: Using `RcuDroppableArc`

`RcuDroppableArc` enables atomic updates to an `Arc`, allowing existing readers to
continue accessing the old value while a writer publishes a new one.

```rust
{% includecode gerrit_repo="fuchsia/fuchsia"
   gerrit_path="examples/rcu/src/rcu_arc_example.rs"
   region_tag="rcu_arc_example"
   adjust_indentation="auto" %}
```

## Relation to `register_delayed_release()`

Starnix currently maintains two separate mechanisms for deferring object
release: RCU and [`register_delayed_release()`][delayed_release]. While
`register_delayed_release()` is eventually planned to be reimplemented using
RCU, the two currently operate as independent pools. At the moment, both pools
are drained independently, though their processing is triggered at the same
execution safe points.

## Implementation status

The current RCU implementation is built using atomics and futexes. This approach
is efficient enough that migrating from `Mutex` and `RwLock` to RCU has yielded
measurable improvements in various benchmarks, including single-threaded
microbenchmarks. Ongoing work is focused on a new implementation based on
*restartable sequences* (RSEQ), which will further optimize the read path by
eliminating the need for atomic operations entirely.

Generally, new code should use RCU whenever a suitable high-level data structure
is available. As the library of RCU-backed structures expands, more areas of
Starnix will be able to leverage these performance benefits.

[rcu-hash-map]: /src/starnix/lib/starnix_rcu/src/rcu_hash_map.rs
[rcu-cache]: /src/starnix/lib/starnix_rcu/src/rcu_cache.rs
[rcu-arc]: /src/lib/fuchsia-rcu/src/rcu_arc.rs
[rcu-droppable-arc]: /src/lib/fuchsia-rcu/src/rcu_droppable_arc.rs
[rcu-option-arc]: /src/lib/fuchsia-rcu/src/rcu_option_arc.rs
[delayed_release]: /src/starnix/kernel/core/task/delayed_release.rs
