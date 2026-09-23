// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::task::{RawWaker, RawWakerVTable, Waker};
use std::rc::Weak;

use crate::testing::executor::TaskHeader;

// The raw waker vtable managing task wakeups.
const VTABLE: RawWakerVTable =
    RawWakerVTable::new(clone_waker, wake_task, wake_by_ref_task, drop_waker);

unsafe fn clone_waker(ptr: *const ()) -> RawWaker {
    let weak = unsafe { Weak::from_raw(ptr as *const TaskHeader) };
    let cloned = weak.clone();
    let _ = weak.into_raw(); // Put back the original to avoid dropping it
    RawWaker::new(cloned.into_raw() as *const (), &VTABLE)
}

// SAFETY: Wakes the task by pushing its index back into the scope's run queue.
// This consumes the waker.
unsafe fn wake_task(ptr: *const ()) {
    let weak = unsafe { Weak::from_raw(ptr as *const TaskHeader) };
    if let Some(header) = weak.upgrade() {
        let id = header.id;
        let mut queue = header.ready_queue.lock();
        if !queue.contains(&id) {
            queue.push_back(id);
        }
    }
    // `weak` is dropped here, consuming the ref count.
}

// SAFETY: Wakes the task by pushing its index back into the scope's run queue.
// This does not consume the waker.
unsafe fn wake_by_ref_task(ptr: *const ()) {
    let weak = unsafe { Weak::from_raw(ptr as *const TaskHeader) };
    if let Some(header) = weak.upgrade() {
        let id = header.id;
        let mut queue = header.ready_queue.lock();
        if !queue.contains(&id) {
            queue.push_back(id);
        }
    }
    let _ = weak.into_raw(); // Put back the original to avoid dropping it
}

unsafe fn drop_waker(ptr: *const ()) {
    unsafe { Weak::from_raw(ptr as *const TaskHeader) };
}

pub fn make_waker(task: Weak<TaskHeader>) -> Waker {
    let raw_waker = RawWaker::new(task.into_raw() as *const _ as *const (), &VTABLE);
    unsafe { Waker::from_raw(raw_waker) }
}
