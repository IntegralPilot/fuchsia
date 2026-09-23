// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::input::{create_media_buttons_proxy, schedule_wakeup_power_button};
use crate::ioctl::{CommandCode, WakeupMethod, WakeupTestType, WakeupTimerInfo};
use crate::tracing;
use anyhow::{Result, anyhow};
use starnix_core::device::DeviceOps;
use starnix_core::mm::MemoryAccessorExt;
use starnix_core::perf::TraceEventQueueList;
use starnix_core::task::{CurrentTask, Kernel};
use starnix_core::vfs::{CloseFreeSafe, FileObject, FileOps, NamespaceNode};
use starnix_core::{fileops_impl_dataless, fileops_impl_noop_sync, fileops_impl_seekless};
use starnix_logging::{log_error, log_info};

use starnix_syscalls::{SUCCESS, SyscallResult};
use starnix_uapi::errors::Errno;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::UserRef;
use starnix_uapi::{device_id, error};
use std::sync::{Arc, Weak};
use zx;

#[derive(Clone)]
pub struct WakeupTestDevice {
    commands: Commands,
}

impl CloseFreeSafe for WakeupTestDevice {}

impl WakeupTestDevice {
    pub fn new(kernel: &Arc<Kernel>) -> Self {
        Self { commands: Commands { kernel: Arc::downgrade(kernel) } }
    }
}

impl DeviceOps for WakeupTestDevice {
    fn open(
        &self,
        current_task: &CurrentTask,
        _id: device_id::DeviceId,
        _node: &NamespaceNode,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(WakeupTestDevice::new(current_task.kernel())))
    }
}

#[derive(Clone)]
struct Commands {
    kernel: Weak<Kernel>,
}

impl Commands {
    fn schedule_wakeup(&self, time_ns: i64) -> Result<()> {
        log_info!("WakeupTestDevice::schedule_wakeup creating async task for time_ns: {time_ns}");
        let kernel = self.kernel.upgrade().expect("kernel should exist");

        kernel.kthreads.spawn_future(
            move || async move {
                let media_button_proxy = match create_media_buttons_proxy().await {
                    Ok(proxy) => proxy,
                    Err(e) => {
                        log_error!("Failed to create media buttons proxy: {:?}", e);
                        return;
                    }
                };
                schedule_wakeup_power_button(
                    &media_button_proxy,
                    zx::Duration::from_nanos(time_ns),
                )
                .await;
                // Keep the device around until the timer goes off.
                // Since this is called from via ioctl, it is not feasible to return an EventPair or
                // some other handle that could be used to make the lifetime more event driven.
                // This may be something to revisit if the test is flaky because of the media_button_proxy
                // being dropped before the timer is goes off and the input sent.

                // Sleep for 5 seconds plus the timer duration to ensure the event is sent before the proxy is dropped.
                let deadline = fuchsia_async::MonotonicInstant::after(
                    zx::Duration::from_seconds(5) + zx::Duration::from_nanos(time_ns),
                );
                fuchsia_async::Timer::new(deadline).await;
                log_info!("media_button proxy dropped.")
            },
            "wakeup_test",
        );
        Ok(())
    }

    fn run_wakeup_set_timers(
        &self,
        current_task: &CurrentTask,
        timer_info: WakeupTimerInfo,
    ) -> Result<()> {
        let method = WakeupMethod::from(timer_info.method);

        // TODO(https://fxbug.dev/458389823): Use other input events to wakeup the system.
        match method {
            WakeupMethod::PowerButton => (),
            _ => {
                return Err(anyhow!(
                    "Only PowerButton wakeup method is currently supported: b/458389823"
                ));
            }
        };

        let test_type = WakeupTestType::from(timer_info.test_type);
        log_info!(
            "WakeupTestDevice::WakeupSetTimers test_type: {test_type:?} Setting {} timers for {:?}, interval {} ns, starting {} ns",
            timer_info.num_events,
            method,
            timer_info.interval,
            timer_info.offset
        );
        tracing::trace_wakeup_test_type(
            self.get_trace_event_queues(),
            current_task.get_tid(),
            test_type,
        );
        for index in 0..timer_info.num_events {
            let time = (timer_info.interval * (index as i64)) + timer_info.offset;
            log_info!("WakeupTestDevice::set_timer i: {index} for {time}");
            self.schedule_wakeup(time)?;
        }
        Ok(())
    }

    /// Gets the trace event queue if available to emit trace events.
    fn get_trace_event_queues(&self) -> Option<Arc<TraceEventQueueList>> {
        if let Some(k) = self.kernel.upgrade() {
            let queues = TraceEventQueueList::from(&k);
            if queues.is_enabled() {
                Some(TraceEventQueueList::from(&k))
            } else {
                log_error!("Trace is not enabled");
                None
            }
        } else {
            None
        }
    }
}

impl FileOps for WakeupTestDevice {
    fileops_impl_seekless!();
    fileops_impl_dataless!();
    fileops_impl_noop_sync!();

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: starnix_syscalls::SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        let cmd_num = CommandCode::from(request);
        log_info!("WakeupTestDevice::ioctl cmd_num {cmd_num:?}, arg: {arg:?}");

        match cmd_num {
            CommandCode::WakeupSetTimers => {
                let timer_ref = UserRef::<WakeupTimerInfo>::new(arg.into());
                let timer_info = current_task.read_object(timer_ref)?;
                log_info!("WakeupTestDevice::WakeupSetTimers {timer_info:?}");
                log_info!("WakeupTestDevice::WakeupSetTimers version 0x{:x}", timer_info.version);

                match self.commands.run_wakeup_set_timers(current_task, timer_info) {
                    Ok(_) => Ok(SUCCESS),
                    Err(e) => {
                        log_error!("WakeupTestDevice::WakeupSetTimers failed: {:?}", e);
                        return error!(EINVAL);
                    }
                }
            }
            CommandCode::WakeupTest => error!(ENOSYS),
            CommandCode::WakeupHowManyTimers => error!(ENOSYS),
            CommandCode::WakeupCancelTimers => error!(ENOSYS),
            _ => error!(ENOTTY),
        }
    }
}
