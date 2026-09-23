// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! # Starnix Binder Tracing Architecture and Flow Ideology
//!
//! This module defines trace events and flow lifecycle tracking for the Starnix Binder driver.
//! Flow traces correlate asynchronous and multi-threaded IPC events across process and thread
//! boundaries, enabling detailed latency diagnosis (e.g., app launch profiling, AIDL method
//! execution times, work queue scheduling delays, and client wake-up latency).
//!
//! ## 1. Core Architecture & Trace Identifiers
//!
//! Rather than wrapping commands in specialized container types, commands in the Binder command
//! queue (`CommandQueueWithWaitQueue`) and active transaction stacks (`TransactionRole`) carry
//! a 64-bit [`fuchsia_trace::Id`].
//!
//! Flow events are matched across threads by their `(category, trace_id)` tuple:
//! - [`fuchsia_trace::instaflow_begin!`]: Starts a new flow attached to an instant event on the initiating thread.
//! - [`fuchsia_trace::instaflow_step!`]: Emits intermediate milestone(s) attached to an instant event as work moves across threads.
//! - [`fuchsia_trace::instaflow_end!`]: Terminates the flow attached to an instant event when the final response is consumed.
//!
//! ## 2. Synchronous Transaction Flow (`BinderTransaction`)
//!
//! Synchronous transactions follow a **4-step round-trip flow lifecycle** across client and
//! server threads, anchoring directly to enclosing duration slices:
//!
//! ```text
//! Client Thread Track                                Server Thread Track
//! ===================                                ===================
//! [HandleThreadWrite]
//!   └─ [HandleTransaction] (Step 1: flow_begin!)
//!        Client enqueues Command::Transaction
//!        (BC_TRANSACTION)
//!              │
//!              │ ── (Driver Queue Wait & Wakeup) ────────> │
//!              │                                           ▼
//!              │                                 [HandleThreadRead] (Step 2: flow_step!)
//!              │                                   Server dequeues Command::Transaction
//!              │                                   (BR_TRANSACTION)
//!              │                                           │
//!              │                                           │ (Userspace AIDL Execution)
//!              │                                           ▼
//!              │                                 [HandleThreadWrite]
//!              │                                   └─ [HandleReply] (Step 3: flow_step!)
//!              │                                        Server enqueues Command::Reply
//!              │                                        (BC_REPLY)
//!              │ <── (Reply Delivery & Wakeup) ─────────── │
//!              ▼
//! [HandleThreadRead] (Step 4: flow_end!)
//!   Client dequeues Command::Reply
//!   (BR_REPLY)
//! ```
//!
//! ### Sequential Chain in Perfetto UI:
//! In Perfetto UI, this 4-step flow is indexed as a continuous sequential chain:
//! `HandleTransaction (1)` -> `HandleThreadRead (2)` -> `HandleReply (3)`
//! -> `HandleThreadRead (4)`.
//!
//! When inspecting in Perfetto UI:
//! - Clicking **Step 1 (`HandleTransaction`)**: Shows outgoing flows connecting to the server.
//! - Clicking **Step 2 (`HandleThreadRead` on Server)**:
//!   - *Preceding Flow (Incoming)*: `HandleTransaction` (Step 1 from Client).
//!   - *Following Flows (Downstream)*: `HandleReply` (Step 3 on Server) and `HandleThreadRead`
//!     (Step 4 on Client).
//! - Clicking **Step 3 (`HandleReply` on Server)**: Shows the return flow to the client.
//! - Clicking **Step 4 (`HandleThreadRead` on Client)**: Shows incoming flow from server reply.
//!
//! ### Latency Breakdown Derived from the 4 Steps:
//! - **Queue Latency (Step 1 → Step 2)**: Time elapsed while the transaction waits in the
//!   server's command queue until a server thread wakes up and dequeues it.
//! - **Server Execution Time (Step 2 → Step 3)**: Time spent executing the AIDL method in
//!   userspace.
//! - **Reply Delivery Latency (Step 3 → Step 4)**: Time elapsed while the reply is routed back
//!   and the client thread wakes up to read the result.
//!
//! ## 3. Error & Failure Flow Transitions
//!
//! When an error occurs during a transaction, the flow retains the original transaction's
//! `trace_id` so the causality chain is preserved in trace tools like Perfetto:
//!
//! - **Mid-Flight Failure (4-Step Flow)**:
//!   If the server crashes or encounters an error after Step 2, the driver or error dispatcher
//!   generates an error reply (`Command::DeadReply`, `Command::FailedReply`,
//!   `Command::FrozenReply`). Step 3 (`flow_step!`) and Step 4 (`flow_end!`) emit
//!   `"BinderDeadReply"`, `"BinderFailedReply"`, or `"BinderFrozenReply"` with `"status"`
//!   arguments, connecting directly to the original Step 1 & 2.
//!
//! - **Pre-Dequeue / Immediate Failure (3-Step Flow)**:
//!   If target process discovery or dispatch fails immediately in `handle_transaction` (e.g.
//!   target already dead or frozen), the driver enqueues the error reply directly into the
//!   client's queue. The flow consists of:
//!   - Step 1: `"BinderTransaction"` (`flow_begin!`) on client enqueue.
//!   - Step 2: `"BinderDeadReply"` / `"BinderFailedReply"` / `"BinderFrozenReply"` (`flow_step!`)
//!     on driver error reply enqueue.
//!   - Step 3: `"BinderDeadReply"` / `"BinderFailedReply"` / `"BinderFrozenReply"` (`flow_end!`)
//!     on client dequeue.
//!
//! ## 4. Asynchronous / Oneway Transactions (`BinderOneway`)
//!
//! Asynchronous transactions (`BC_TRANSACTION` with `TF_ONE_WAY`) do not block for a reply:
//! - **Step 1 (`flow_begin!`)**: Client enqueues `Command::OnewayTransaction` (`"BinderOneway"`).
//! - **Step 2 (`flow_end!`)**: Server dequeues `Command::OnewayTransaction`.
//! - Tracks queueing and dispatch latency without awaiting server execution.
//!
//! ## 5. Transaction Complete Acknowledgments
//!
//! Submitting threads receive `BR_TRANSACTION_COMPLETE` immediately after submitting a transaction
//! or reply:
//! - `Command::TransactionComplete`: 2-step flow (`"BinderTransactionComplete"`).
//! - `Command::OnewayTransactionComplete`: 2-step flow (`"BinderOnewayTransactionComplete"`).
//!
//! ## 6. Reference Counting & State Notifications
//!
//! Reference updates and lifecycle notifications are 2-step flows (`flow_begin!` when enqueued by
//! driver → `flow_end!` when dequeued by target process):
//! - Ref counts: `"BinderAcquireRef"`, `"BinderReleaseRef"`, `"BinderIncRef"`, `"BinderDecRef"`.
//! - Lifecycle: `"BinderDeathNotification"`, `"BinderFrozenBinder"`,
//!   `"BinderClearDeathNotificationDone"`, `"BinderClearFreezeNotificationDone"`,
//!   `"BinderPendingFrozen"`.
//!
//! ## 7. Trace Arguments
//!
//! Where applicable, flows capture metadata for deep AIDL call inspection:
//! - `code`: AIDL method/transaction code (e.g., `FIRST_CALL_TRANSACTION + N` or
//!   `INTERFACE_TRANSACTION`).
//! - `data_size`: Byte size of the Parcel payload data buffer.
//! - `offsets_size`: Byte size of the Binder object offsets buffer.
//! - `status`: Failure/error state (`"failed_reply"`, `"dead_reply"`, `"frozen_reply"`).

