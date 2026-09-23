// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::lib_context::Notifier;
use fdomain_client::{
    Channel, Error as FDomainInternalError, Handle, MessageBuf, OnFDomainSignals, Socket,
};
use fuchsia_async::Task;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicU32, Ordering};
use std::task::{Context, Poll, Waker};
use thiserror::Error;

pub(crate) type HandleQueue = VecDeque<u32>;
pub(crate) type HandleMap = HashMap<u32, Handle>;
pub(crate) type SignalMap = HashMap<u32, Task<Result<fidl::Signals, FDomainInternalError>>>;

#[derive(Debug, Error)]
pub(crate) enum FDomainError {
    #[error("no such FDomain handle registered under ID: {0}.")]
    NoSuchHandle(u32),
    #[error(
        "no associated context for FDomain handle under ID {0}. This means the associated `Context` object with which the handle was created was garbage collected."
    )]
    NoFDomainClient(u32),
}

pub(crate) struct FDomainState {
    handle_map: HandleMap,
    free_handles: HandleQueue,
    next_handle: AtomicU32,
    signal_waiters: SignalMap,
    handle_notifier: Notifier,
}

impl FDomainState {
    pub fn new(handle_notifier: Notifier) -> Self {
        Self {
            handle_map: Default::default(),
            free_handles: Default::default(),
            signal_waiters: Default::default(),
            next_handle: AtomicU32::new(1),
            handle_notifier,
        }
    }

    pub(crate) fn register(&mut self, handle: fdomain_client::Handle) -> u32 {
        // Handles are currently being allocated in this manner because there are a number of tests
        // that compare handle ID's with each other when sending them across channel boundaries. It
        // may not be necessary to allocate handle ID's this way, but this was adapted from the
        // fuchsia-async emulated handle code to preserve the pre-existing API. A simpler approach
        // would probably be to use a monotonically increasing number rather than a pool of
        // available handle ID's.
        let mapping = self
            .free_handles
            .pop_front()
            .unwrap_or_else(|| self.next_handle.fetch_add(1, Ordering::Relaxed));
        self.handle_map.insert(mapping, handle);
        mapping
    }

    /// Takes an ID and, if there's an available handle, will look it up in the handle
    /// table. If it doesn't exist an error will return. If the handle does not have an existing
    /// FDomain client for some reason, then an error will be returned.
    pub(crate) fn handle(&mut self, id: u32) -> Result<&fdomain_client::Handle, FDomainError> {
        match self.handle_map.entry(id) {
            Entry::Occupied(h) => {
                if !h.get().has_client() {
                    h.remove();
                    self.free_handles.push_back(id);
                    return Err(FDomainError::NoFDomainClient(id));
                }
                Ok(self.handle_map.get(&id).unwrap())
            }
            _ => Err(FDomainError::NoSuchHandle(id)),
        }
    }

    pub(crate) fn take_handle(&mut self, id: u32) -> Result<fdomain_client::Handle, FDomainError> {
        let res = self.handle_map.remove(&id).ok_or(FDomainError::NoSuchHandle(id))?;
        self.free_handles.push_back(id);
        if !res.has_client() {
            return Err(FDomainError::NoFDomainClient(id));
        }
        Ok(res)
    }

    async fn make_handle_notifier_waker(&self, handle_id: u32) -> Waker {
        crate::waker::handle_notifier_waker(
            handle_id,
            self.handle_notifier.lock().await.as_ref().map(|n| n.sender()),
        )
    }

    /// Polls a signal on a handle. If the handle does not exist, returns an error, else, returns
    /// `Ok(Poll<fidl::Signals>)`. This is here because signalling requires a bit of extra
    /// bookkeeping in order to prevent a hang.
    pub(crate) async fn poll_signal(
        &mut self,
        id: u32,
        signals: fidl::Signals,
    ) -> Result<Poll<Result<fidl::Signals, FDomainInternalError>>, FDomainError> {
        let waker = self.make_handle_notifier_waker(id).await;
        let ctx = &mut Context::from_waker(&waker);
        // This does a mutable borrow. We only care about the error message being returned (and the
        // invalid handle being thrown out of the map). To prevent multiple overalapping borrow
        // scopes, we just do an "unchecked" borrow using an unwrap afterwards so that the borrow
        // is immutable.
        let _ = self.handle(id)?;
        let handle = self.handle_map.get(&id).unwrap();
        match self.signal_waiters.entry(id) {
            Entry::Occupied(mut task) => {
                let res = Pin::new(task.get_mut()).poll(ctx);
                if res.is_ready() {
                    task.remove();
                }
                Ok(res)
            }
            Entry::Vacant(new_spot) => {
                let signaller = OnFDomainSignals::new(handle, signals);
                let mut task = Task::local(async { signaller.await });
                match Pin::new(&mut task).poll(ctx) {
                    res @ Poll::Ready(_) => Ok(res),
                    Poll::Pending => {
                        new_spot.insert(task);
                        Ok(Poll::Pending)
                    }
                }
            }
        }
    }

    pub(crate) async fn channel_read(
        &mut self,
        id: u32,
        buf: &mut MessageBuf,
    ) -> Result<Poll<Result<(), FDomainInternalError>>, FDomainError> {
        let waker = self.make_handle_notifier_waker(id).await;
        let mut ctx = Context::from_waker(&waker);
        let handle = self.handle(id)?;
        let channel = handle.as_unowned::<Channel>();
        Ok(channel.recv_from(&mut ctx, buf))
    }

    pub(crate) async fn socket_read(
        &mut self,
        id: u32,
        buf: &mut [u8],
    ) -> Result<Poll<Result<usize, FDomainInternalError>>, FDomainError> {
        let waker = self.make_handle_notifier_waker(id).await;
        let mut ctx = Context::from_waker(&waker);
        let handle = self.handle(id)?;
        let socket = handle.as_unowned::<Socket>();
        Ok(socket.poll_socket(&mut ctx, buf))
    }
}
