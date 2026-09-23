// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::borrow::Borrow;
use std::panic::Location;

use fidl::endpoints::{ProtocolMarker, RequestStream};
use fuchsia_async::JoinHandle;
use fuchsia_async::scope::{ScopeActiveGuard, ScopeHandle};
use futures::{Future, FutureExt as _};
use log::debug;

use crate::bindings::util::ErrorLogExt;

#[derive(Debug)]
pub(crate) struct ScopeFinishedError;

pub(crate) trait ScopeExt: Borrow<ScopeHandle> {
    #[track_caller]
    fn spawn_guarded_assert_cancelled(
        &self,
        guard: ScopeActiveGuard,
        fut: impl Future<Output = ()> + Send + 'static,
    ) -> JoinHandle<()> {
        // Get location info now because we're not going to have it in the
        // scope.
        let location = Location::caller();
        self.borrow().spawn(fut.then(move |()| {
            assert!(
                guard.as_handle().is_cancelled(),
                "task from {location} exited before scope was cancelled"
            );
            futures::future::ready(())
        }))
    }

    #[track_caller]
    fn spawn_new_guard_assert_cancelled(
        &self,
        fut: impl Future<Output = ()> + Send + 'static,
    ) -> Result<JoinHandle<()>, ScopeFinishedError> {
        let guard = self.borrow().active_guard().ok_or(ScopeFinishedError)?;
        Ok(self.spawn_guarded_assert_cancelled(guard, fut))
    }

    fn spawn_fidl_task<M, E, Fut>(&self, fut: Fut)
    where
        Fut: Future<Output = Result<(), E>> + Send + 'static,
        E: ErrorLogExt,
        M: ProtocolMarker,
    {
        debug!("serving: {}", M::DEBUG_NAME);
        let _: JoinHandle<()> = self.borrow().spawn(fut.map(|res| match res {
            Ok(()) => (),
            Err(err) => err.log(format_args!("{} error", M::DEBUG_NAME)),
        }));
    }

    fn spawn_request_stream_handler<S, E, Fut, F>(&self, rs: S, f: F)
    where
        F: FnOnce(S) -> Fut,
        Fut: Future<Output = Result<(), E>> + Send + 'static,
        E: ErrorLogExt,
        S: RequestStream,
    {
        self.spawn_fidl_task::<S::Protocol, _, _>(f(rs))
    }

    fn spawn_server_end<M, E, Fut, F>(&self, server_end: fidl::endpoints::ServerEnd<M>, f: F)
    where
        F: FnOnce(fidl::endpoints::ServerEnd<M>) -> Fut,
        Fut: Future<Output = Result<(), E>> + Send + 'static,
        E: ErrorLogExt,
        M: ProtocolMarker,
    {
        self.spawn_fidl_task::<M, _, _>(f(server_end))
    }

    fn new_detached_child(&self, name: impl Into<String>) -> ScopeHandle {
        let child = self.borrow().new_child_with_name(name.into());
        let handle = child.to_handle();
        child.detach();
        handle
    }
}

impl<O> ScopeExt for O where O: Borrow<ScopeHandle> {}