use crate::thread::Command;

// The trace category used for binder related traces.
pub const CATEGORY_STARNIX_BINDER: &'static str = "starnix:binder";

// The name used to track the duration of a local binder ioctl.
pub const NAME_BINDER_IOCTL: &'static str = "BinderIoctl";

// The name used to track handling a thread write in the binder driver.
pub const NAME_HANDLE_THREAD_WRITE: &'static str = "HandleThreadWrite";

// The name used to track handling a thread read in the binder driver.
pub const NAME_HANDLE_THREAD_READ: &'static str = "HandleThreadRead";

// The name used to track handling a binder transaction in the driver.
pub const NAME_HANDLE_TRANSACTION: &'static str = "HandleTransaction";

// The name used to track handling a binder transaction reply in the driver.
pub const NAME_HANDLE_REPLY: &'static str = "HandleReply";

// The names used to track remote binder operations.
pub const NAME_REMOTE_BINDER_IOCTL: &'static str = "RemoteBinderIoctl";
pub const NAME_REMOTE_BINDER_IOCTL_SEND_WORK: &'static str = "RemoteBinderIoctlSendWork";
pub const NAME_REMOTE_BINDER_IOCTL_FIDL_REPLY: &'static str = "RemoteBinderIoctlFidlReply";
pub const NAME_REMOTE_BINDER_IOCTL_WORKER_PROCESS: &'static str = "RemoteBinderIoctlWorkerProcess";

