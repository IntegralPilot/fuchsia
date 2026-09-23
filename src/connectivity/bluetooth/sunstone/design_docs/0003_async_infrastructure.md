---
title: Async infrastructure for Sunstone
description: >
    Defines the underlying asynchronous foundational
    infrastructure to build a reliable, embedded-friendly Bluetooth stack
status: approved # [approved | declined | implemented | superseded]
authors:
  - asnaider@google.com
tags:
  - async
  - channels
  - broadcast
  - rpc
  - testing
bugs:
  - 518915192
rfc: 0003
---

# Objective

In order to build a reliable, embedded-friendly Bluetooth stack in Rust, the
Sunstone project will need foundational data structures for asynchronous
communication between isolated tasks.

# Summary

This document presents foundational structures that will enable loosely-coupled
task communication through different mediums. This document focuses on these
fundamentals and demonstrates their utility through examples appropriate for a
Bluetooth stack.

Additionally, this document outlines the non-functional characteristics of the
proposed design, referencing [RFC 0002](./0002_generic_collections.md) and
[RFC 0004](./0004_platform_agnostic_synchronization.md) for detailed resource
constraint analysis.

# Background

The Sunstone project is a comprehensive rewrite of the Bluetooth Sapphire stack
in Rust. As part of this rewrite, the asynchronous model will transition from a
C++ callback-oriented approach to a Rust async/await model.

This design builds heavily on
[RFC 0002 - Collections](./0002_generic_collections.md) and
[RFC 0004 - Synchronization](./0004_platform_agnostic_synchronization.md)

# Requirements

-   **Broad Usage:** The async foundation provided here should facilitate most
    use cases in our stack.
-   **Async-First:** The data structures should provide async APIs, instead of
    callbacks.
-   **Thread-Capable:** The data structures should allow inter-thread
    communication, allowing tasks to be spawned in separate threads

# Non-Functional Requirements

The Sunstone project has identified several key requirements:

-   **Works in Fuchsia:** The initial target is the Fuchsia OS.
-   **Embedded Friendliness:** The stack should be suitable for
    resource-constrained devices. This has some technical implications:
    -   **Heap Management:** The stack must handle memory exhaustion gracefully.
    -   **Code Footprint:** The stack must be cognizant of the final binary
        size.
    -   **Modularity:** The stack must be modular enough so that unneeded
        elements can be easily stripped away from the final binary.
-   **Testability:** Testing is of high priority, and the infrastructure should
    be well-suited for working in a test environment.

The following async infrastructure requirements stem directly from the above
goals:

-   **Platform Agnostic:** The async infrastructure should not be tied to a
    specific runtime.
-   **Allocation Avoidance:** The implementation should avoid allocations after
    initialization.
-   **Minimal Monomorphization:** The design should avoid a monomorphization
    explosion to reduce code size.
-   **Test Runtime:** An asynchronous test runtime is required for verifying
    correctness.

# Non-Requirements

-   **Exclusively Safe Code:** Minimizing `unsafe` is a priority, but
    restricting the design to 100% safe code would conflict with
    platform-abstraction and performance requirements.

# Design

## Meet the Team

The proposed `async` foundational structures are:

-   **MPSC:** A multi-producer, single-consumer queue is ubiquitous in Rust for
    both sync and async code. These data structures are excellent for
    dispatching work where a reply is not required. This can be expanded to an
    MPMC queue to set up concurrent servers.
-   **Broadcast:** Usually modeled as a pub-sub channel, these data structures
    enable broadly shared notifications across the entire system.
-   **RPC:** While not often described as such, an RPC channel provides a
    call/response sync-like API. These channels enable the actor model.

These three data structures are sufficient for most cases needed in a Bluetooth
stack.

> ***Note:*** One exception is priority handling. This is not specifically
> described in this document, but both RPC and MPSC channels can be extended to
> use a priority queue as the underlying backend.

### MPSC

