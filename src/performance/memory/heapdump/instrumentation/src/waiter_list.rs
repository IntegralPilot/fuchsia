// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::cell::Cell;
use std::sync::{Condvar, MutexGuard};
use std::time::Duration;

/// A collection of waiting threads, each identified by the address being waited for.
pub struct WaiterList {
    /// The first node in the linked list (null if the list is empty).
    head: *const Node,
}

// SAFETY: Nodes' `next` pointers are managed by the WaiterList methods and nodes' contents are safe
// to access from any thread.
unsafe impl Send for WaiterList {}

struct Node {
    // The address being waited for.
    waited_address: u64,

    // A condition variable to wake up the waiting thread.
    condvar: Condvar,

    /// Nodes are stored in an intrusive singly-linked list.
    ///
    /// - None means that this node is not currently part of a list.
    /// - Some(ptr) means that this node is part of a list and its successor is ptr (which might be
    ///   a null pointer, if at the end of the list).
    next: Cell<Option<*const Node>>,
}

impl Default for WaiterList {
    fn default() -> Self {
        Self { head: std::ptr::null() }
    }
}

impl WaiterList {
    /// Waits until a notification for the given address is received.
    ///
    /// The caller must provide the MutexGuard of an Option<T>, where T contains the WaiterList, and
    /// and a function to obtain the WaiterList starting from T.
    ///
    /// If, at any point during the wait, the option value becomes None (i.e. the WaiterList is
    /// destroyed), the wait will immediately stop and return.
    ///
    /// Note: The mutex may be released and re-acquired multiple times while waiting.
    ///
    /// If the requested timeout is exceeded, this function will panic.
    pub fn wait<'a, T>(
        mut guard: MutexGuard<'a, Option<T>>,
        get_waiter_list: impl Fn(&mut T) -> &mut WaiterList,
        address: u64,
        panic_after_timeout: Duration,
    ) -> MutexGuard<'a, Option<T>> {
        let Some(guard_contents) = guard.as_mut() else {
            return guard; // The WaiterList has been destroyed.
        };

        let node = std::pin::pin!(Node {
            waited_address: address,
            condvar: Condvar::new(),
            next: Cell::new(None)
        });

        // Insert it at the head of the list.
        let old_head = std::mem::replace(&mut get_waiter_list(guard_contents).head, &*node);
        node.next.set(Some(old_head));

        // When the address is notified, the node will be removed from the list and its condvar
        // notified.
        let (guard, timeout_result) = node
            .condvar
            .wait_timeout_while(guard, panic_after_timeout, |_| node.next.get().is_some())
            .unwrap();
        if timeout_result.timed_out() {
            panic!("WaiterList::wait timed out while waiting for address {:#x}", address);
        }

        guard
    }

    /// Notifies the first node that is waiting on the given address.
    pub fn notify_one(&mut self, address: u64) {
        let mut prev: Option<&Node> = None;
        let mut it: *const Node = self.head;

        // SAFETY: If `it` is not null, the object it points to is alive, because it's part of this
        // list.
        while let Some(node) = unsafe { it.as_ref() } {
            if node.waited_address == address {
                // Remove the node.
                let next = node.next.take().expect("node must be in the list");
                if let Some(prev) = prev {
                    prev.next.set(Some(next));
                } else {
                    self.head = next;
                }

                // Notify the waiting thread.
                node.condvar.notify_one();

                return;
            } else {
                // Advance to the next node.
                it = node.next.get().expect("node must be in the list");
                prev = Some(node);
            }
        }
    }
}

// Wake up all the waiters when the WaiterList is dropped.
impl Drop for WaiterList {
    fn drop(&mut self) {
        let mut it: *const Node = self.head;

        // SAFETY: If `it` is not null, the object it points to is alive, because it's part of this
        // list.
        while let Some(node) = unsafe { it.as_ref() } {
            // Notify the waiting thread.
            node.condvar.notify_one();

            // Move to the next node.
            it = node.next.take().expect("node must be in the list");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex, mpsc};

    #[test]
    fn test_notify_waited_address() {
        let waiter_list = Arc::new(Mutex::new(Some(WaiterList::default())));
        let guard = waiter_list.lock().unwrap();

        let _notifier_thread = {
            let waiter_list = waiter_list.clone();
            std::thread::spawn(move || {
                let mut guard = waiter_list.lock().unwrap();
                guard.as_mut().unwrap().notify_one(0x1234);
            })
        };

        // Wait for an address that is notified by the above thread as soon as the `wait` function
        // internally releases the lock.
        let guard =
            WaiterList::wait(guard, |waiter_list| waiter_list, 0x1234, Duration::from_secs(10));
        drop(guard);
    }

    #[test]
    fn test_notify_empty_list() {
        let waiter_list = Mutex::new(Some(WaiterList::default()));
        let mut guard = waiter_list.lock().unwrap();
        guard.as_mut().unwrap().notify_one(0x1234);
    }

    #[test]
    #[should_panic(expected = "timed out while waiting for address 0x1234")]
    fn test_wait_timeout() {
        let waiter_list = Mutex::new(Some(WaiterList::default()));
        let guard = waiter_list.lock().unwrap();

        // Wait for an address that is never notified. This will panic due to timeout.
        let guard =
            WaiterList::wait(guard, |waiter_list| waiter_list, 0x1234, Duration::from_millis(1));
        drop(guard);
    }

    #[test]
    fn test_wait_interrupted_by_drop() {
        let waiter_list = Arc::new(Mutex::new(Some(WaiterList::default())));

        let (sender, receiver) = mpsc::channel();

        let _waiter_thread = {
            let waiter_list = waiter_list.clone();
            std::thread::spawn(move || {
                let guard = waiter_list.lock().unwrap();
                sender.send(()).unwrap();

                let guard = WaiterList::wait(
                    guard,
                    |waiter_list| waiter_list,
                    0x1234,
                    Duration::from_secs(30),
                );

                // We have been waken up because of the WaiterList being destroyed.
                assert!(guard.is_none());
            })
        };

        // Wait for the thread to have acquired the mutex before proceeding.
        receiver.recv().unwrap();

        // Drop the WaiterList. This will make wait() return immediately.
        *waiter_list.lock().unwrap() = None;
    }
}
