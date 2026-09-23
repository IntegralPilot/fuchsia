---
title: Platform agnostic thread synchronization
description: >
    Defines the mechanism for enabling thread synchronization in Sunstone
    while maintaining the goal of being platform independent
status: approved # [approved | declined | implemented | superseded]
authors:
  - asnaider@google.com
tags:
  - sync
  - mutex
bugs:
  - 520452028
rfc: 0004
---

# Objective

To support a reliable, embedded-friendly Bluetooth stack in Rust, the Sunstone
project requires platform-agnostic thread synchronization primitives.

# Summary

This document proposes a design for thread synchronization in Sunstone that
allows target platforms to provide their own synchronization primitives.

# Background

The Sunstone project is a Rust rewrite of the Bluetooth Sapphire stack. Rust
simplifies adding multithreading to asynchronous systems safely. Because
Sunstone must support both Fuchsia and Pigweed, it requires synchronization
primitives that function on both full operating systems and bare-metal
microcontrollers.

# Requirements

-   **Primitives:** Provide foundational synchronization mechanisms, starting
    with `Mutex` and `RwLock`.
-   **Zero-Cost (Single-Threaded):** Single-threaded applications must not incur
    the performance overhead of multi-threaded mutexes.
-   **Platform-Agnostic:** Support diverse backings, such as `std::sync::Mutex`
    on Fuchsia, and `critical_section`, spinlocks, or Pigweed-specific mutexes
    on Pigweed.

# Non-Requirements

-   **Exclusively Safe Code:** Minimizing `unsafe` is a priority, but
    restricting the design to 100% safe code would conflict with
    platform-abstraction and performance requirements.

# Design

Inspired by `embassy_sync`, the proposed design parameterizes synchronization
APIs over a `RawMutex` trait. This generic abstraction enables platform
portability and allows single-threaded contexts to use a zero-overhead
`SingleThreadMutex` (similar to `RefCell`) when thread-safety is unnecessary.

## `pub trait RawMutex`

The `RawMutex` trait defines the core interface for mutual exclusion.
Implementations must provide `lock` and `unlock` methods that define a critical
section. The `lock` method blocks if the mutex is held, waiting until it is
released.

This minimal interface is easy to implement. We provide a higher-level `Mutex`
wrapper that uses this trait, decoupling Sunstone data structures from the
concrete locking implementation.

```rust
/// A raw mutex trait with locking semantics.
///
/// # Safety
///
/// Implementors must guarantee mutual exclusion and memory visibility appropriate for their execution context.
pub unsafe trait RawMutex {
    /// The RawMutex's initial state: unlocked
    const UNLOCKED: Self;

    /// Acquires the lock, blocking or panicking if already locked.
    fn lock(&self);
    /// Releases the lock.
    ///
    /// # Safety
    /// The caller must guarantee that the mutex is currently locked by the current execution context.
    unsafe fn unlock(&self);
}
```

The `RawMutex` trait is marked `unsafe` because callers rely on its mutual
exclusion guarantee to safely obtain exclusive access to data via shared
references.

### `pub struct Mutex<Raw: RawMutex, T>`

The `Mutex` struct wraps a `RawMutex` and the protected payload. Its API matches
`std::sync::Mutex` but is generic over the `RawMutex` implementation.

```rust
pub struct Mutex<Kind, T> {
    raw_mutex: Kind,
    payload: UnsafeCell<T>,
}

impl<Kind: RawMutex, T> Mutex<Kind, T> {
    /// Creates a new mutex protecting the given data.
    pub const fn new(data: T) -> Self;
    /// Consumes the mutex, returning the underlying protected data.
    pub const fn into_inner(self) -> T;
    /// Returns a mutable reference to the protected data without locking.
    pub const fn get_mut(&mut self) -> &mut T;
    /// Acquires the mutex, blocking until the lock becomes available.
    pub fn lock(&self) -> MutexGuard<'_, Kind, T>;
}

impl<'a, Kind: RawMutex, T> MutexGuard<'a, Kind, T> {
    /// Acquires the lock and constructs a new guard.
    pub fn new(mutex: &'a Mutex<Kind, T>) -> Self;
    /// Returns a reference to the underlying mutex.
    pub fn mutex(&self) -> &'a Mutex<Kind, T>;
}
```

`MutexGuard` implements `Deref<Target = T>` and `DerefMut<Target = T>`. Its
`Drop` implementation automatically calls `unlock` on the underlying `RawMutex`.

### Thread Safety (`Send` and `Sync`)

The bounds for `Send` and `Sync` differ slightly from the standard library:

```rust
// SAFETY: Mutex is Send if the payload and the raw mutex are Send.
unsafe impl<Kind: RawMutex + Send, T: Send> Send for Mutex<Kind, T> {}
// SAFETY: Mutex is Sync if the payload is Send and the raw mutex is Sync.
unsafe impl<Kind: RawMutex + Sync, T: Send> Sync for Mutex<Kind, T> {}
```

-   **`Send`:** Since `Mutex` is an owning container, it is `Send` if both `T`
    and `Kind` are `Send`.
-   **`Sync`:** Like `std::sync::Mutex`, we require `T: Send` because sharing
    the `Mutex` allows threads to acquire exclusive references and potentially
    move the data out (e.g., using `core::mem::take`). We additionally require
    `Kind: Sync`. This enables single-threaded mutex implementations (where
    `Kind` is not `Sync`) to behave like a `RefCell` without incurring
    multi-threaded locking overhead.

## RwLock

An `RwLock` can be implemented in a similar way to `Mutex` by providing a
`RawRwLock` trait that provides `lock_shared` and `lock_exclusive` functions as
well as the corresponding `unlock` methods.

> TODO(559037324): Complete this section when RwLock is implemented

# Resource Constraints & Requirements

## Code Size (Monomorphization)

Code size increases if the compiler generates duplicate implementations
(monomorphization) for the same payload type parameterized with different
`RawMutex` types. To minimize this overhead, platforms should standardize on a
single system-wide mutex type where possible.

# Alternatives

## `lock_api` crate

While the `lock_api` crate seemingly satisfies the requirements for this
implementation, it's unlikely that Pigweed will adopt it at this time. Therefore
the design proposed here reimplements these traits and structs until we can
migrate to Pigweed's implementation.

# Implementation Plan

## Files to Add

We will introduce a new crate, `sapphire-sync`, with the following files:

-   `sapphire-sync/src/lib.rs`
-   `sapphire-sync/src/mutex.rs`
-   `sapphire-sync/src/rw_lock.rs`
-   `sapphire-sync/src/mutex/raw.rs`
-   `sapphire-sync/src/mutex/raw/single_thread.rs`
-   `sapphire-sync/src/mutex/raw/spin.rs`

# Documentation & Examples

All APIs will be documented with `doctests` to provide verified usage examples.

# Testing

We will verify the implementation using three testing layers:

*   **Unit Tests:** Standard tests using the Rust unit-testing framework.
*   **Documentation Tests:** Doctests to ensure code examples remain correct.
*   **Miri:** Running all tests under Miri to validate memory safety and
    synchronization soundness.

# Future Work

*   **Fallible Locking:** Consider adding `try_lock` variants to `RawMutex` to
    support non-blocking locking semantics (i.e., acquiring the lock if
    available, or returning an error immediately).
