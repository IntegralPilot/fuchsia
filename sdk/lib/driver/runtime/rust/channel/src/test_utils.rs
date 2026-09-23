// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Helpers for writing tests involving channels.

use std::cell::Cell;
use std::sync::mpsc;

/// Implements a cloneable object that will send only one message
/// on an [`mpsc::Sender`] when its 'last' clone is dropped. It will assert
/// if an attempt to re-clone an already cloned [`DropSender`] happens,
/// ensuring that the object is only cloned in a linear path.
pub struct DropSender<T: Clone>(pub T, Cell<Option<mpsc::Sender<T>>>);
impl<T: Clone> DropSender<T> {
    /// When this object is dropped it will send 'val' to the given 'sender'.
    pub fn new(val: T, sender: mpsc::Sender<T>) -> Self {
        Self(val, Cell::new(Some(sender)))
    }
}
impl<T: Clone> Drop for DropSender<T> {
    fn drop(&mut self) {
        if let Some(sender) = self.1.get_mut() {
            println!("dropping a drop sender");
            sender.send(self.0.clone()).unwrap();
        }
    }
}
impl<T: Clone> Clone for DropSender<T> {
    fn clone(&self) -> Self {
        Self(
            self.0.clone(),
            Cell::new(Some(self.1.take().expect("Attempted to re-clone a `DropSender`"))),
        )
    }
}
