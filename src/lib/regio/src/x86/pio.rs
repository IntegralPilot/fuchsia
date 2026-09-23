// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use core::marker::PhantomData;

use crate::{Accessible, IoHandle, LayoutOver, Register};

/// A specialization of [`Register`] representing an x86 I/O port.
///
/// Example usage:
/// ```rust
/// use regio::RwSafe;
/// use regio::x86::Port;
///
/// const IO_PORT: Port<u8, u8, RwSafe> = Port::new(0x3f8);
/// ```
pub type Port<Layout, Base, Access> = Register<Layout, Access, PortIo<Base, Access>>;

impl<Layout, Base, Access> Port<Layout, Base, Access>
where
    Layout: LayoutOver<Base>,
    Base: Copy,
    Access: Accessible,
{
    /// Constructs a new port register directly from a port address.
    pub const fn new(port: u16) -> Self {
        // Safety: There is nothing unsound about PortIo construction.
        unsafe { Register::from_io(PortIo::new(port)) }
    }
}

/// An I/O backend for reading from and writing to x86 I/O ports.
#[derive(Debug)]
pub struct PortIo<Base, Access: Accessible> {
    port: u16,
    _marker: PhantomData<(Base, Access)>,
}

impl<Base, Access: Accessible> PortIo<Base, Access> {
    /// Constructs a new port I/O handle.
    pub const fn new(port: u16) -> Self {
        Self { port, _marker: PhantomData }
    }

    /// Returns the port address.
    pub const fn port(&self) -> u16 {
        self.port
    }
}

impl<Base, Access: Accessible> Clone for PortIo<Base, Access> {
    fn clone(&self) -> Self {
        Self { port: self.port, _marker: PhantomData }
    }
}

impl<Base, Access: Accessible> Copy for PortIo<Base, Access> {}

impl<Base: Copy, Access: Accessible> IoHandle for PortIo<Base, Access> {
    type Base = Base;
}

#[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
mod x86_only {
    use core::arch::asm;

    use super::*;
    use crate::{ReadHandle, Readable, Writable, WriteHandle};

    macro_rules! impl_port_io_handle_for_small_base {
        ($ty:ty, $reg:tt) => {
            impl<Access: Readable> ReadHandle for PortIo<$ty, Access> {
                #[inline]
                unsafe fn read_raw(&self) -> $ty {
                    let value: $ty;
                    unsafe {
                        asm!(
                            concat!("in ", $reg, ", dx"),
                            in("dx") self.port,
                            out($reg) value,
                            options(nostack, preserves_flags),
                        );
                    }
                    value
                }
            }

            impl<Access: Writable> WriteHandle for PortIo<$ty, Access> {
                #[inline]
                unsafe fn write_raw(&self, value: $ty) {
                    unsafe {
                        asm!(
                            concat!("out dx, ", $reg),
                            in("dx") self.port,
                            in($reg) value,
                            options(nostack, preserves_flags),
                        );
                    }
                }
            }
        };
    }

    impl_port_io_handle_for_small_base!(u8, "al");
    impl_port_io_handle_for_small_base!(u16, "ax");
    impl_port_io_handle_for_small_base!(u32, "eax");

    impl<Access: Readable> ReadHandle for PortIo<u64, Access> {
        #[inline]
        unsafe fn read_raw(&self) -> u64 {
            let low = unsafe { PortIo::<u32, Access>::new(self.port).read_raw() };
            let high = unsafe { PortIo::<u32, Access>::new(self.port + 1).read_raw() };
            u64::from(high) << 32 | u64::from(low)
        }
    }

    impl<Access: Writable> WriteHandle for PortIo<u64, Access> {
        #[inline]
        unsafe fn write_raw(&self, value: u64) {
            let low = value as u32;
            let high = (value >> 32) as u32;
            unsafe {
                PortIo::<u32, Access>::new(self.port).write_raw(low);
                PortIo::<u32, Access>::new(self.port + 1).write_raw(high);
            }
        }
    }
}