pub fn on_command_enqueued(command: &Command, trace_id: fuchsia_trace::Id) {
    match command {
        Command::Transaction { data, .. } => {
            fuchsia_trace::instaflow_begin!(
                CATEGORY_STARNIX_BINDER,
                "BinderTransaction",
                "enqueue_transaction",
                trace_id,
                "code" => data.code,
                "data_size" => data.buffers.data.length as u64,
                "offsets_size" => data.buffers.offsets.length as u64
            );
        }
        Command::Reply(data) => {
            fuchsia_trace::instaflow_step!(
                CATEGORY_STARNIX_BINDER,
                "BinderTransaction",
                "enqueue_reply",
                trace_id,
                "code" => data.code,
                "data_size" => data.buffers.data.length as u64,
                "offsets_size" => data.buffers.offsets.length as u64
            );
        }
        Command::FailedReply => {
            fuchsia_trace::instaflow_step!(
                CATEGORY_STARNIX_BINDER,
                "BinderFailedReply",
                "enqueue_failed_reply",
                trace_id,
                "status" => "failed_reply"
            );
        }
        Command::DeadReply => {
            fuchsia_trace::instaflow_step!(
                CATEGORY_STARNIX_BINDER,
                "BinderDeadReply",
                "enqueue_dead_reply",
                trace_id,
                "status" => "dead_reply"
            );
        }
        Command::FrozenReply => {
            fuchsia_trace::instaflow_step!(
                CATEGORY_STARNIX_BINDER,
                "BinderFrozenReply",
                "enqueue_frozen_reply",
                trace_id,
                "status" => "frozen_reply"
            );
        }
        Command::OnewayTransaction(data) => {
            fuchsia_trace::instaflow_begin!(
                CATEGORY_STARNIX_BINDER,
                "BinderOneway",
                "enqueue_oneway",
                trace_id,
                "code" => data.code,
                "data_size" => data.buffers.data.length as u64,
                "offsets_size" => data.buffers.offsets.length as u64
            );
        }
        Command::DeadBinder(cookie) => {
            fuchsia_trace::instaflow_begin!(
                CATEGORY_STARNIX_BINDER,
                "BinderDeathNotification",
                "enqueue_dead_binder",
                trace_id,
                "cookie" => *cookie
            );
        }
        Command::AcquireRef(obj) => {
            fuchsia_trace::instaflow_begin!(
                CATEGORY_STARNIX_BINDER,
                "BinderAcquireRef",
                "enqueue_acquire_ref",
                trace_id,
                "weak_ref_addr" => obj.weak_ref_addr.ptr() as u64
            );
        }
        Command::ReleaseRef(obj) => {
            fuchsia_trace::instaflow_begin!(
                CATEGORY_STARNIX_BINDER,
                "BinderReleaseRef",
                "enqueue_release_ref",
                trace_id,
                "weak_ref_addr" => obj.weak_ref_addr.ptr() as u64
            );
        }
        Command::IncRef(obj) => {
            fuchsia_trace::instaflow_begin!(
                CATEGORY_STARNIX_BINDER,
                "BinderIncRef",
                "enqueue_inc_ref",
                trace_id,
                "weak_ref_addr" => obj.weak_ref_addr.ptr() as u64
            );
        }
        Command::DecRef(obj) => {
            fuchsia_trace::instaflow_begin!(
                CATEGORY_STARNIX_BINDER,
                "BinderDecRef",
                "enqueue_dec_ref",
                trace_id,
                "weak_ref_addr" => obj.weak_ref_addr.ptr() as u64
            );
        }
        Command::TransactionComplete => {
            fuchsia_trace::instaflow_begin!(
                CATEGORY_STARNIX_BINDER,
                "BinderTransactionComplete",
                "enqueue_transaction_complete",
                trace_id
            );
        }
        Command::OnewayTransactionComplete => {
            fuchsia_trace::instaflow_begin!(
                CATEGORY_STARNIX_BINDER,
                "BinderOnewayTransactionComplete",
                "enqueue_oneway_transaction_complete",
                trace_id
            );
        }
        Command::PendingFrozen => {
            fuchsia_trace::instaflow_begin!(
                CATEGORY_STARNIX_BINDER,
                "BinderPendingFrozen",
                "enqueue_pending_frozen",
                trace_id
            );
        }
        Command::ClearDeathNotificationDone(cookie) => {
            fuchsia_trace::instaflow_begin!(
                CATEGORY_STARNIX_BINDER,
                "BinderClearDeathNotificationDone",
                "enqueue_clear_death_notification_done",
                trace_id,
                "cookie" => *cookie
            );
        }
        Command::FrozenBinder(info) => {
            fuchsia_trace::instaflow_begin!(
                CATEGORY_STARNIX_BINDER,
                "BinderFrozenBinder",
                "enqueue_frozen_binder",
                trace_id,
                "cookie" => info.cookie,
                "is_frozen" => info.is_frozen
            );
        }
        Command::ClearFreezeNotificationDone(cookie) => {
            fuchsia_trace::instaflow_begin!(
                CATEGORY_STARNIX_BINDER,
                "BinderClearFreezeNotificationDone",
                "enqueue_clear_freeze_notification_done",
                trace_id,
                "cookie" => *cookie
            );
        }
        Command::Error(errno) => {
            fuchsia_trace::instaflow_begin!(
                CATEGORY_STARNIX_BINDER,
                "BinderError",
                "enqueue_error",
                trace_id,
                "errno" => *errno
            );
        }
        Command::SpawnLooper => {}
    }
}

