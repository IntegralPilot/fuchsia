// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::cell::OnceCell;

use zx_status::Status;

use crate::{AsAsyncDispatcherRef, AsyncDispatcher, CurrentDispatcher, GetAsyncDispatcher};

/// Implements a dispatcher that is either set at construction or detected and cached on first use.
///
/// This is intended to be used inside futures to implement detecting the current dispatcher on
/// first poll if the instigator of the future did not specify a dispatcher to run on.
#[derive(Default, Debug)]
pub struct DetectDispatcher {
    dispatcher: OnceCell<Option<AsyncDispatcher>>,
}

impl DetectDispatcher {
    /// Use this to pre-set the dispatcher so that future calls to [`GetAsyncDispatcher`] will
    /// return is dispatcher instead of trying to get the default one.
    pub fn new(with_dispatcher: impl AsAsyncDispatcherRef) -> Self {
        let dispatcher = OnceCell::new();
        // unwrap because this cannot fail on a freshly constructed OnceCell
        dispatcher.set(Some(AsyncDispatcher::new(&with_dispatcher))).unwrap();
        Self { dispatcher }
    }

    /// Gets the dispatcher if set in the constructor, or attempts to retrieve the current
    /// dispatcher as set by [`crate::CurrentDispatcher::set`] (or the underlying async-default
    /// api).
    ///
    /// Use this in a [`std::task::Poll`] implementation to defer finding the current dispatcher until the
    /// future is being run on the dispatcher.
    ///
    /// Once a dispatcher has been set or detected, that dispatcher will be held and returned for
    /// all future calls.
    ///
    /// Returns [`Status::BAD_STATE`] if there is no dispatcher set or detected.
    pub fn get_or_detect(&self) -> Result<&AsyncDispatcher, Status> {
        self.dispatcher
            .get_or_init(|| CurrentDispatcher.try_get_async_dispatcher())
            .as_ref()
            .ok_or(Status::BAD_STATE)
    }

    /// Gets the dispatcher if it had been previously set or detected. If it hasn't, this will
    /// return None.
    pub fn get(&self) -> Option<&AsyncDispatcher> {
        self.dispatcher.get()?.as_ref()
    }
}