MPSC queues are internally composed of a Deque or Queue data structure, and some
form of an `async` semaphore is used to notify senders and receivers of empty
and used slots (this pattern of wrapping a synchronous data structure with a
semaphore will come up often in this document).

Additionally, if the implementation must be thread-safe, a Mutex is required.
Platform-agnostic Mutexes are detailed in
[RFC 0002 - Synchronization](./0002_platform_agnostic_synchronization.md).

#### API

```rust
impl<T, Cfg: MpscCfg> Mpsc<T, Cfg> {
    /// Creates a new, empty `Mpsc` with the configured buffer type.
    pub fn new() -> Self;

    /// Creates a new, empty `Mpsc` backed by the provided buffer container.
    pub fn new_with(mut buffer: Deque<Cfg::Buffer, T>) -> Self;

    /// Splits the `Mpsc` channel into a sender (`Sender`) and a receiver (`Receiver`) pair.
    pub fn split(&mut self) -> (Sender<&'_ Self>, Receiver<&'_ Self>);

    #[cfg(feature = "std")]
    /// Splits the channel into atomically reference-counted sender and receiver ends
    pub fn split_to_arc(self) -> (Sender<std::sync::Arc<Self>>, Receiver<std::sync::Arc<Self>>);

    #[cfg(feature = "std")]
    /// Splits the channel into reference-counted sender and receiver ends
    pub fn split_to_rc(self) -> (Sender<std::rc::Rc<Self>>, Receiver<std::rc::Rc<Self>>);

}

/// Error sending a payload to the channel.
pub enum SendError {
    /// All receiving ends of the channel have been closed
    Closed,
}

/// Error sending a payload to the channel synchronously (without blocking).
pub enum SendSyncError {
    SendError(SendError),
    WouldBlock,
}


impl<T, Cfg, Chan> Sender<Chan>
where
    Chan: Deref<Target = Mpsc<T, Cfg>>,
    Cfg: MpscCfg,
{
    /// Asynchronously sends a message over the channel.
    ///
    /// Blocks if the channel's buffer is full until a receiver reads a message and frees a slot.
    pub async fn send(&mut self, payload: T) -> Result<(), (T, SendError)>;

    /// Attempts to immediately send a message over the channel without blocking.
    ///
    /// Returns `Err(payload)` if the channel buffer is currently full.
    pub fn try_send(&self, payload: T) -> Result<(), (T, SendSyncError)>;
}


/// Error receiving a payload from the channel.
pub enum RecvError {
    /// Channel is empty and no other sender handles exist.
    Closed,
}

impl<T, Cfg, Chan> Receiver<Chan>
where
    Chan: Deref<Target = Mpsc<T, Cfg>>,
    Cfg: MpscCfg,
{
    /// Asynchronously receives the next message from the channel.
    ///
    /// Blocks if the channel is empty until a sender publishes a message.
    pub async fn recv(&self) -> Result<T, RecvError>;

    /// Attempts to immediately receive the next message from the channel without blocking.
    ///
    /// Returns `None` if there are no pending messages enqueued.
    pub fn try_recv(&self) -> Result<Option<T>, RecvError>;
}
```

Furthermore, `Sender/Receiver` handles can be `Clone` if the underlying channel
reference type is also clone which enables `mpsc` or even `mpmc` semantics.

#### `pub trait [Foo]Cfg`

Throughout this design, `Cfg` traits are used to describe some of the internals
of the async data structures. This pattern stems from the requirement to be
platform-agnostic. The goal is not to deny memory allocations or multithreading
altogether. Rather, the design empowers users of these APIs to choose the right
semantics for their system. `Cfg` traits are used to describe system-specific
facets in a platform-agnostic way. They are collections of types that will be
used in the data structure.

```rust
pub trait MpscCfg {
    /// The storage container family used for the internal queue buffer.
    type Buffer: RawVecFamily;
    /// The raw mutex fundamental used to synchronize internal channel state.
    type Mtx: RawMutex;
}
```

