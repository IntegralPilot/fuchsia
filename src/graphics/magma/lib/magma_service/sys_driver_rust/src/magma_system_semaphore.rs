// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::magma_common_defs::*;
use crate::magma_system_connection::MagmaStatus;
use crate::traits::LogError;
use crate::{traits, utils};

pub struct MagmaSystemSemaphore {
    global_id: u64,
    msd_semaphore: Box<dyn traits::Semaphore>,
}

impl MagmaSystemSemaphore {
    pub fn new(global_id: u64, msd_semaphore: Box<dyn traits::Semaphore>) -> Self {
        Self { global_id, msd_semaphore }
    }

    pub fn global_id(&self) -> u64 {
        self.global_id
    }

    pub fn msd_semaphore(&self) -> &dyn traits::Semaphore {
        &*self.msd_semaphore
    }
}

#[derive(PartialEq, Debug)]
enum SemaphoreHandleType {
    Event,
    Counter,
}

/// This Semaphore type can be used across different drivers, but a driver is welcome
/// to write their own if they need to.
pub struct Semaphore {
    handle: zx::NullableHandle,
    handle_type: SemaphoreHandleType,
    koid: zx::Koid,
    one_shot: bool,
}

impl Semaphore {
    pub fn import(handle: zx::NullableHandle, flags: u64) -> Result<Self, MagmaStatus> {
        let handle_info = handle
            .basic_info()
            .map_err(|_| MagmaStatus::InvalidArgs)
            .log_err("Failed to get handle info")?;
        let handle_type = match handle_info.object_type {
            zx::ObjectType::EVENT => SemaphoreHandleType::Event,
            zx::ObjectType::COUNTER => SemaphoreHandleType::Counter,
            _ => {
                return Err(MagmaStatus::InvalidArgs).dlog_err(format_args!(
                    "unexpected object type: {}",
                    handle_info.object_type.into_raw()
                ));
            }
        };

        let id = handle
            .koid()
            .map_err(|_| MagmaStatus::InvalidArgs)
            .dlog_err("failed to get event id")?;

        let semaphore = match handle_type {
            SemaphoreHandleType::Event => {
                if flags != 0 {
                    return Err(MagmaStatus::InvalidArgs)
                        .dlog_err(format_args!("invalid flag bits 0x{:x}", flags));
                }
                let event = zx::Event::from(handle);
                Semaphore::new_event_semaphore(event, id)
            }
            SemaphoreHandleType::Counter => {
                const UNHANDLED_FLAG_BITS: u64 = !MAGMA_IMPORT_SEMAPHORE_ONE_SHOT;
                if flags & UNHANDLED_FLAG_BITS != 0 {
                    return Err(MagmaStatus::InvalidArgs).dlog_err(format_args!(
                        "invalid flag bits 0x{:x}",
                        flags & UNHANDLED_FLAG_BITS
                    ));
                }
                let one_shot = flags & MAGMA_IMPORT_SEMAPHORE_ONE_SHOT != 0;
                let counter = zx::Counter::from(handle);
                Semaphore::new_counter_semaphore(counter, id, one_shot)
            }
        };
        Ok(semaphore)
    }

    pub fn new_event_semaphore(event: zx::Event, koid: zx::Koid) -> Self {
        Self {
            handle: event.into(),
            handle_type: SemaphoreHandleType::Event,
            koid,
            one_shot: false,
        }
    }

    pub fn new_counter_semaphore(counter: zx::Counter, koid: zx::Koid, one_shot: bool) -> Self {
        Self { handle: counter.into(), handle_type: SemaphoreHandleType::Counter, koid, one_shot }
    }

    fn get_signals(&self) -> zx::Signals {
        match self.handle_type {
            SemaphoreHandleType::Event => zx::Signals::EVENT_SIGNALED,
            SemaphoreHandleType::Counter => zx::Signals::COUNTER_SIGNALED,
        }
    }

    pub fn signal(&self) {
        fuchsia_trace::flow_begin!("gfx", "event_signal", self.koid.raw_koid().into());
        fuchsia_trace::duration!("magma:sync", "Semaphore::signal", "id" => self.koid);
        fuchsia_trace::flow_begin!("magma:sync", "semaphore_signal", self.koid.raw_koid().into());

        let timestamp = zx::MonotonicInstant::get();
        self.write_timestamp(timestamp.into_nanos());

        let status = self
            .handle
            .as_handle_ref()
            .signal(/*clear_mask=*/ zx::Signals::empty(), self.get_signals());
        utils::debug_assert_ok!(status);
    }

