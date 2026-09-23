# KOID <-> PID remapping in Starnix

This document describes the design of the shared Zircon-koid to Linux-pid/tid
remapping infrastructure implemented in [`tracing.rs`](tracing.rs).

## Problem

Starnix tasks have two identities: a Linux pid/tid inside the container, and
the koids of the Zircon process/thread that back them. Two independent clients
need to translate between them, in opposite directions:

| Client | Data comes from | Data goes to | Translation |
|---|---|---|---|
| CPU profiling (`perf_event_open`) | Fuchsia profiler (koids) | Linux `traced_perf` (pids) | koid to pid/tid |
| System tracing (`perfetto_consumer`) | Linux atrace (tids) | Fuchsia trace session (koids) | tid to koid |

The profiler's FXT backtrace records are stamped with the koids the Fuchsia
profiler sampled, but `PERF_RECORD_SAMPLE` consumers expect Linux pid/tid. The
perfetto consumer receives events carrying Linux tids, but the Fuchsia trace
convention (and the Perfetto Fuchsia importer) identifies threads by koid.

Both translations must work for threads that exit before their data is
processed, and neither may resolve identities by locking `kernel.pids` on hot
paths.

The clients do not share a lookup *direction*, but they can share the
*recording*: one write pipeline feeding a bidirectional map, with two read
views.

## Design

Three types with distinct roles:

- **`PidKoidMap`** (private): the data. Three `HashMap`s (`tid ->
  ZirconIdentity`, `pid -> Koid`, `koid -> LinuxIdentity`) kept consistent by a
  single `insert`. Entries are only added, never removed, while recording is
  active, so data can be resolved past a task's lifetime; the whole map is
  released when the last session drops. Seeding merges with `or_insert` so a
  snapshot can never overwrite a fresher entry recorded by a concurrently
  spawning task.

  A `ZirconIdentity` always holds both koids: only tasks with a complete
  identity are stored. A task observed mid-creation by the seed (Zircon thread
  handle not yet attached) is skipped and records itself moments later, on the
  startup path that attaches the handle just before recording; during the gap
  its tid cannot appear in trace data because its creator's syscall has not
  returned. Starnix kernel threads, which have no corresponding Linux process
  (and therefore no Linux pid/tid), are intentionally excluded.

- **`TracePerformanceEventManager`**: the kernel-wide singleton owning the map
  (behind one `RwLock`) and the recording lifecycle. It is a direct field on
  `Kernel` (`kernel.trace_event_manager`), created in `Kernel::new` with a
  `Weak<Kernel>` back-reference and alive for the kernel's lifetime; whether
  recording is active is the session count inside it, never its presence.
  When idle it holds empty `HashMap`s and an atomic counter.

- **`PidKoidSession`**: one client's RAII recording interest and the exclusive
  query interface, obtained from `manager.open()`. The first session seeds the
  map from `kernel.pids` (the only time that lock is taken); dropping the last
  session releases the map. Because cleanup is driven by `Drop`, it runs on
  every exit path, and because resolution methods only exist on the session,
  holding an active session is enforced at compile time for every lookup.

New tasks record themselves in `Task::record_pid_koid_mapping` during task
creation, gated by `manager.is_recording()` before any task state is queried.

### Session count transitions

A dedicated `state_lock: Mutex<()>` serializes session lifecycle transitions
(0 -> 1 seeding and 1 -> 0 cleanup), while `active_sessions: AtomicUsize`
provides a zero-lock atomic check on the hot recording path. When the
first session opens, `state_lock` is held while taking the `kernel.pids`
snapshot and seeding the map before setting `active_sessions` to 1. When the
last session drops, `state_lock` is held while clearing the map before setting
`active_sessions` to 0. Unbalanced drops are impossible by construction (the
guard is the only caller); a stray internal stop is detected and logged rather
than underflowing.

### Why the manager is a `Kernel` field, not an expando entry

`expando.peek()` acquires the expando's internal mutex on every call, and
`record_pid_koid_mapping` runs on every task spawn forever. A direct field
makes the idle fast path a single atomic load with no locks at all.

## Locking strategy

The one lock that must stay off hot paths is `kernel.pids`; it is taken
exactly once per recording epoch (the seed on the first `open()`) and never
again. The map's own `RwLock` is a private, dedicated lock, and readers take it
per lookup:

| Path | Frequency | Cost |
|---|---|---|
| Task creation, nothing recording | constant background | 1 acquire atomic load |
| Task creation, recording active | rare (spawns) | 1 short `write()` |
| Profiler sample resolution | batch at collection | 1 `read()` per record |
| Perfetto tid-to-koid lookup | batch at trace flush | 1 `read()` per lookup |
| Session open / last drop | ~once per trace | 1 `write()` (seed / release) |

Per-lookup read locks are a deliberate choice, based on measuring the actual
workload rather than assuming locks are expensive:

- Both read paths are **post-processing batches** (draining samples at
  disable, converting the trace buffer at stop). Nothing in the running
  system waits on them; only the collection pipeline itself does.
- Both consumers are **single-threaded**, so reader-vs-reader serialization
  cannot occur, and readers never block each other on an `RwLock` anyway. The
  only contention is the rare task-spawn write (one insert, roughly 100 ns),
  against which an uncontended `read()` costs tens of nanoseconds. Even a
  worst-case flush of about a million lookups adds tens of milliseconds to a
  multi-second batch.
- Live reads are also **more correct** than snapshots: a record parsed at
  time T sees every mapping written before T, so a thread sampled moments
  before a collection still resolves. A snapshot taken at batch start would
  miss threads that spawn while the batch drains.

## Alternatives considered

- **Manual `start_recording`/`stop_recording` calls instead of an RAII
  guard**: requires every client to pair calls on every exit path, including
  errors and kthread teardown; a missed stop leaks recording until reboot.
  The guard makes cleanup structural.
- **Per-client lookup cache in front of the map**: zero-lock cache hits,
  but adds staleness/invalidation logic and a second copy of the table, to
  avoid an uncontended lock with a single-threaded reader.
- **Per-batch snapshot clones**: lock-free per-record resolution, but
  reintroduces a window where late-spawning threads resolve stale. Kept as the
  documented escape hatch if lookup volume ever grows by orders of magnitude.
- **Shared `Mutex` cache inside the manager**: strictly worse; it keeps a
  second table and takes an exclusive lock per lookup, serializing readers
  the `RwLock` would let run in parallel.
- **RCU / copy-on-write map** (readers never lock): writers would clone the
  map on every task spawn to optimize reads that are not a bottleneck.
- **Seqlock (`RwSeqLock`)**: wants small, copyable, torn-read-tolerant data;
  wrong tool for `HashMap`s.
- **Sharded / concurrent maps**: solve write contention this workload does
  not have.
