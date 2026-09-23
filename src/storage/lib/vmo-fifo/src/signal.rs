// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Use Zircon user signals to coordinate queue states between producer and consumer.
// We use a pair of alternating signals for each event type (data available vs. space available)
// to eliminate the extra syscall required to clear signal bits after waking up.
pub const SIG_DATA_AVAILABLE_0: zx::Signals = zx::Signals::USER_0;
pub const SIG_DATA_AVAILABLE_1: zx::Signals = zx::Signals::USER_1;
pub const SIG_SPACE_AVAILABLE_0: zx::Signals = zx::Signals::USER_2;
pub const SIG_SPACE_AVAILABLE_1: zx::Signals = zx::Signals::USER_3;
pub const SIG_SHUTDOWN: zx::Signals = zx::Signals::USER_4;

// A synchronization helper that coordinates event waiting and signaling on a VMO using a pair of
// Zircon user signals and toggling between them.
pub(crate) struct EventSignal {
    set_next: zx::Signals,
    clear_next: zx::Signals,
}

impl EventSignal {
    pub(crate) const fn new(sig_0: zx::Signals, sig_1: zx::Signals) -> Self {
        Self { set_next: sig_0, clear_next: sig_1 }
    }

    // Blocks until the currently expected signal bit (or `shutdown_sig`) is asserted.
    pub(crate) fn wait(
        &mut self,
        vmo: &zx::Vmo,
        shutdown_sig: zx::Signals,
    ) -> Result<(), zx::Status> {
        let mask = self.set_next | shutdown_sig;

        let observed = vmo.wait_one(mask, zx::MonotonicInstant::INFINITE).to_result()?;
        if observed.contains(shutdown_sig) {
            return Err(zx::Status::CANCELED);
        }

        std::mem::swap(&mut self.set_next, &mut self.clear_next);
        Ok(())
    }

    pub(crate) async fn wait_async(
        &mut self,
        vmo: &zx::Vmo,
        shutdown_sig: zx::Signals,
    ) -> Result<(), zx::Status> {
        let mask = self.set_next | shutdown_sig;

        let observed = fuchsia_async::OnSignals::new(vmo, mask).await?;
        if observed.contains(shutdown_sig) {
            return Err(zx::Status::CANCELED);
        }

        std::mem::swap(&mut self.set_next, &mut self.clear_next);
        Ok(())
    }

    pub(crate) fn signal(&mut self, vmo: &zx::Vmo) -> Result<(), zx::Status> {
        // Asserts the active signal bit and clears the alternate bit.
        vmo.signal(self.clear_next, self.set_next)?;
        std::mem::swap(&mut self.set_next, &mut self.clear_next);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_async::{DurationExt, Timer};

    #[fuchsia::test]
    fn test_event_signal() {
        let vmo = zx::Vmo::create(4096).expect("VMO creation failed");
        let mut event_sender = EventSignal::new(SIG_DATA_AVAILABLE_0, SIG_DATA_AVAILABLE_1);
        let mut event_receiver = EventSignal::new(SIG_DATA_AVAILABLE_0, SIG_DATA_AVAILABLE_1);

        // First cycle
        event_sender.signal(&vmo).expect("signal failed");
        event_receiver.wait(&vmo, SIG_SHUTDOWN).expect("wait failed");

        // Second cycle
        event_sender.signal(&vmo).expect("signal failed");
        event_receiver.wait(&vmo, SIG_SHUTDOWN).expect("wait failed");
    }

    #[fuchsia::test]
    async fn test_event_signal_async_concurrency() {
        let vmo = zx::Vmo::create(4096).expect("VMO creation failed");
        let mut event_sender = EventSignal::new(SIG_DATA_AVAILABLE_0, SIG_DATA_AVAILABLE_1);
        let mut event_receiver = EventSignal::new(SIG_DATA_AVAILABLE_0, SIG_DATA_AVAILABLE_1);

        // This task will wait for the signal before it is sent.
        let wait_task = async {
            event_receiver.wait_async(&vmo, SIG_SHUTDOWN).await.expect("wait failed");
        };

        // This task will delay briefly, then send the signal.
        let signal_task = async {
            Timer::new(zx::MonotonicDuration::from_millis(1).after_now()).await;
            event_sender.signal(&vmo).expect("signal failed");
        };

        // Run the tasks concurrently. If the wait_task blocks the executor thread synchronously,
        // this join will deadlock.
        futures::join!(wait_task, signal_task);
    }
}