    pub fn wait_and_reset(&self, deadline: zx::MonotonicInstant) -> Result<(), zx::Status> {
        fuchsia_trace::duration!("magma:sync", "Semaphore::wait_and_reset", "id" => self.koid);
        // Returns error result if the wait fails.
        self.handle.wait_one(self.get_signals(), deadline).to_result()?;

        fuchsia_trace::flow_begin!("magma:sync", "semaphore_signal", self.koid.raw_koid().into());

        if self.one_shot {
            return Ok(());
        }

        let status = self
            .handle
            .as_handle_ref()
            .signal(self.get_signals(), /*signal_mask=*/ zx::Signals::empty());

        // This should never fail.
        utils::debug_assert_ok!(status);

        Ok(())
    }

    pub fn koid(&self) -> zx::Koid {
        self.koid
    }

    fn write_timestamp(&self, timestamp: i64) {
        match self.handle_type {
            SemaphoreHandleType::Event => {
                // Nothing to do.
            }
            SemaphoreHandleType::Counter => {
                let status = self.handle.as_handle_ref().cast::<zx::Counter>().write(timestamp);
                utils::debug_assert_ok!(status);
            }
        }
    }

    #[cfg(test)]
    fn read_timestamp(&self) -> i64 {
        match self.handle_type {
            SemaphoreHandleType::Event => 0,
            SemaphoreHandleType::Counter => {
                self.handle.as_handle_ref().cast::<zx::Counter>().read().unwrap()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    fn create_event_semaphore() {
        let event = zx::Event::create();
        let koid = event.koid().expect("getkoid failed");
        let semaphore = Semaphore::new_event_semaphore(event, koid);
        assert_eq!(semaphore.koid, koid);
    }

    #[fuchsia::test]
    fn signal_and_wait() {
        let event = zx::Event::create();
        let koid = event.koid().expect("getkoid failed");
        let semaphore = Semaphore::new_event_semaphore(event.into(), koid);
        let result = semaphore
            .wait_and_reset(zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(1)));
        assert!(result.err() == Some(zx::Status::TIMED_OUT));
        semaphore.signal();
        let result = semaphore
            .wait_and_reset(zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(1)));
        assert!(result.is_ok());
    }

    #[fuchsia::test]
    fn create_counter_semaphore() {
        let counter = zx::Counter::create();
        let koid = counter.koid().expect("getkoid failed");
        let semaphore = Semaphore::new_counter_semaphore(counter, koid, /*one_shot=*/ false);
        assert_eq!(semaphore.koid, koid);
    }

    #[fuchsia::test]
    fn counter_semaphore_timestamps() {
        let counter = zx::Counter::create();
        let koid = counter.koid().expect("getkoid failed");
        let semaphore = Semaphore::new_counter_semaphore(counter, koid, /*one_shot=*/ false);
        assert_eq!(semaphore.read_timestamp(), 0);
        semaphore.signal();
        assert_ne!(semaphore.read_timestamp(), 0);
    }

    #[fuchsia::test]
    fn counter_signal_and_wait() {
        let counter: zx::Counter = zx::Counter::create();
        let koid = counter.koid().expect("getkoid failed");
        let semaphore =
            Semaphore::new_counter_semaphore(counter.into(), koid, /*one_shot=*/ false);
        let result = semaphore
            .wait_and_reset(zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(1)));
        assert!(result.err() == Some(zx::Status::TIMED_OUT));
        semaphore.signal();
        let result = semaphore
            .wait_and_reset(zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(1)));
        assert!(result.is_ok());
        let result = semaphore
            .wait_and_reset(zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(1)));
        assert!(result.err() == Some(zx::Status::TIMED_OUT));
    }

    #[fuchsia::test]
    fn counter_oneshot_signal_and_wait() {
        let counter: zx::Counter = zx::Counter::create();
        let koid = counter.koid().expect("getkoid failed");
        let semaphore =
            Semaphore::new_counter_semaphore(counter.into(), koid, /*one_shot=*/ true);
        let result = semaphore
            .wait_and_reset(zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(1)));
        assert!(result.err() == Some(zx::Status::TIMED_OUT));
        semaphore.signal();
        let result = semaphore
            .wait_and_reset(zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(1)));
        assert!(result.is_ok());
        let result = semaphore
            .wait_and_reset(zx::MonotonicInstant::after(zx::MonotonicDuration::from_seconds(1)));
        assert!(result.is_ok());
    }
}
