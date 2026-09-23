// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use libasync_sys::{
    ASYNC_OPS_V4, async_ops_t, async_ops_v1, async_ops_v2, async_ops_v3, async_ops_v4,
};

pub mod v1;
pub mod v2;
pub mod v3;
pub mod v4;

pub const ASYNC_OPS: async_ops_t = async_ops_t {
    version: ASYNC_OPS_V4,
    reserved: 0,
    v1: async_ops_v1 {
        now: Some(v1::now),
        begin_wait: Some(v1::begin_wait),
        cancel_wait: Some(v1::cancel_wait),
        post_task: Some(v1::post_task),
        cancel_task: Some(v1::cancel_task),
        queue_packet: Some(v1::queue_packet),
        set_guest_bell_trap: Some(v1::set_guest_bell_trap),
    },
    v2: async_ops_v2 {
        bind_irq: Some(v2::bind_irq),
        unbind_irq: Some(v2::unbind_irq),
        create_paged_vmo: Some(v2::create_paged_vmo),
        detach_paged_vmo: Some(v2::detach_paged_vmo),
    },
    v3: async_ops_v3 {
        get_sequence_id: Some(v3::get_sequence_id),
        check_sequence_id: Some(v3::check_sequence_id),
    },
    v4: async_ops_v4 {
        acquire_shared_ref: Some(v4::acquire_shared_ref),
        release_shared_ref: Some(v4::release_shared_ref),
    },
};
