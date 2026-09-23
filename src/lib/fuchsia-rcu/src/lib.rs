// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod atomic_stack;
mod rcu_arc;
mod rcu_box;
mod rcu_droppable;
mod rcu_droppable_arc;
mod rcu_option_arc;
mod rcu_option_box;
mod rcu_ptr;
mod rcu_read_scope;
mod rcu_weak;
mod read_counters;
mod state_machine;

pub use fuchsia_rcu_derive::RcuDroppable;
pub use rcu_arc::RcuArc;
pub use rcu_box::RcuBox;
pub use rcu_droppable::RcuDroppable;
pub use rcu_droppable_arc::RcuDroppableArc;
pub use rcu_option_arc::RcuOptionArc;
pub use rcu_option_box::RcuOptionBox;
pub use rcu_ptr::RcuReadGuard;
pub use rcu_read_scope::RcuReadScope;
pub use rcu_weak::RcuWeak;
pub use state_machine::{rcu_drop, rcu_run_callbacks, rcu_synchronize, with_thread_block_counters};

pub mod subtle {
    pub use super::rcu_droppable_arc::{rcu_ptr_to_arc, rcu_ptr_upgrade};
    pub use super::rcu_ptr::{RcuPtr, RcuPtrRef};
}
