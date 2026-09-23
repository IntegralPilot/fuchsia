// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::fs::fuchsia::RemoteCounter;
use crate::mm::MemoryAccessorExt;
use crate::task::{
    CurrentTask, EventHandler, ManyZxHandleSignalHandler, SignalHandler, SignalHandlerInner,
    WaitCanceler, Waiter,
};
use crate::vfs::buffers::{InputBuffer, OutputBuffer};
use crate::vfs::{
    Anon, FdFlags, FdNumber, FileHandle, FileObject, FileOps, fileops_impl_nonseekable,
    fileops_impl_noop_sync,
};

use starnix_lifecycle::AtomicCounter;
use starnix_logging::{impossible_error, log_warn};
use starnix_syscalls::{SUCCESS, SyscallArg, SyscallResult};
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::{UserAddress, UserRef};
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{
    SYNC_IOC_MAGIC, c_char, error, sync_fence_info, sync_file_info, sync_merge_data,
};
use std::collections::HashSet;
use std::sync::Arc;

// Implementation of the sync framework described at:
// https://source.android.com/docs/core/graphics/sync
//
// A sync point "is a single value or point on a sync_timeline. A point has three states: active,
// signaled, and error. Points start in the active state and transition to the signaled or error
// states."  A timestamp of the state transition is returned by the ioctl SYNC_IOC_FILE_INFO,
// so we use VMOs to implement the sync point.  The timestamp is stored in the first 8 bytes of
// the VMO and should be stored before the object signal state change.  This timestamp is always
// early; while in most cases only by a slight amount, the difference could be substantial if the
// signaling thread is de-scheduled in the middle of the two syscalls.
// TODO(b/305781995) - use events instead of VMOs.
//

const SYNC_IOC_MERGE: u8 = 3;
const SYNC_IOC_FILE_INFO: u8 = 4;
const TRACE_CATEGORY: &'static str = "gfx";

#[derive(Clone, Debug)]
pub enum Timeline {
    Magma,
    Hwc,
}

#[derive(PartialEq, Copy, Clone)]
// Error status (-1) is not currently used.
pub enum Status {
    Active = 0,
    Signaled = 1,
}

#[derive(Clone)]
pub struct SyncPoint {
    pub timeline: Timeline,
    pub counter: Arc<zx::Counter>,
    koid: std::sync::OnceLock<zx::Koid>,
}

impl SyncPoint {
    pub fn new(timeline: Timeline, counter: zx::Counter) -> SyncPoint {
        SyncPoint { timeline, counter: Arc::new(counter), koid: std::sync::OnceLock::new() }
    }

    pub fn with_koid(timeline: Timeline, counter: zx::Counter, koid: zx::Koid) -> SyncPoint {
        let once_lock = std::sync::OnceLock::new();
        let _ = once_lock.set(koid);
        SyncPoint { timeline, counter: Arc::new(counter), koid: once_lock }
    }

    pub fn koid(&self) -> zx::Koid {
        *self.koid.get_or_init(|| self.counter.koid().unwrap())
    }
}

pub struct SyncFence {
    pub sync_points: Vec<SyncPoint>,
}

pub struct SyncFile {
    pub name: [u8; 32],
    pub fence: SyncFence,
}

struct FenceState {
    status: Status,
    timestamp_ns: u64,
}

impl SyncFile {
    const SIGNALS: zx::Signals = zx::Signals::COUNTER_SIGNALED;

    /// Returns a `FileHandle` with the specified `name` and `fence`.
    pub fn new_file(
        current_task: &CurrentTask,
        name: [u8; 32],
        fence: SyncFence,
    ) -> Result<FileHandle, Errno> {
        // The Linux `create_sync_file()` helper is described as using `anon_inode_getfile()` to
        // wrap the file-ops into a file, which means using the singleton private anonymous inode.
        Ok(Anon::new_private_file(
            current_task,
            Box::new(SyncFile::new(name, fence)),
            OpenFlags::RDWR,
            "sync_file",
        ))
    }

    pub fn new(name: [u8; 32], fence: SyncFence) -> SyncFile {
        SyncFile { name, fence }
    }

    fn get_fence_state(&self) -> Vec<FenceState> {
        let mut state = Vec::with_capacity(self.fence.sync_points.len());

        for sync_point in &self.fence.sync_points {
            let timestamp_ns = sync_point.counter.read().unwrap() as u64;
            if timestamp_ns > 0 {
                state.push(FenceState { status: Status::Signaled, timestamp_ns });
            } else {
                state.push(FenceState { status: Status::Active, timestamp_ns: 0 });
            }
        }
        state
    }
}

impl FileOps for SyncFile {
    fileops_impl_nonseekable!();
    fileops_impl_noop_sync!();

    fn to_handle(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
    ) -> Result<Option<zx::NullableHandle>, Errno> {
        error!(ENOTSUP)
    }

