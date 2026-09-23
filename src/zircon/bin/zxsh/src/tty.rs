// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_hardware_pty as fpty;
use std::os::fd::AsFd;
use zx::Task;

use bitflags::bitflags;

bitflags! {
    /// Represents POSIX-like software signals managed by the shell.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub struct ShellSignals: u32 {
        const INT  = 1 << 0;
        const TERM = 1 << 1;
        const HUP  = 1 << 2;
        const QUIT = 1 << 3;
    }
}

impl ShellSignals {
    /// List of all supported signal flags and their corresponding trap names.
    pub const ALL: &'static [(ShellSignals, &'static [u8])] = &[
        (ShellSignals::INT, b"INT"),
        (ShellSignals::TERM, b"TERM"),
        (ShellSignals::HUP, b"HUP"),
        (ShellSignals::QUIT, b"QUIT"),
    ];

    /// Returns the POSIX signal number associated with a single signal flag.
    pub fn posix_number(&self) -> Option<i32> {
        match *self {
            ShellSignals::INT => Some(libc::SIGINT),
            ShellSignals::TERM => Some(libc::SIGTERM),
            ShellSignals::HUP => Some(libc::SIGHUP),
            ShellSignals::QUIT => Some(libc::SIGQUIT),
            _ => None,
        }
    }

    /// Returns the standard shell exit code (128 + POSIX signal number) for this signal.
    pub fn exit_code(&self) -> Option<i32> {
        self.posix_number().map(|num| 128 + num)
    }
}

/// Represents the state of pending signals in the shell.
#[derive(Clone, Copy, Default)]
pub struct ShellSignalState {
    pub pending: ShellSignals,
}

impl ShellSignalState {
    /// Creates a new empty `ShellSignalState`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the specified signal flag(s) as pending.
    pub fn set(&mut self, sig: ShellSignals) {
        self.pending.insert(sig);
    }

    /// Clears the specified signal flag(s).
    pub fn clear(&mut self, sig: ShellSignals) {
        self.pending.remove(sig);
    }

    /// Checks if the specified signal flag(s) are pending.
    pub fn is_pending(&self, sig: ShellSignals) -> bool {
        self.pending.contains(sig)
    }

    /// Takes all pending signals, clearing them from the state.
    pub fn take_pending(&mut self) -> ShellSignals {
        std::mem::take(&mut self.pending)
    }

    /// If any signal is pending, returns the exit code (128 + POSIX signal number) for the first
    /// pending signal.
    pub fn pending_exit_code(&self) -> Option<i32> {
        for &(sig, _) in ShellSignals::ALL {
            if self.is_pending(sig) {
                return sig.exit_code();
            }
        }
        None
    }
}

/// Controls a PTY device.
pub struct PtyControl {
    /// The synchronous proxy to the PTY device.
    pub proxy: fpty::DeviceSynchronousProxy,
    /// The event pair associated with the PTY device for signaling.
    pub event: zx::EventPair,
}

/// Attempts to get PTY control from a file descriptor.
///
/// Returns `None` if the file descriptor is not a PTY or if cloning the channel fails.
pub fn get_pty_control(fd: &impl AsFd) -> Option<PtyControl> {
    let channel = fdio::clone_channel(fd).ok()?;
    let proxy = fpty::DeviceSynchronousProxy::new(channel);
    let info = proxy.describe(zx::MonotonicInstant::INFINITE).ok()?;
    let event = info.event?;
    Some(PtyControl { proxy, event })
}

fn kill_and_wait(
    proc: &zx::Process,
    signal_state: &mut ShellSignalState,
) -> Result<zx::Signals, zx::Status> {
    signal_state.set(ShellSignals::INT);
    let _ = proc.kill();
    proc.wait_one(zx::Signals::PROCESS_TERMINATED, zx::MonotonicInstant::INFINITE).to_result()
}

/// Waits for a process to terminate, while allowing interruption via PTY events (e.g., Ctrl+C).
///
/// If `pty_control` is provided, it monitors PTY events for interrupts.
/// If SIGINT is received or was already received, it kills the process and returns.
pub fn wait_for_process_with_interrupt(
    proc: &zx::Process,
    pty_control: Option<&PtyControl>,
    signal_state: &mut ShellSignalState,
) -> Result<zx::Signals, zx::Status> {
    if signal_state.is_pending(ShellSignals::INT) {
        return kill_and_wait(proc, signal_state);
    }

    if let Some(pty) = pty_control {
        let mut items = [
            proc.wait_item(zx::Signals::PROCESS_TERMINATED),
            pty.event.wait_item(zx::Signals::USER_1 | zx::Signals::OBJECT_PEER_CLOSED),
        ];

        loop {
            if signal_state.is_pending(ShellSignals::INT) {
                return kill_and_wait(proc, signal_state);
            }

            match zx::object_wait_many(&mut items, zx::MonotonicInstant::INFINITE) {
                Ok(_) => {
                    if items[0].pending().contains(zx::Signals::PROCESS_TERMINATED) {
                        return Ok(items[0].pending());
                    }
                    if items[1].pending().contains(zx::Signals::USER_1) {
                        match pty.proxy.read_events(zx::MonotonicInstant::INFINITE) {
                            Ok((status, events)) => {
                                if zx::Status::ok(status).is_ok() {
                                    if (events & fpty::EVENT_INTERRUPT) != 0 {
                                        signal_state.set(ShellSignals::INT);
                                        return kill_and_wait(proc, signal_state);
                                    }
                                }
                            }
                            Err(_) => {}
                        }
                    }
                    if items[1].pending().contains(zx::Signals::OBJECT_PEER_CLOSED) {
                        proc.wait_one(
                            zx::Signals::PROCESS_TERMINATED,
                            zx::MonotonicInstant::INFINITE,
                        )
                        .to_result()?;
                        return Ok(zx::Signals::PROCESS_TERMINATED);
                    }
                }
                Err(status) => return Err(status),
            }
        }
    } else {
        proc.wait_one(zx::Signals::PROCESS_TERMINATED, zx::MonotonicInstant::INFINITE).to_result()
    }
}