The `MpscCfg` trait requires concrete implementations for:

1.  `RawVecFamily`: The underlying buffer storage (see
    [RFC 0001](./0001_generic_collections.md) for details on implementations
    like `StdRawVec` or `StackRawVec`).
2.  `RawMutex`: The lock used for synchronization (see
    [RFC 0002](./0002_platform_agnostic_synchronization.md) for details on
    implementations like `SpinRawMutex` or `SingleThreadedRawMutex`).

### Broadcast

Broadcast channels hold a `Deque` for tracking broadcasted messages and a
monotonically increasing, unique-ish global index associated with each message.

> ***Note:*** Unique-ish because they will eventually wrap around safely, but
> this can lead to message mismatch if a subscriber has not read for a very long
> time.

Each subscriber also tracks the global index describing their current position
in the global queue.

New messages are posted to the back of the deque, and the next global index is
incremented, notifying subscribers.

When events are read by subscribers, they update their internal cursor and
potentially clean up stale messages in the deque (which is bottlenecked by the
slowest subscriber).

Broadcasters can choose between waiting for a slot in the deque or evicting the
oldest message in the queue. If an eviction takes place, a subscriber will
receive an error when they call `.next()`, returning a `MissedMessages` error
containing the number of messages they lagged behind. When this happens, their
cursor is also updated to the oldest message in the queue, allowing them to
receive the next event when they call `.next()` again.

#### API

```rust
impl<T: Clone, Cfg: BroadcastCfg> BroadcastChannel<T, Cfg> {
    /// Creates a new, empty `BroadcastChannel` with the configured mutex and notification fundamentals.
    pub fn new() -> Self;

    /// Subscribes to the channel, returning a [`Subscriber`] endpoint if there is slot capacity.
    ///
    /// Returns `None` if the maximum number of subscribers (defined by `SubscriptionList` capacity)
    /// has been reached.
    pub fn subscribe(&self) -> Option<Subscriber<'_, T, Cfg>>;

    /// Publishes a message to the channel asynchronously.
    ///
    /// If the channel's buffer is at capacity, this method blocks until the slowest reader
    /// reads enough elements to reclaim space.
    pub async fn publish(&self, payload: T);

    /// Publishes a message to the channel, evicting the oldest message if at capacity.
    ///
    /// This method never blocks. If the channel is at capacity, the oldest message is evicted
    /// and slow readers will miss it, returning `Err(MissedMessages)` on their next poll.
    pub fn force_publish(&self, payload: T);
}



impl<'a, T: Clone, Cfg: BroadcastCfg> Subscriber<'a, T, Cfg> {
    /// Asynchronously polls and retrieves the next broadcasted message.
    pub async fn next(&self) -> Result<T, MissedMessages>;
}
```

#### Cfg

The Broadcast channel configuration requires the following types:

```rust
/// Configuration trait for configuring the types inside a [`BroadcastChannel`].
pub trait BroadcastCfg {
    /// The storage container family used for the internal message queue buffer.
    type Buffer: RawVecFamily;
    /// The storage container family used for the active subscriber list.
    type SubscriptionList: RawVecFamily;
    /// The raw mutex fundamental used to synchronize internal channel state.
    type Mtx: RawMutex;
}
```

The `Buffer` is used to hold events, while the `SubscriptionList` is used to
hold subscribers. `SubscriptionList` might be updated in the future to use a
linked list or map-based API instead to be more efficient for searching and
deletion. The other generics are semantically identical to those in `MpscCfg`.

### RPC

RPC channels are used for call-response semantics but are more optimized than
tuples of `mpsc` channels. An RPC channel is composed of an inbox (`Deque`) for
the server. Each item in the `Deque` is a combination of a request (caller's
payload) and a completion status.

The channel's state is also wrapped by a mutex to enable multi-threaded
operation.

An RPC request may be in the following states:

