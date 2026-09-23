// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

pub mod syscall_signatures {
    include!(rustenv_path::envpath!("SYSCALL_SIGS_PATH"));
}

mod bti;
mod clock;
mod counter;
mod cprng;
mod debug;
mod debuglog;
mod event;
mod fifo;
mod iob;
mod iommu;
mod job;
mod membarrier;
mod msi;
mod nanosleep;
mod object_info;
mod object_property;
mod object_wait;
mod pmt;
mod process;
mod profile;
mod resource;
mod restricted;
mod sampler;
mod smc;
mod socket;
mod stream;
mod system;
mod task;
mod test;
mod thread;
mod ticks;
mod timer;
mod vmar;
mod vmo;
