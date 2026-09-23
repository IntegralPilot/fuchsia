// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::marker::PhantomData;
use core::ops::Deref;
use core::pin::Pin;

struct InvariantLifetime<'a>(PhantomData<fn(&'a ()) -> &'a ()>);
struct CovariantLifetime<'a>(PhantomData<fn() -> &'a ()>);

impl InvariantLifetime<'_> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}
impl CovariantLifetime<'_> {
    pub const fn new() -> Self {
        Self(PhantomData)
    }
}

pub struct BoundedExecutor<'runtime, 'env: 'runtime, E> {
    exec: Pin<&'runtime E>,
    _runtime: InvariantLifetime<'runtime>,
    _env: CovariantLifetime<'env>,
}

pub struct Spawner<'runtime, E: Executor> {
    exec: Pin<&'runtime E>,
    _invariant: InvariantLifetime<'runtime>,
}

impl<'runtime, E: Executor> Spawner<'runtime, E> {
    pub fn spawn<'a, F, T>(&self, fut: F) -> E::JoinHandle<T>
    where
        'a: 'runtime,
        F: Future<Output = T> + 'a,
        T: 'a,
    {
        // SAFETY: 'a outlives the runtime
        unsafe { self.exec.spawn_unchecked(fut) }
    }
}

impl<E: Executor> BoundedExecutor<'_, '_, E> {
    pub fn new<'env, F>(executor: E, fun: F)
    where
        for<'runtime> F: FnOnce(&'runtime BoundedExecutor<'runtime, 'env, E>),
    {
        let bounded = BoundedExecutor {
            // SAFETY: executor won't move until its destruction at the end of the scope
            exec: unsafe { Pin::new_unchecked(&executor) },
            _env: CovariantLifetime::new(),
            _runtime: InvariantLifetime::new(),
        };
        fun(&bounded);
    }
}

impl<'runtime, 'env, E: Executor> BoundedExecutor<'runtime, 'env, E> {
    pub fn spawn<'a, F, T>(&self, fut: F) -> E::JoinHandle<T>
    where
        'a: 'runtime,
        F: Future<Output = T> + 'a,
        T: 'a,
    {
        // SAFETY: 'a outlives 'runtime.
        unsafe { self.exec.spawn_unchecked(fut) }
    }

    pub fn spawner<'a: 'runtime>(&'a self) -> Spawner<'a, E> {
        Spawner { exec: self.exec, _invariant: InvariantLifetime::new() }
    }

    pub fn inner(&self) -> &E {
        &self.exec
    }
}

impl<E> Deref for BoundedExecutor<'_, '_, E> {
    type Target = E;

    fn deref(&self) -> &Self::Target {
        &self.exec
    }
}

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
    unsafe fn spawn_unchecked<'a, F, T>(self: Pin<&Self>, fut: F) -> Self::JoinHandle<T>
    where
        F: Future<Output = T> + 'a,
        T: 'a;

    /// Spawns a `Future`
    fn spawn<F, T>(self: Pin<&Self>, fut: F) -> Self::JoinHandle<T>
    where
        F: Future<Output = T> + 'static,
        T: 'static,
    {
        // SAFETY: 'static bound means that there a no lifetime bounds
        unsafe { self.spawn_unchecked(fut) }
    }
}