-   `Requested`: A client requested an RPC invocation.
-   `Accepted`: The server has begun working on this request. The request
    payload is taken.
-   `Completed`: The server has completed the request, sending its response to
    the client.
-   `Cancelled`: The client's `Future` has been dropped and the response slot
    may now be dangling.

RPC channels can be optimized around the synchronous rendezvous semantics of the
channel. Since a client will have to wait for the response, the client can give
the server a stack-allocated slot where the response may be written.

#### Cancellation

If a client request is cancelled (i.e., the `Future` is dropped), the client
will execute its `Drop` implementation that marks a `Requested` or `Accepted`
request as `Cancelled`. This tells the server that the response slot is not
valid. The server will check the request state before responding to validate
that the request has not been cancelled.

#### API

```rust
impl<R: Rpc, Cfg: RpcCfg> RpcChannel<R, Cfg> {

    /// Creates a new, empty `RpcChannel` with the configured wakers and mutex fundamentals.
    pub fn new() -> Self;

    /// Splits the `RpcChannel` into a [`Client`] and a [`Server`] pair.
    pub fn split(&mut self) -> (Client<R, Cfg, &'_ Self>, Server<R, Cfg, &'_ Self>);
}

impl<R: Rpc, Cfg: RpcCfg, C: Deref<Target = RpcChannel<R, Cfg>>> Client<C> {
    /// Submits an RPC request to the server asynchronously and blocks until the response is returned.
    ///
    /// # Cancel Safety
    ///
    /// Cancelling this Future may notify the server that the request may not need to be evaluated
    /// and will be discarded. However, if the request has been accepted, the server may continue
    /// its handler for the request but will avoid writing the response back to the client
    pub async fn call(&self, request: R::Request) -> R::Response;
}

impl<R: Rpc, Cfg: RpcCfg, C: Deref<Target = RpcChannel<R, Cfg>> + Clone> Server<C> {
    /// Asynchronously blocks until the next Client request is received.
    ///
    /// Returns a pair containing the request payload and a [`Responder`] endpoint.
    pub async fn recv(&self) -> (R::Request, Responder<'_, R, Cfg>);

    /// Attempts to immediately receive a Client request without blocking.
    ///
    /// Returns `None` if there are no pending commands currently enqueued.
    pub fn try_recv(&self) -> Option<(R::Request, Responder<'_, R, Cfg>)>;
}

impl<R: Rpc, Cfg: RpcCfg, C: Deref<Target = RpcChannel<R, Cfg>>> Responder<R, Cfg, C> {
    /// Sends the response back to the Client and wakes their waker.
    pub fn respond(self, response: R::Response);
}
```

An RPC channel needs both a configuration and an `Rpc` implementation that
describes the Request and Response types. This is a generalization over a
generic `T` to better name two generics `T` and `U` for request and response.

```rust
/// Configuration trait defining the types and synchronization fundamentals for an [`RpcChannel`].
pub trait Rpc {
    /// Request type enqueued by the Client.
    type Request;
    /// Response returned by the Server.
    type Response;
}
```

#### Cfg

```rust
pub trait RpcCfg {
    /// Raw mutex fundamental used to synchronize internal channel state.
    type Mtx: RawMutex;
    /// Storage container family used for the enqueued request queue.
    type Chan: RawVecFamily;
}
```

The RPC configuration is semantically identical to the MPSC channel
configuration.

### `pub trait Executor`

In order to stay agnostic to the platform, the design includes an `Executor`
trait which is likely to grow over time as more `async` intrinsics are needed.
However, for now, the `Executor` looks like this:

