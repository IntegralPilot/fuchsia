// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::errors::{Errno, errno, error};
use crate::sigset_t;
use linux_uapi as uapi;
use static_assertions::assert_eq_size;
use std::fmt;
use std::ops::{BitAnd, BitOr, Not};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

pub const UNBLOCKABLE_SIGNALS: SigSet = SigSet(SIGKILL.mask() | SIGSTOP.mask());

/// An unchecked signal represents a signal that has not been through verification, and may
/// represent an invalid signal number.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct UncheckedSignal(u64);

impl UncheckedSignal {
    pub fn new(value: u64) -> UncheckedSignal {
        UncheckedSignal(value)
    }

    pub fn is_zero(self) -> bool {
        self.0 == 0
    }

    pub fn raw(self) -> u64 {
        self.0
    }
}
impl From<Signal> for UncheckedSignal {
    fn from(signal: Signal) -> UncheckedSignal {
        UncheckedSignal(signal.number as u64)
    }
}
impl From<u32> for UncheckedSignal {
    fn from(value: u32) -> UncheckedSignal {
        UncheckedSignal(value as u64)
    }
}

/// The `Signal` struct represents a valid signal.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct Signal {
    number: u32,
}

impl Signal {
    /// The signal number, guaranteed to be a value between 1..=NUM_SIGNALS.
    pub fn number(&self) -> u32 {
        self.number
    }

    /// Returns the bitmask for this signal number.
    pub const fn mask(&self) -> u64 {
        1 << (self.number - 1)
    }

    /// Returns true if the signal is a real-time signal.
    pub fn is_real_time(&self) -> bool {
        self.number >= super::SIGRTMIN
    }

    /// Returns true if this signal can't be blocked. This means either SIGKILL or SIGSTOP.
    pub fn is_unblockable(&self) -> bool {
        UNBLOCKABLE_SIGNALS.has_signal(*self)
    }

    /// Used exclusively for PTRACE_O_TRACESYSGOOD
    pub fn set_ptrace_syscall_bit(&mut self) {
        self.number |= 0x80
    }

    pub fn name(&self) -> &'static str {
        if self.is_real_time() {
            return "real time signal";
        }
        match self.number {
            uapi::SIGHUP => "SIGHUP",
            uapi::SIGINT => "SIGINT",
            uapi::SIGQUIT => "SIGQUIT",
            uapi::SIGILL => "SIGILL",
            uapi::SIGTRAP => "SIGTRAP",
            uapi::SIGABRT => "SIGABRT",
            uapi::SIGBUS => "SIGBUS",
            uapi::SIGFPE => "SIGFPE",
            uapi::SIGKILL => "SIGKILL",
            uapi::SIGUSR1 => "SIGUSR1",
            uapi::SIGSEGV => "SIGSEGV",
            uapi::SIGUSR2 => "SIGUSR2",
            uapi::SIGPIPE => "SIGPIPE",
            uapi::SIGALRM => "SIGALRM",
            uapi::SIGTERM => "SIGTERM",
            uapi::SIGSTKFLT => "SIGSTKFLT",
            uapi::SIGCHLD => "SIGCHLD",
            uapi::SIGCONT => "SIGCONT",
            uapi::SIGSTOP => "SIGSTOP",
            uapi::SIGTSTP => "SIGTSTP",
            uapi::SIGTTIN => "SIGTTIN",
            uapi::SIGTTOU => "SIGTTOU",
            uapi::SIGURG => "SIGURG",
            uapi::SIGXCPU => "SIGXCPU",
            uapi::SIGXFSZ => "SIGXFSZ",
            uapi::SIGVTALRM => "SIGVTALRM",
            uapi::SIGPROF => "SIGPROF",
            uapi::SIGWINCH => "SIGWINCH",
            uapi::SIGIO => "SIGIO",
            uapi::SIGPWR => "SIGPWR",
            uapi::SIGSYS => "SIGSYS",
            _ => "unknown signal",
        }
    }

    /// The number of signals, also the highest valid signal number.
    pub const NUM_SIGNALS: u32 = 64;
}

