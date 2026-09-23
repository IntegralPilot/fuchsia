// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::ptr::NonNull;

use crate::notification::Waiter;

pub struct WaiterListLink {
    next: Option<NonNull<Waiter>>,
    prev: Option<NonNull<Waiter>>,
}

pub struct WaiterList {
    head: Option<NonNull<Waiter>>,
    tail: Option<NonNull<Waiter>>,
}

// SAFETY: Pointer is non-null and misaligned which is okay.
const UNLINKED: NonNull<Waiter> = unsafe { NonNull::new_unchecked(1 as *mut Waiter) };

const _WAITER_ALIGN_IS_NOT_1: () = {
    assert!(
        align_of::<Waiter>() > 1,
        "Waiter's alignment must be greater than 1 to distinguish from the UNLINKED state"
    );
};

impl WaiterListLink {
    pub const fn null() -> Self {
        Self { next: Some(UNLINKED), prev: Some(UNLINKED) }
    }

    fn is_linked(&self) -> bool {
        !self.is_unlinked()
    }

    fn is_unlinked(&self) -> bool {
        self.next.is_some_and(|link| link == UNLINKED)
    }
}

impl WaiterList {
    /// Constructs an empty WaiterList.
    pub const fn new() -> Self {
        Self { head: None, tail: None }
    }

    /// Returns true if the given waiter is currently linked to the list
    ///
    /// # Safety
    ///
    /// If waiter is linked, it must be linked to `&self`.
    pub unsafe fn is_linked(&self, waiter: &Waiter) -> bool {
        waiter.link.is_linked()
    }

    /// Returns true if the given waiter is currently unlinked from the list
    ///
    /// # Safety
    ///
    /// If waiter is linked, it must be linked to `&self`.
    pub unsafe fn is_unlinked(&self, waiter: &Waiter) -> bool {
        waiter.link.is_unlinked()
    }

    /// Removes the front element of the list (if there is one) and returns a pointer to the value.
    pub fn pop_front(&mut self) -> Option<NonNull<Waiter>> {
        let head = self.head?;
        // SAFETY: head is present which is obviously in the list
        unsafe {
            self.remove(head);
        }
        Some(head)
    }

    /// Pushes a waiter to the back of the list
    ///
    /// # Safety
    ///
    /// `waiter` must not moved or be dropped before being unlinked.
    pub unsafe fn push_back(&mut self, waiter: NonNull<Waiter>) {
        // SAFETY: Precondition and tail and None are consecutive
        unsafe { self.insert_between(waiter, self.tail, None) };
    }

    /// Inserts a waiter in between prev and anext
    ///
    /// # Safety
    ///
    /// - `waiter` must not moved or be dropped before being unlinked.
    /// - `prev` and `next` must be consecutive elements.
    unsafe fn insert_between(
        &mut self,
        mut waiter: NonNull<Waiter>,
        prev: Option<NonNull<Waiter>>,
        next: Option<NonNull<Waiter>>,
    ) {
        // SAFETY: We have exclusive access to the nodes and all NonNull pointers must be valid.
        unsafe {
            waiter.as_mut().link.prev = prev;
            waiter.as_mut().link.next = next;
            if let Some(mut prev) = prev {
                prev.as_mut().link.next = Some(waiter);
            } else {
                self.head = Some(waiter);
            }
            if let Some(mut next) = next {
                next.as_mut().link.prev = Some(waiter);
            } else {
                self.tail = Some(waiter);
            }
        }
    }

    /// Removes the waiter from the list
    ///
    /// # Safety
    ///
    /// - If `waiter` is linked, it must be linked to `&mut self` (i.e. this specific list).
    pub unsafe fn remove(&mut self, mut waiter: NonNull<Waiter>) {
        // SAFETY: We have exclusive access to the nodes and all NonNull pointers must be valid.
        unsafe {
            let link = &(*waiter.as_ptr()).link;
            if link.is_linked() {
                debug_assert!(self.head.is_some());
                debug_assert!(self.tail.is_some());
                let prev = link.prev;
                let next = link.next;
                // SAFETY: The waker is guaranteed to be linked to this list.
                if let Some(mut prev) = prev {
                    prev.as_mut().link.next = next;
                } else {
                    debug_assert_eq!(self.head, Some(waiter));
                    self.head = next;
                }
                if let Some(mut next) = next {
                    next.as_mut().link.prev = prev;
                } else {
                    debug_assert_eq!(self.tail, Some(waiter));
                    self.tail = prev;
                }
                waiter.as_mut().link.next = Some(UNLINKED);
                waiter.as_mut().link.prev = Some(UNLINKED);
            }
        }
    }
}

impl Drop for WaiterList {
    fn drop(&mut self) {
        while let Some(_) = self.pop_front() {}
    }
}
