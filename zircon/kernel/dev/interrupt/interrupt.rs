// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

/// The maximum number of interrupts supported in the system.
pub const MAX_INTERRUPTS: usize = interrupt_limits::MAX_INTERRUPTS as usize;

/// The maximum number of MSI IRQs.
pub const MAX_MSI_IRQS: usize = 32;

/// Represents an interrupt vector number.
#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct InterruptVector(pub u32);

impl InterruptVector {
    /// Creates a new `InterruptVector` from a raw `u32`.
    #[inline]
    pub const fn from_raw(vector: u32) -> Self {
        Self(vector)
    }

    /// Converts this `InterruptVector` to its underlying `u32` value.
    #[inline]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

impl From<u32> for InterruptVector {
    #[inline]
    fn from(vector: u32) -> Self {
        Self(vector)
    }
}

impl From<InterruptVector> for u32 {
    #[inline]
    fn from(vector: InterruptVector) -> u32 {
        vector.0
    }
}

impl From<InterruptVector> for usize {
    #[inline]
    fn from(vector: InterruptVector) -> usize {
        vector.0 as usize
    }
}

impl core::ops::Deref for InterruptVector {
    type Target = u32;
    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct InterruptHandler {
    pub cookie: *mut core::ffi::c_void,
    pub r#fn: Option<extern "C" fn(*mut core::ffi::c_void) -> ()>,
}

zr::static_assert_size_and_align!(InterruptHandler, 16, 8);

impl InterruptHandler {
    pub const DEFAULT: Self = Self { cookie: core::ptr::null_mut(), r#fn: None };
    pub fn present(&self) -> bool {
        self.r#fn.is_some()
    }
    pub fn invoke(&self) {
        debug_assert!(self.present());
        (self.r#fn.unwrap())(self.cookie)
    }
}

// SAFETY: `InterruptHandler` represents an opaque C-ABI callback
// `(cookie, fn)`. Callers registering a handler guarantee that the callback
// and its cookie context are safe to transfer and invoke from interrupt
// context across any CPU. Synchronization of the storage slot itself is
// managed by the enclosing interrupt manager.
unsafe impl Send for InterruptHandler {}
unsafe impl Sync for InterruptHandler {}

/// The trigger mode of an interrupt.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptTriggerMode {
    /// Edge triggered interrupt.
    Edge = 0,
    /// Level triggered interrupt.
    Level = 1,
}

impl InterruptTriggerMode {
    /// Returns a string representation of the trigger mode.
    pub fn string(self) -> &'static str {
        match self {
            InterruptTriggerMode::Edge => "edge",
            InterruptTriggerMode::Level => "level",
        }
    }
}

/// The polarity of an interrupt.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptPolarity {
    /// Active high or rising edge polarity.
    High = 0,
    /// Active low or falling edge polarity.
    Low = 1,
}

impl InterruptPolarity {
    /// Returns a string representation of the polarity.
    pub fn string(self) -> &'static str {
        match self {
            InterruptPolarity::High => "high",
            InterruptPolarity::Low => "low",
        }
    }
}

/// A structure which holds the state of a block of IRQs allocated by the
/// platform to be used for delivering MSI or MSI-X interrupts.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MsiBlock {
    /// The target write transaction physical address.
    pub tgt_addr: u64,
    /// The data which the device should write when triggering an IRQ. Note,
    /// only the lower 16 bits are used when the block has been allocated for MSI
    /// instead of MSI-X.
    pub tgt_data: u32,
    /// The first IRQ id in the allocated block.
    pub base_irq_id: u32,
    /// The number of irqs in the allocated block.
    pub num_irq: u32,
    /// Whether or not this block has been allocated.
    pub allocated: bool,
    /// 32 bit if true, 64 bit otherwise.
    pub is_32bit: bool,
}

const _: () = assert!(core::mem::size_of::<InterruptVector>() == 4);
const _: () = assert!(core::mem::align_of::<InterruptVector>() == 4);

const _: () = assert!(core::mem::size_of::<InterruptTriggerMode>() == 4);
const _: () = assert!(core::mem::align_of::<InterruptTriggerMode>() == 4);

const _: () = assert!(core::mem::size_of::<InterruptPolarity>() == 4);
const _: () = assert!(core::mem::align_of::<InterruptPolarity>() == 4);

const _: () = assert!(core::mem::size_of::<MsiBlock>() == 24);
const _: () = assert!(core::mem::align_of::<MsiBlock>() == 8);