    fn get_handles(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
    ) -> Result<Vec<zx::NullableHandle>, Errno> {
        let mut handles = Vec::with_capacity(self.fence.sync_points.len());
        for sync_point in &self.fence.sync_points {
            let handle = sync_point
                .counter
                .duplicate_handle(zx::Rights::SAME_RIGHTS)
                .map_err(impossible_error)?
                .into();
            handles.push(handle);
        }
        Ok(handles)
    }

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        let user_addr = UserAddress::from(arg);
        let ioctl_type = (request >> 8) as u8;
        let ioctl_number = request as u8;

        if ioctl_type != SYNC_IOC_MAGIC {
            log_warn!("Unexpected type {:?}", ioctl_type);
            return error!(EINVAL);
        }

        match ioctl_number {
            SYNC_IOC_MERGE => {
                fuchsia_trace::duration!(TRACE_CATEGORY, "SyncFileMerge");
                let user_ref = UserRef::new(user_addr);
                let mut merge_data: sync_merge_data = current_task.read_object(user_ref)?;
                let file2 = current_task.files().get(FdNumber::from_raw(merge_data.fd2))?;

                let file2_sync = file2.downcast_file::<SyncFile>();
                let max_capacity = self.fence.sync_points.len()
                    + if let Some(file2) = file2_sync {
                        file2.fence.sync_points.len()
                    } else {
                        // Remote counter is represented by a single sync point.
                        1
                    };

                let mut fence = SyncFence { sync_points: Vec::with_capacity(max_capacity) };
                let mut set = HashSet::<zx::Koid>::with_capacity(max_capacity);

                for sync_point in &self.fence.sync_points {
                    let koid = sync_point.koid();
                    if set.insert(koid) {
                        fence.sync_points.push(sync_point.clone());
                    }
                }

                if let Some(file2) = file2_sync {
                    for sync_point in &file2.fence.sync_points {
                        let koid = sync_point.koid();
                        if set.insert(koid) {
                            fence.sync_points.push(sync_point.clone());
                        }
                    }
                } else if let Some(file2) = file2.downcast_file::<RemoteCounter>() {
                    let counter = file2.duplicate_handle()?;
                    let sp = SyncPoint::with_koid(Timeline::Hwc, counter.into(), file2.koid());
                    if set.insert(sp.koid()) {
                        fence.sync_points.push(sp);
                    }
                } else {
                    return error!(EINVAL);
                }

                // Filter out signaled sync points in-place with zero heap allocations.
                let mut last_signaled_timestamp_ns = 0;
                let mut last_signaled_sync_point: Option<SyncPoint> = None;

                fence.sync_points.retain(|sync_point| {
                    let timestamp_ns = match sync_point.counter.read() {
                        Ok(t) if t > 0 => t as u64,
                        _ => return true, // Retain active points
                    };

                    // Signaled!
                    if timestamp_ns >= last_signaled_timestamp_ns {
                        last_signaled_timestamp_ns = timestamp_ns;
                        last_signaled_sync_point = Some(sync_point.clone());
                    }
                    false // Filter out signaled points
                });

                if fence.sync_points.is_empty() {
                    fence.sync_points.push(last_signaled_sync_point.expect("No sync points left."));
                }

                let name = merge_data.name.map(|x| x as u8);
                let file = SyncFile::new_file(current_task, name, fence)?;

                let fd = current_task.add_file(file, FdFlags::empty())?;
                merge_data.fence = fd.raw();

                current_task.write_object(user_ref, &merge_data)?;
                Ok(SUCCESS)
            }
            SYNC_IOC_FILE_INFO => {
                fuchsia_trace::duration!(TRACE_CATEGORY, "SyncFileInfo");
                let user_ref = UserRef::new(user_addr);
                let mut info: sync_file_info = current_task.read_object(user_ref)?;

                for i in 0..self.name.len() {
                    info.name[i] = self.name[i] as c_char;
                }
                info.status = 0;

                if info.num_fences == 0 {
                    info.num_fences = self.fence.sync_points.len() as u32;
                } else if info.num_fences > self.fence.sync_points.len() as u32 {
                    return error!(EINVAL);
                } else {
                    let fence_state = self.get_fence_state();
                    let mut user_addr = info.sync_fence_info;

                    let mut sync_file_status = 1;
                    for (i, state) in fence_state.iter().enumerate() {
                        if state.status == Status::Active {
                            sync_file_status = 0;
                        }
                        if i < info.num_fences as usize {
                            // Note: obj_name not supported.
                            let mut fence_info = sync_fence_info {
                                status: state.status as i32,
                                timestamp_ns: state.timestamp_ns,
                                ..sync_fence_info::default()
                            };
                            let driver_name = match self.fence.sync_points[i].timeline {
                                Timeline::Magma => b"Magma\0",
                                Timeline::Hwc => b"Hwc\0\0\0",
                            };
                            assert!(driver_name.len() <= fence_info.driver_name.len());
                            for i in 0..driver_name.len() {
                                fence_info.driver_name[i] = driver_name[i] as c_char;
                            }

                            let fence_user_ref = UserRef::new(UserAddress::from(user_addr));
                            user_addr += std::mem::size_of::<sync_fence_info>() as u64;

                            current_task.write_object(fence_user_ref, &fence_info)?;
                        }
                    }

                    info.status = sync_file_status;
                }

                current_task.write_object(user_ref, &info)?;
                Ok(SUCCESS)
            }
            _ => {
                error!(EINVAL)
            }
        }
    }

    fn wait_async(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        event_handler: EventHandler,
    ) -> Option<WaitCanceler> {
        if !events.contains(FdEvents::POLLIN) {
            return None;
        }

        let count = Arc::<AtomicCounter<usize>>::new(0.into());

        let mut canceler = WaitCanceler::new_noop();

        for sync_point in &self.fence.sync_points {
            let signal_handler = SignalHandler {
                inner: SignalHandlerInner::ManyZxHandle(ManyZxHandleSignalHandler {
                    count: self.fence.sync_points.len(),
                    counter: count.clone(),
                    expected_signals: Self::SIGNALS,
                    events: FdEvents::POLLIN,
                }),
                event_handler: event_handler.clone(),
                err_code: None,
            };

            let canceler_result = waiter.wake_on_zircon_signals(
                sync_point.counter.as_ref(),
                Self::SIGNALS,
                signal_handler,
            );
            let canceler_result = match canceler_result {
                Ok(o) => o,
                Err(e) => {
                    log_warn!("Error returned from wake_on_zircon_signals: {:?}", e);
                    return None;
                }
            };

            // The wakeup is edge triggered, so handles that were already signaled will never get
            // a callback. Normally the "already signaled" case is handled by a call to
            // query_events() after this query_async() returns; however that works only if all
            // handles are signaled.  Here we perform the counting, and cancel waits, for any
            // handles currently signaled.
            if sync_point.counter.wait_one(Self::SIGNALS, zx::MonotonicInstant::ZERO).to_result()
                == Err(zx::Status::TIMED_OUT)
            {
                canceler = WaitCanceler::merge_unbounded(
                    canceler,
                    WaitCanceler::new_port(canceler_result),
                );
            } else {
                canceler_result.cancel();
                count.next();
            }
        }

        Some(canceler)
    }

    fn query_events(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        let fence_state = self.get_fence_state();

        for state in fence_state.iter() {
            if state.status == Status::Active {
                return Ok(FdEvents::empty());
            }
        }

        Ok(FdEvents::POLLIN)
    }

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        error!(ENODEV)
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        error!(ENODEV)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use crate::mm::PAGE_SIZE;
    use crate::testing::*;
    use crate::vfs::{FdFlags, FdNumber};
    use starnix_uapi::sync_merge_data;
    use starnix_uapi::user_address::UserRef;

    #[::fuchsia::test]
    async fn test_sync_file_merge() {
        spawn_kernel_and_run(async |current_task| {
            let counter1 = zx::Counter::create();
            let counter2 = zx::Counter::create();

            let sp1 = SyncPoint::new(Timeline::Magma, counter1);
            let sp2 = SyncPoint::new(Timeline::Hwc, counter2);

            let file1 = SyncFile::new_file(
                &current_task,
                [0; 32],
                SyncFence { sync_points: vec![sp1.clone()] },
            )
            .unwrap();
            let file2 = SyncFile::new_file(
                &current_task,
                [0; 32],
                SyncFence { sync_points: vec![sp2.clone()] },
            )
            .unwrap();

            let fd2 = current_task.add_file(file2, FdFlags::empty()).unwrap();

            let merge_data =
                sync_merge_data { name: [0; 32], fd2: fd2.raw(), fence: 0, flags: 0, pad: 0 };

            let user_addr = map_memory(&current_task, UserAddress::default(), *PAGE_SIZE);
            current_task.write_object(UserRef::new(user_addr), &merge_data).unwrap();

            let request = ((SYNC_IOC_MAGIC as u32) << 8) | (SYNC_IOC_MERGE as u32);

            let sync_file1 = file1.downcast_file::<SyncFile>().unwrap();
            let res = sync_file1.ioctl(&file1, &current_task, request, user_addr.into()).unwrap();
            assert_eq!(res, SUCCESS);

            let updated_merge_data: sync_merge_data =
                current_task.read_object(UserRef::new(user_addr)).unwrap();
            let new_fd = FdNumber::from_raw(updated_merge_data.fence);

            let new_file = current_task.files().get(new_fd).unwrap();
            let merged_sync_file = new_file.downcast_file::<SyncFile>().unwrap();

            assert_eq!(merged_sync_file.fence.sync_points.len(), 2);
            assert_eq!(merged_sync_file.fence.sync_points[0].koid(), sp1.koid());
            assert_eq!(merged_sync_file.fence.sync_points[1].koid(), sp2.koid());
        })
        .await;
    }
}
