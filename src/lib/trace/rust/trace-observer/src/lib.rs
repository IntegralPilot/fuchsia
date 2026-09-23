// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use fuchsia_trace::{TraceState, trace_state};

/// A waiter that is notified with the tracing state changes.
///
/// # Examples
///
/// ```
/// use fuchsia_trace_observer::TraceObserver;
///
/// let observer = TraceObserver::new();
/// while let Ok(state) = observer.on_state_changed().await {
///     println!("New state: {:?}", state);
/// }
/// ```
pub struct TraceObserver {
    event: zx::Event,
}

impl Drop for TraceObserver {
    fn drop(&mut self) {
        unsafe { sys::trace_unregister_observer(self.event.raw_handle()) }
    }
}

impl TraceObserver {
    /// Creates a new TraceObserver which is ready to be waited on and notified.
    ///
    /// # Examples
    ///
    /// ```
    /// use fuchsia_trace_observer::TraceObserver;
    ///
    /// let observer = TraceObserver::new();
    /// ```
    pub fn new() -> TraceObserver {
        let event = zx::Event::create();
        unsafe { sys::trace_register_observer(event.raw_handle()) };
        Self { event }
    }

    /// Asynchronously wait for the trace state to change, returning the updated state on success.
    ///
    /// # Examples
    ///
    /// ```
    /// use fuchsia_trace_observer::TraceObserver;
    ///
    /// let observer = TraceObserver::new();
    /// while let Ok(state) = observer.on_state_changed().await {
    ///     println!("New state: {:?}", state);
    /// }
    /// ```
    pub async fn on_state_changed(&self) -> Result<TraceState, zx::Status> {
        let _signals = fuchsia_async::OnSignalsRef::new(
            self.event.as_handle_ref(),
            zx::Signals::EVENT_SIGNALED,
        )
        .await?;
        // Careful here, we need to clear the signal from the handle before we read the trace
        // state.
        //
        // When stopping, the engine does the operations
        // 1) set trace state (STOPPING)
        // 2) signal handle
        // 3) set trace state (STOPPED)
        // 4) signal handle
        //
        // We do the operations:
        // a) clear signal
        // b) read state
        //
        // If we do them in the opposite order, the following bug could occur:
        // 1) set trace state (STOPPING)
        // 2) signal handle
        // a) read state (STOPPING)
        // 3) set trace state (STOPPED)
        // 4) signal handle
        // b) clear signal
        //
        // In this case, we would return STOPPING and never check for STOPPED
        self.event.signal(zx::Signals::EVENT_SIGNALED, zx::Signals::NONE)?;
        let state = trace_state();
        unsafe { sys::trace_notify_observer_updated(self.event.raw_handle()) };
        Ok(state)
    }
}

mod sys {
    #![allow(non_camel_case_types)]
    use zx::sys::zx_handle_t;
    // From librust-trace-observer.so
    unsafe extern "C" {
        pub fn trace_register_observer(event: zx_handle_t);
        pub fn trace_notify_observer_updated(event: zx_handle_t);
        pub fn trace_unregister_observer(event: zx_handle_t);
    }
}