pub const SIGHUP: Signal = Signal { number: uapi::SIGHUP };
pub const SIGINT: Signal = Signal { number: uapi::SIGINT };
pub const SIGQUIT: Signal = Signal { number: uapi::SIGQUIT };
pub const SIGILL: Signal = Signal { number: uapi::SIGILL };
pub const SIGTRAP: Signal = Signal { number: uapi::SIGTRAP };
pub const SIGABRT: Signal = Signal { number: uapi::SIGABRT };
#[allow(dead_code)]
pub const SIGIOT: Signal = Signal { number: uapi::SIGIOT };
pub const SIGBUS: Signal = Signal { number: uapi::SIGBUS };
pub const SIGFPE: Signal = Signal { number: uapi::SIGFPE };
pub const SIGKILL: Signal = Signal { number: uapi::SIGKILL };
pub const SIGUSR1: Signal = Signal { number: uapi::SIGUSR1 };
pub const SIGSEGV: Signal = Signal { number: uapi::SIGSEGV };
pub const SIGUSR2: Signal = Signal { number: uapi::SIGUSR2 };
pub const SIGPIPE: Signal = Signal { number: uapi::SIGPIPE };
pub const SIGALRM: Signal = Signal { number: uapi::SIGALRM };
pub const SIGTERM: Signal = Signal { number: uapi::SIGTERM };
pub const SIGSTKFLT: Signal = Signal { number: uapi::SIGSTKFLT };
pub const SIGCHLD: Signal = Signal { number: uapi::SIGCHLD };
pub const SIGCONT: Signal = Signal { number: uapi::SIGCONT };
pub const SIGSTOP: Signal = Signal { number: uapi::SIGSTOP };
pub const SIGTSTP: Signal = Signal { number: uapi::SIGTSTP };
pub const SIGTTIN: Signal = Signal { number: uapi::SIGTTIN };
pub const SIGTTOU: Signal = Signal { number: uapi::SIGTTOU };
pub const SIGURG: Signal = Signal { number: uapi::SIGURG };
pub const SIGXCPU: Signal = Signal { number: uapi::SIGXCPU };
pub const SIGXFSZ: Signal = Signal { number: uapi::SIGXFSZ };
pub const SIGVTALRM: Signal = Signal { number: uapi::SIGVTALRM };
pub const SIGPROF: Signal = Signal { number: uapi::SIGPROF };
pub const SIGWINCH: Signal = Signal { number: uapi::SIGWINCH };
pub const SIGIO: Signal = Signal { number: uapi::SIGIO };
pub const SIGPWR: Signal = Signal { number: uapi::SIGPWR };
pub const SIGSYS: Signal = Signal { number: uapi::SIGSYS };
#[allow(dead_code)]
pub const SIGRTMIN: Signal = Signal { number: super::SIGRTMIN };

impl TryFrom<UncheckedSignal> for Signal {
    type Error = Errno;

    fn try_from(value: UncheckedSignal) -> Result<Self, Self::Error> {
        let value = u32::try_from(value.0).map_err(|_| errno!(EINVAL))?;
        if (1..=Signal::NUM_SIGNALS).contains(&value) {
            Ok(Signal { number: value })
        } else {
            error!(EINVAL)
        }
    }
}

impl fmt::Debug for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_real_time() {
            write!(f, "SIGRTMIN+{}({})", self.number - super::SIGRTMIN, self.number)
        } else {
            write!(f, "{}({})", self.name(), self.number)
        }
    }
}

/// POSIX defines sigset_t as either a numeric or structure type (so the number of signals can
/// exceed the bits in a machine word). The programmer is supposed to modify this bitfield using
/// sigaddset(), etc. rather than setting bits directly so client code can be agnostic to the
/// definition.
///
///  * On x64, sigset_t is a typedef for "unsigned long".
///  * On ARM64, sigset_t is a structure containing one "unsigned long" member.
///
/// To keep the Starnix code agnostic to the definition, this SigSet type provides a wrapper
/// with a uniform interface for the things we need.
///
/// The layout of this object is designed to be identical to the layout of the current
/// architecture's sigset_t type so UserRef<SigSet> can be used for system calls.
#[repr(transparent)]
#[derive(
    Debug, Copy, Clone, Default, IntoBytes, Eq, KnownLayout, FromBytes, Immutable, PartialEq,
)]
pub struct SigSet(pub std::os::raw::c_ulong);
assert_eq_size!(SigSet, sigset_t);

impl SigSet {
    /// Returns whether this signal set contains the given signal.
    pub fn has_signal(&self, signal: Signal) -> bool {
        (self.0 & (signal.mask() as std::os::raw::c_ulong)) != 0
    }

    /// Returns whether this signal set has any signals in common with another.
    pub fn intersects(&self, other: &Self) -> bool {
        (self.0 & other.0) != 0
    }
}

impl From<sigset_t> for SigSet {
    fn from(value: sigset_t) -> Self {
        // `transmute()` is safe here because this is a POD value of the same size (see
        // assert above).
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        SigSet(unsafe { std::mem::transmute(value) })
    }
}

impl From<SigSet> for sigset_t {
    fn from(val: SigSet) -> Self {
        // `transmute()` is safe here because this is a POD value of the same size (see
        // assert above).
        #[allow(
            clippy::undocumented_unsafe_blocks,
            reason = "Force documented unsafe blocks in Starnix"
        )]
        unsafe {
            std::mem::transmute(val.0)
        }
    }
}

impl BitAnd for SigSet {
    type Output = Self;

    // rhs is the "right-hand side" of the expression `a & b`
    fn bitand(self, rhs: Self) -> Self::Output {
        Self(self.0 & rhs.0)
    }
}

impl BitOr for SigSet {
    type Output = Self;

    // rhs is the "right-hand side" of the expression `a | b`
    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl Not for SigSet {
    type Output = Self;

    fn not(self) -> Self::Output {
        SigSet(!self.0)
    }
}

impl From<Signal> for SigSet {
    /// Constructs a sigset consisting of one signal value.
    fn from(value: Signal) -> Self {
        SigSet(value.mask() as std::os::raw::c_ulong)
    }
}

pub fn sigaltstack_contains_pointer(stack: &uapi::sigaltstack, ptr: u64) -> bool {
    let min = stack.ss_sp.addr as u64;
    let max = (stack.ss_sp.addr as u64).saturating_add(stack.ss_size as u64);
    ptr >= min && ptr <= max
}
