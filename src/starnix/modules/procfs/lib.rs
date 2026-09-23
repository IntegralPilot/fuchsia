// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "512"]

mod cgroups;
mod config_gz;
mod cpuinfo;
mod device_tree;
mod devices;
mod filesystems;
mod fs;
mod kmsg;
mod loadavg;
mod meminfo;
mod misc;
mod mounts_symlink;
mod net_dev;
pub mod pid_directory;
mod pressure_directory;
mod proc_directory;
mod self_symlink;
mod stat;
mod swaps;
pub mod sys_net;
mod sysctl;
mod sysctl_directory;
mod sysrq;
mod thread_self;
mod uid_cputime;
mod uid_io;
mod uid_procstat;
mod uptime;
mod vmstat;
mod zoneinfo;

pub use fs::proc_fs;
