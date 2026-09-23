// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Register access abstracted as traits.
//!
//! Routines involving [`Register`] instances can be written generically against
//! these traits to aid testability. See [`testing`](crate::testing) for
//! utilities also implementing these traits that may be supplied instead in
//! tests.

use super::{
    IoHandle, LayoutOver, ReadHandle, Readable, Register, SafeWrite, UnsafeWrite, Writable,
    WriteHandle,
};

/// A trait representing a readable register.
pub trait ReadReg<Layout> {
    fn read(&self) -> Layout;
}

/// A trait representing a safe-writable register.
pub trait SafeWriteReg<Layout> {
    fn write(&self, value: Layout);

    fn modify<ModifyFn, Ret>(&self, cb: ModifyFn) -> Ret
    where
        ModifyFn: FnOnce(&mut Layout) -> Ret,
        Self: ReadReg<Layout>,
    {
        let mut value = self.read();
        let ret = cb(&mut value);
        self.write(value);
        ret
    }
}

/// A trait representing an unsafe-writable register.
pub trait UnsafeWriteReg<Layout> {
    /// # Safety
    ///
    /// The caller must guarantee that the write does not result in
    /// undefined behaviour.
    unsafe fn write(&self, value: Layout);

    /// # Safety
    ///
    /// The caller must guarantee that the write does not result in
    /// undefined behaviour.
    unsafe fn modify<ModifyFn, Ret>(&self, cb: ModifyFn) -> Ret
    where
        ModifyFn: FnOnce(&mut Layout) -> Ret,
        Self: ReadReg<Layout>,
    {
        let mut value = self.read();
        let ret = cb(&mut value);
        // Safety: Justification deferred to the caller.
        unsafe { self.write(value) };
        ret
    }
}

/// A trait representing a register that is both readable and safe-writable. It
/// is automatically implemented for any type that implements both
/// [`ReadReg`] and [`SafeWriteReg`].
pub trait RwSafeReg<Layout>: ReadReg<Layout> + SafeWriteReg<Layout> {}
impl<Layout, R> RwSafeReg<Layout> for R where R: ReadReg<Layout> + SafeWriteReg<Layout> {}

/// A trait representing a register that is both readable and unsafe-writable.
/// It is automatically implemented for any type that implements both
/// [`ReadReg`] and [`UnsafeWriteReg`].
pub trait RwUnsafeReg<Layout>: ReadReg<Layout> + UnsafeWriteReg<Layout> {}
impl<Layout, R> RwUnsafeReg<Layout> for R where R: ReadReg<Layout> + UnsafeWriteReg<Layout> {}

impl<Layout, R: ReadReg<Layout>> ReadReg<Layout> for &R {
    fn read(&self) -> Layout {
        (*self).read()
    }
}

impl<Layout, R: UnsafeWriteReg<Layout>> UnsafeWriteReg<Layout> for &R {
    unsafe fn write(&self, value: Layout) {
        unsafe { (*self).write(value) }
    }
}

impl<Layout, R: SafeWriteReg<Layout>> SafeWriteReg<Layout> for &R {
    fn write(&self, value: Layout) {
        (*self).write(value)
    }
}

//
// Register of course implements the above traits.
//

impl<Layout, Access, Io> ReadReg<Layout> for Register<Layout, Access, Io>
where
    Layout: LayoutOver<<Io as IoHandle>::Base>,
    Access: Readable,
    Io: ReadHandle,
{
    fn read(&self) -> Layout {
        Register::read(self)
    }
}

impl<Layout, R, Io> SafeWriteReg<Layout> for Register<Layout, (R, SafeWrite), Io>
where
    Layout: LayoutOver<<Io as IoHandle>::Base>,
    (R, SafeWrite): Writable,
    Io: WriteHandle,
{
    fn write(&self, value: Layout) {
        Register::<_, (R, SafeWrite), _>::write(self, value);
    }
}

impl<Layout, R, Io> UnsafeWriteReg<Layout> for Register<Layout, (R, UnsafeWrite), Io>
where
    Layout: LayoutOver<<Io as IoHandle>::Base>,
    (R, UnsafeWrite): Writable,
    Io: WriteHandle,
{
    unsafe fn write(&self, value: Layout) {
        unsafe { Register::<_, (R, UnsafeWrite), _>::write(self, value) };
    }
}
