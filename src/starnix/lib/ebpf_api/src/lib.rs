// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#[cfg(target_os = "fuchsia")]
mod helpers;
#[cfg(target_os = "fuchsia")]
mod maps;
mod program_type;

#[cfg(target_os = "fuchsia")]
pub use helpers::*;
#[cfg(target_os = "fuchsia")]
pub use maps::*;
pub use program_type::*;

pub use linux_uapi::{__sk_buff, bpf_sock, uaddr, uid_t};
pub const BPF_MAP_TYPE_HASH: u32 = linux_uapi::bpf_map_type_BPF_MAP_TYPE_HASH;