pub fn on_command_dequeued(command: &Command, trace_id: fuchsia_trace::Id) {
    match command {
        Command::Transaction { data, .. } => {
            fuchsia_trace::instaflow_step!(
                CATEGORY_STARNIX_BINDER,
                "BinderTransaction",
                "dequeue_transaction",
                trace_id,
                "code" => data.code,
                "data_size" => data.buffers.data.length as u64,
                "offsets_size" => data.buffers.offsets.length as u64
            );
        }
        Command::Reply(data) => {
            fuchsia_trace::instaflow_end!(
                CATEGORY_STARNIX_BINDER,
                "BinderTransaction",
                "dequeue_reply",
                trace_id,
                "code" => data.code,
                "data_size" => data.buffers.data.length as u64,
                "offsets_size" => data.buffers.offsets.length as u64
            );
        }
        Command::FailedReply => {
            fuchsia_trace::instaflow_end!(
                CATEGORY_STARNIX_BINDER,
                "BinderFailedReply",
                "dequeue_failed_reply",
                trace_id,
                "status" => "failed_reply"
            );
        }
        Command::DeadReply => {
            fuchsia_trace::instaflow_end!(
                CATEGORY_STARNIX_BINDER,
                "BinderDeadReply",
                "dequeue_dead_reply",
                trace_id,
                "status" => "dead_reply"
            );
        }
        Command::FrozenReply => {
            fuchsia_trace::instaflow_end!(
                CATEGORY_STARNIX_BINDER,
                "BinderFrozenReply",
                "dequeue_frozen_reply",
                trace_id,
                "status" => "frozen_reply"
            );
        }
        Command::OnewayTransaction(data) => {
            fuchsia_trace::instaflow_end!(
                CATEGORY_STARNIX_BINDER,
                "BinderOneway",
                "dequeue_oneway",
                trace_id,
                "code" => data.code,
                "data_size" => data.buffers.data.length as u64,
                "offsets_size" => data.buffers.offsets.length as u64
            );
        }
        Command::DeadBinder(cookie) => {
            fuchsia_trace::instaflow_end!(
                CATEGORY_STARNIX_BINDER,
                "BinderDeathNotification",
                "dequeue_dead_binder",
                trace_id,
                "cookie" => *cookie
            );
        }
        Command::AcquireRef(obj) => {
            fuchsia_trace::instaflow_end!(
                CATEGORY_STARNIX_BINDER,
                "BinderAcquireRef",
                "dequeue_acquire_ref",
                trace_id,
                "weak_ref_addr" => obj.weak_ref_addr.ptr() as u64
            );
        }
        Command::ReleaseRef(obj) => {
            fuchsia_trace::instaflow_end!(
                CATEGORY_STARNIX_BINDER,
                "BinderReleaseRef",
                "dequeue_release_ref",
                trace_id,
                "weak_ref_addr" => obj.weak_ref_addr.ptr() as u64
            );
        }
        Command::IncRef(obj) => {
            fuchsia_trace::instaflow_end!(
                CATEGORY_STARNIX_BINDER,
                "BinderIncRef",
                "dequeue_inc_ref",
                trace_id,
                "weak_ref_addr" => obj.weak_ref_addr.ptr() as u64
            );
        }
        Command::DecRef(obj) => {
            fuchsia_trace::instaflow_end!(
                CATEGORY_STARNIX_BINDER,
                "BinderDecRef",
                "dequeue_dec_ref",
                trace_id,
                "weak_ref_addr" => obj.weak_ref_addr.ptr() as u64
            );
        }
        Command::TransactionComplete => {
            fuchsia_trace::instaflow_end!(
                CATEGORY_STARNIX_BINDER,
                "BinderTransactionComplete",
                "dequeue_transaction_complete",
                trace_id
            );
        }
        Command::OnewayTransactionComplete => {
            fuchsia_trace::instaflow_end!(
                CATEGORY_STARNIX_BINDER,
                "BinderOnewayTransactionComplete",
                "dequeue_oneway_transaction_complete",
                trace_id
            );
        }
        Command::PendingFrozen => {
            fuchsia_trace::instaflow_end!(
                CATEGORY_STARNIX_BINDER,
                "BinderPendingFrozen",
                "dequeue_pending_frozen",
                trace_id
            );
        }
        Command::ClearDeathNotificationDone(cookie) => {
            fuchsia_trace::instaflow_end!(
                CATEGORY_STARNIX_BINDER,
                "BinderClearDeathNotificationDone",
                "dequeue_clear_death_notification_done",
                trace_id,
                "cookie" => *cookie
            );
        }
        Command::FrozenBinder(info) => {
            fuchsia_trace::instaflow_end!(
                CATEGORY_STARNIX_BINDER,
                "BinderFrozenBinder",
                "dequeue_frozen_binder",
                trace_id,
                "cookie" => info.cookie,
                "is_frozen" => info.is_frozen
            );
        }
        Command::ClearFreezeNotificationDone(cookie) => {
            fuchsia_trace::instaflow_end!(
                CATEGORY_STARNIX_BINDER,
                "BinderClearFreezeNotificationDone",
                "dequeue_clear_freeze_notification_done",
                trace_id,
                "cookie" => *cookie
            );
        }
        Command::Error(errno) => {
            fuchsia_trace::instaflow_end!(
                CATEGORY_STARNIX_BINDER,
                "BinderError",
                "dequeue_error",
                trace_id,
                "errno" => *errno
            );
        }
        Command::SpawnLooper => {
            fuchsia_trace::instant!(
                CATEGORY_STARNIX_BINDER,
                "SpawnLooper",
                fuchsia_trace::Scope::Thread
            );
        }
    }
}
