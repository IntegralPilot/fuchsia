// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::mem::ManuallyDrop;
use std::ops::Deref;
use std::sync::Arc;
use std::task::{RawWaker, RawWakerVTable, Waker};
use zx_types;

struct HandleReadWaker {
    handle: zx_types::zx_handle_t,
    notification_sender: Option<async_channel::Sender<zx_types::zx_handle_t>>,
}

impl HandleReadWaker {
    unsafe fn waker_clone(this: *const ()) -> RawWaker {
        let this = ManuallyDrop::new(unsafe { Arc::from_raw(this as *const Self) });
        let res = RawWaker::new(Arc::into_raw(this.deref().clone()) as *const (), &WAKER_VTABLE);
        res
    }

    unsafe fn waker_wake(this: *const ()) {
        unsafe { Self::waker_wake_by_ref(this) };
        unsafe { Self::waker_drop(this) };
    }

    unsafe fn waker_wake_by_ref(this: *const ()) {
        let this = ManuallyDrop::new(unsafe { Arc::from_raw(this as *const Self) });
        if let Some(ref sender) = this.deref().notification_sender {
            let _ = sender
                .try_send(this.deref().handle)
                .map_err(|e| log::debug!("failed sending notification {e:?}"));
        }
    }

    unsafe fn waker_drop(this: *const ()) {
        let _this = unsafe { Arc::from_raw(this as *const Self) };
    }
}

const WAKER_VTABLE: RawWakerVTable = RawWakerVTable::new(
    HandleReadWaker::waker_clone,
    HandleReadWaker::waker_wake,
    HandleReadWaker::waker_wake_by_ref,
    HandleReadWaker::waker_drop,
);

pub(crate) fn handle_notifier_waker(
    handle: zx_types::zx_handle_t,
    notification_sender: Option<async_channel::Sender<zx_types::zx_handle_t>>,
) -> Waker {
    let data = Arc::into_raw(Arc::new(HandleReadWaker { handle, notification_sender }));
    unsafe { Waker::from_raw(RawWaker::new(data as *const (), &WAKER_VTABLE)) }
}
