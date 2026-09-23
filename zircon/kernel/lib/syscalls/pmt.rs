// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::object::{HandleValue, PinnedMemoryTokenDispatcher, ProcessDispatcher};
use debug::ltracef;
use syscalls_macro::syscall;
use zx_status::Status;

const LOCAL_TRACE: u32 = 0;

// Having a single-purpose syscall like this is a bit of an anti-pattern in our
// syscall API, but we feel there is benefit in this over trying to extend the
// semantics of handle closing in sys_handle_close and process death. In
// particular, PMTs are the only objects in the system that track the lifetime
// of something external to the process model (external hardware DMA
// capabilities).
#[syscall]
pub fn sys_pmt_unpin(handle: HandleValue) -> Result<(), Status> {
    ltracef!("handle {:#x}\n", handle.raw_value());

    let handle_owner =
        ProcessDispatcher::with_current(|up| up.remove_handle(handle)).ok_or(Status::BAD_HANDLE)?;

    let dispatcher = handle_owner.dispatcher();
    let pmt_dispatcher =
        dispatcher.downcast::<PinnedMemoryTokenDispatcher>().ok_or(Status::WRONG_TYPE)?;

    pmt_dispatcher.unpin();

    Ok(())
}