```rust
pub trait Executor {
    type JoinHandle<T>;
    /// Spawns a `Future` irrespective of its lifetime
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the spawned future won't outlive its underlying lifetime, for
    /// instance, by guaranteeing that the executor's lifetime itself is shorter than `'a`.
    ///
    /// Consider using `spawn` instead for a safe API that requires `'static`
    unsafe fn spawn_unchecked<'a, F, T>(&self, fut: F) -> Self::JoinHandle<T>
    where
        F: Future<Output = T> + 'a;

    /// Spawns a `Future`
    fn spawn<F, T>(&self, fut: F) -> Self::JoinHandle<T>
    where
        F: Future<Output = T> + 'static,
    {
        // SAFETY: 'static bound means that there are no lifetime bounds
        unsafe { self.spawn_unchecked(fut) }
    }
}
```

The only required function (for now) is `spawn_unchecked`, which allows spawning
a `Future` irrespective of its lifetime. This is very `unsafe`.

A `spawn` function that is safe so long as the `Future` is `'static` can be
trivially defined, but that can be very limiting, especially in tests.
Therefore, the design also provides a safe API for spawning non-static futures,
taking a page from `std::thread`'s book, specifically `std::thread::scope`.

#### Lifetime Scoping with HRTB

`std::thread::scope` uses
[higher-rank trait bounds](https://doc.rust-lang.org/nomicon/hrtb.html) to
constrain the lifetime of the executor.

To provide a safe, non-static spawn API, the design must guarantee the `Drop`
implementation of the underlying executor. That is, all spawned futures must
outlive the Executor. However, since Rust does not have a way to describe a type
which `Drop` implementation must be called; it is always possible to forget a
type and break our safety invariants.

`BoundedExecutor` solves this by using a scope-based API with Higher-Rank Trait
Bounds (HRTBs), similar to `std::thread::scope`.

The design includes a wrapper around an executor called a
`BoundedExecutor<'runtime, 'env, E: Executor>` that has an
[invariant lifetime](https://doc.rust-lang.org/nomicon/subtyping.html) over
`'runtime`.

It provides the following API:

```rust
impl<E: Executor> BoundedExecutor<'_, '_, E> {
    pub fn new<'env, F>(executor: E, fun: F)
    where
        for<'runtime> F: FnOnce(&'runtime BoundedExecutor<'runtime, 'env, E>),
    {
        let bounded = BoundedExecutor { ... };
        fun(&bounded);
    }
}

impl<'runtime, 'env, E: Executor> BoundedExecutor<'runtime, 'env, E> {
    pub fn spawn<'a, F, T>(&self, fut: F) -> E::JoinHandle<T>
    where
        'a: 'runtime,
        F: Future<Output = T> + 'a;

    pub fn spawner<'a: 'runtime>(&'a self) -> Spawner<'a, E>;
    pub fn inner(&self) -> &E;
}

impl<'runtime, E: Executor> Spawner<'runtime, E> {
    pub fn spawn<'a, F>(&self, fut: F)
    where
        'a: 'runtime,
        F: Future<Output = ()> + 'a;
}
```

The only way to construct this executor is with `BoundedExecutor::new`, which
**does not** return **Self**. Instead, it accepts the underlying `Executor` as
its first parameter and a closure that will be executed as its second parameter.

The closure takes a reference to the `BoundedExecutor`. The key design points
are:

1.  **No Ownership Transfer:** Ownership of the `BoundedExecutor` is retained by
    `new` and never exposed (only a borrow is passed to the closure). This
    prevents callers from leaking the executor via `std::mem::forget`,
    guaranteeing its `Drop` implementation will run and block until all tasks
    complete.
2.  **Invariance:** `BoundedExecutor` is invariant over `'runtime`, preventing
    the compiler from unsafely shrinking this lifetime.
3.  **HRTB Constraints:** The `'runtime` lifetime is constrained by an HRTB,
    forcing the closure to treat it as arbitrary.

To make the HRTB useful (since arbitrary lifetimes normally default to
`'static`), a second `'env` lifetime is introduced. The compiler knows `'env`
outlives the arbitrary `'runtime`, allowing it to deduce that any data outliving
the `BoundedExecutor::new` call is safe to reference within the spawned futures.
This enables safe use of stack-allocated references in non-`'static` futures.

## Async Notifications

In order to make it easier to build `async` collections, a `Notification` can be
used as the underlying mechanism for notifying pending tasks that progress can
be made.

This API should be low-level enough that most `async` collections can use it,
but high-level enough such that these collections do not have to juggle
intrusive collections of wakers internally.

A `Notification` provides such an API. It has two parts to its API: the `wait`
methods and the `notify` methods.

### API

```rust
impl<Mtx: RawMutex> Notification<Mtx> {
    /// Creates a new, unnotified `Notification` fundamental.
    pub fn new() -> Self;

    /// Asynchronously blocks the current task until notified.
    pub fn wait(&self) -> WaitFuture<'_, Mtx>;

    /// Asynchronously blocks the current task while releasing the provided `guard`.
    ///
    /// Atomically releases the lock and registers the current task to block. Upon waking,
    /// re-acquires the lock and returns a new `MutexGuard`.
    pub fn wait_locking<'a, ChannelMtx: RawMutex, T>(
        &self,
        guard: MutexGuard<'a, ChannelMtx, T>,
    ) -> WaitLockingFuture<'_, 'a, ChannelMtx, T, Mtx>;

    /// Asynchronously blocks until the provided predicate closure `fun` evaluates to `Poll::Ready(R)`.
    ///
    /// Performs predicate checking in a loop: if `fun` returns `Poll::Pending`, it atomically
    /// releases the lock and blocks via `wait_locking`. Wakes up on notification to re-evaluate.
    pub async fn when<'a, F, ChannelMtx: RawMutex, T, R>(
        &self,
        mut lock: MutexGuard<'a, ChannelMtx, T>,
        mut fun: F,
    ) -> R
    where
        F: FnMut(&mut T) -> Poll<R>;

    /// Wakes up exactly one blocked task waiting on this notification.
    pub fn notify_one(&self);

    /// Wakes up `min(count, self.waiters())` blocked tasks waiting on this notification.
    ///
    /// Does nothing if `count == 0`
    pub fn notify_many(&self, count: usize);

    /// Wakes up all blocked tasks waiting on this notification.
    pub fn notify_all(&self);

    /// Returns the number of active tasks currently blocked and waiting on this notification.
    pub fn waiters(&self) -> usize;
}
```

A `Notification` is built with an intrusive list of wakers. When a future wants
to block (i.e., `wait`) on this `Notification` object, it can do so by creating
a waker slot and adding it to the intrusive collection. This waker will remain
tied to the list of wakers until either:

1.  The notification wakes the future.
1.  The future is dropped.

Because of the semantics around `Pin`, the memory in the future will not be
reclaimed if the future is forgotten, which makes this API sound.

The `wait` API is quite bare and is often not what is actually needed. In most
cases, one wants to await a notification and atomically release a `Mutex`. That
is what `wait_locking` does. It is very similar in essence to a
[`Condvar`](https://doc.rust-lang.org/std/sync/struct.Condvar.html). This API
bridges the synchronous and asynchronous worlds.

A higher-level API for blocking is the `.when()` call. It takes a lock guard and
a closure which returns `Poll`. `.when()` will run the closure in a loop until
it returns `Poll::Ready`. After every iteration, it will call `.wait_locking()`.
The closure itself takes a `&mut T`, which is the underlying payload in the
`Mutex`. Most users of a `Notification` object will block using `.when()`.

On the other end of a `Notification` is the notification API.

It supports notifying any number of wakers in the list in FIFO order.

## Other Async Types

### `pub struct Condition<Mtx, T>`

Without going into much detail here, `Condition<Mtx, T>` can be built on top of
a `Notification` easily. It essentially wraps together the `Mutex` and payload
with the `Notification`. This is useful in many cases, but in reality,
`Notification` itself can be more useful since multiple `Notification`s can
exist over the same data. `Mpsc`, `Rpc`, and `Broadcast` all use a `Mutex` and
two `Notification`s.

### `pub struct Semaphore<Mtx>`

An async `Semaphore` is just a wrapper over a `Condition<Mtx, usize>` with an
`async fn down(&self)` and a `fn up(&self)`.

## Testing Infrastructure

The only testing infrastructure provided is a `TestExecutor` that implements
`Executor`. It sets up a list of tasks and wakers and implements `Executor`, but
also provides the following API:

```rust
impl TestExecutor {
    pub const fn new() -> Self;
    pub fn run_until_stalled(&self);
    pub fn block_on<'a, F>(&self, mut future: F) -> F::Output
    where
        F: Future + 'a;
}
```

This API, combined with the `Executor` implementation, allows spawning tasks and
running the executor until no more progress can be made. Assertions can be
placed between the stalling points to verify specific conditions. `block_on` may
be used to force a future to run sequentially.

In general, `TestExecutor` should be used with `BoundedExecutor` to provide a
safe `spawn` API that can use stack-allocated variables internally.

# Resource Constraints & Requirements

This design builds upon [RFC 0001](./0001_generic_collections.md), which details
design trade-offs and allocation schemes. However, building async infrastructure
on top of these generic collections introduces an additional layer of
monomorphization. We must carefully manage this to prevent excessive code size
growth.

# Alternatives

Using `embassy_sync` for the channels' implementations was originally
considered. However, this option was rejected because it could not properly
cooperate with Pigweed's allocation decisions. As far as the authors are aware,
no other library exists that makes the implementation agnostic to the underlying
backing storage.

# Drawbacks

*   **Generic Verbosity:** This design requires specifying numerous generic
    parameters throughout the codebase, which can compound with system
    complexity. To mitigate this, a unified `System` trait can encapsulate
    platform-specific configurations, allowing developers to propagate a single
    generic parameter instead of multiple individual `Cfg` types.
*   **Lifetime Complexity:** The extensive use of lifetimes increases the
    likelihood of lifetime compilation errors ("does not live long enough").
    While some may be false positives resolvable via code adjustments, true
    positives will require transitioning to static-lifetime shared pointers
    (e.g., `Arc` or `Rc`).

# Unknowns

It is still somewhat unclear how easy or difficult it will be to use this API,
specifically whether the large number of `Cfg`s and generics will be a
bottleneck in development velocity.

# Implementation Plan

## Files to Add

### `sapphire-async`

-   sapphire-async/src/broadcast.rs
-   sapphire-async/src/condition.rs
-   sapphire-async/src/executor.rs
-   sapphire-async/src/global_idx.rs
-   sapphire-async/src/lib.rs
-   sapphire-async/src/mpsc.rs
-   sapphire-async/src/notification.rs
-   sapphire-async/src/rpc.rs
-   sapphire-async/src/semaphore.rs
-   sapphire-async/src/testing.rs
-   sapphire-async/src/testing/executor.rs
-   sapphire-async/src/testing/executor/waker.rs

# Documentation & Examples

All APIs will be extensively documented, including `doctests` for clear usage
examples.

# Testing

There are three layers of testing in this design.

## Unit Tests

All code will be unit tested using Rust's native testing framework. Any
`async`-related code will use the `TestExecutor` for testing the `async`
interactions.

## Documentation Tests

All code will be heavily documented and will include `doctests` that behave just
like the unit tests.

## Property-Based Tests

`proptest` will be used to set up property-based tests where appropriate. All of
the collections (sync and async) will be property-tested.

## Miri

Additionally, every test will be run with Miri to verify soundness of the
implementation, including the proptests (though the number of generated examples
will be reduced since Miri tests are significantly slower).

# Future Work

*   **Priority-Based Execution:** Consider introducing priority-based channels
    and task scheduling to optimize latency-sensitive workflows.
*   **Executor Additions:** We will want to add executor-native features to the
    `Executor` (e.g. `Timer`).
