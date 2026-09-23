// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! This module provides abstractions over the fatfs Dir and File types,
//! erasing their lifetimes and allowing them to be kept.
use crate::directory::FatDirectory;
use crate::filesystem::FatFilesystem;
use crate::node::Node;
use crate::types::{Dir, File};
use scopeguard::defer;
use std::cell::{Cell, Ref, RefCell, RefMut};
use std::rc::Rc;
use std::sync::Arc;
use zx::Status;

mod opaque {
    #[repr(transparent)]
    pub struct Opaque<T>(T);
}
pub use opaque::Opaque;

/// Trait for types that can have their lifetime faked to `'static` and restored.
///
/// # Safety
///
/// Implementer must ensure that `Self` and `Self::Target` have the same memory layout
/// and size. Typically this is only implemented for `T<'static>` with `Target = T<'a>`.
pub unsafe trait ErasedLifetime<'a>: Sized {
    type Target: 'a;

    /// Erases the lifetime by transmute_copying the value.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the returned value is not used after the lifetime
    /// of the original value has expired.
    unsafe fn erase(val: Self::Target) -> Opaque<Self> {
        // SAFETY: Transmute is safe because the trait guarantees that Self and Self::Target
        // have the same memory layout.
        unsafe {
            let res = std::mem::transmute_copy(&val);
            std::mem::forget(val);
            res
        }
    }

    /// Restores the lifetime by transmute_copying the value.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the returned value is bound to the correct lifetime.
    unsafe fn restore(val: Opaque<Self>) -> Self::Target {
        // SAFETY: Transmute is safe because the trait guarantees that Self and Self::Target
        // have the same memory layout.
        unsafe {
            let res = std::mem::transmute_copy(&val);
            std::mem::forget(val);
            res
        }
    }

    /// Restores the reference lifetime by casting the pointer.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the returned reference is bound to the correct lifetime.
    unsafe fn restore_ref(val: &Opaque<Self>) -> &Self::Target {
        // SAFETY: Casting pointer is safe because the trait guarantees that Self and Self::Target
        // have the same memory layout.
        unsafe { &*(val as *const Opaque<Self> as *const Self::Target) }
    }

    /// Restores the mutable reference lifetime by casting the pointer.
    ///
    /// # Safety
    ///
    /// The caller must ensure that the returned reference is bound to the correct lifetime.
    unsafe fn restore_mut(val: &mut Opaque<Self>) -> &mut Self::Target {
        // SAFETY: Casting pointer is safe because the trait guarantees that Self and Self::Target
        // have the same memory layout.
        unsafe { &mut *(val as *mut Opaque<Self> as *mut Self::Target) }
    }
}

// SAFETY: Dir<'static> and Dir<'a> have the same memory layout and size.
unsafe impl<'a> ErasedLifetime<'a> for Dir<'static> {
    type Target = Dir<'a>;
}

// SAFETY: File<'static> and File<'a> have the same memory layout and size.
unsafe impl<'a> ErasedLifetime<'a> for File<'static> {
    type Target = File<'a>;
}

pub struct FsRef<T> {
    inner: RefCell<Option<Opaque<T>>>,
    open_count: Cell<usize>,
    filesystem: Rc<FatFilesystem>,
}

impl<T> FsRef<T> {
    /// Creates an `FsRef` from a value with erased lifetime.
    ///
    /// # Safety
    ///
    /// The caller must ensure that `val` is a reference associated with the provided `filesystem`.
    /// Mixing references from different filesystem instances will result in undefined behavior.
    pub unsafe fn from<'a>(
        val: <T as ErasedLifetime<'a>>::Target,
        filesystem: Rc<FatFilesystem>,
    ) -> Self
    where
        T: ErasedLifetime<'a>,
    {
        FsRef {
            // SAFETY: Safe because T::Target (Dir<'a>/File<'a>) is bound to the lifetime of the
            // filesystem, which is kept alive by `self.filesystem`.
            inner: RefCell::new(Some(unsafe { T::erase(val) })),
            open_count: Cell::new(1),
            filesystem,
        }
    }

    pub fn empty(filesystem: Rc<FatFilesystem>) -> Self {
        FsRef { inner: RefCell::new(None), open_count: Cell::new(0), filesystem }
    }

    pub fn filesystem(&self) -> &Rc<FatFilesystem> {
        &self.filesystem
    }

    pub fn close<'a>(&self)
    where
        T: ErasedLifetime<'a>,
    {
        let open_count = self.open_count.get();
        assert!(open_count > 0);
        self.open_count.set(open_count - 1);
        if open_count == 1 {
            self.clear();
        }
    }

    pub fn clear<'a>(&self)
    where
        T: ErasedLifetime<'a>,
    {
        if let Some(f) = self.inner.borrow_mut().take() {
            // SAFETY: Safe because the restored value is dropped immediately while `self`
            // (and thus `self.filesystem`) is alive.
            unsafe {
                let restored = <T as ErasedLifetime<'a>>::restore(f);
                drop(restored);
            }
        }
    }

    pub fn get<'a>(&'a self) -> Option<Ref<'a, <T as ErasedLifetime<'a>>::Target>>
    where
        T: ErasedLifetime<'a>,
    {
        let borrow = self.inner.borrow();
        if borrow.is_none() {
            None
        } else {
            Some(Ref::map(borrow, |val| {
                // SAFETY: Safe because the returned reference is bound to the lifetime of
                // self.inner borrow by Ref::map.
                unsafe { T::restore_ref(val.as_ref().unwrap()) }
            }))
        }
    }

    pub fn get_mut<'a>(&'a self) -> Option<RefMut<'a, <T as ErasedLifetime<'a>>::Target>>
    where
        T: ErasedLifetime<'a>,
    {
        let borrow = self.inner.borrow_mut();
        if borrow.is_none() {
            None
        } else {
            Some(RefMut::map(borrow, |val| {
                // SAFETY: Safe because the returned reference is bound to the lifetime of
                // self.inner borrow by RefMut::map.
                unsafe { T::restore_mut(val.as_mut().unwrap()) }
            }))
        }
    }
}

impl<T> Drop for FsRef<T> {
    fn drop(&mut self) {
        assert_eq!(self.open_count.get(), 0);
        assert!(self.inner.borrow().is_none());
    }
}

impl FsRef<Dir<'static>> {
    pub fn maybe_reopen(
        &self,
        parent: Option<&Arc<FatDirectory>>,
        name: &str,
    ) -> Result<(), Status> {
        if self.open_count.get() == 0 { Ok(()) } else { self.reopen(parent, name) }
    }

    fn reopen(&self, parent: Option<&Arc<FatDirectory>>, name: &str) -> Result<(), Status> {
        let dir = if let Some(parent) = parent {
            parent.open_ref()?;
            defer! { parent.close_ref() }
            parent.find_child(name)?.ok_or(Status::NOT_FOUND)?.to_dir()
        } else {
            self.filesystem.fatfs_root_dir()
        };
        // SAFETY: We hold `self.filesystem` which is Rc<FatFilesystem>, ensuring the
        // filesystem outlives this FsRef.
        self.inner.replace(Some(unsafe { <Dir<'static> as ErasedLifetime>::erase(dir) }));
        Ok(())
    }

    pub fn open(&self, parent: Option<&Arc<FatDirectory>>, name: &str) -> Result<(), Status> {
        let open_count = self.open_count.get();
        if open_count == usize::MAX {
            Err(Status::UNAVAILABLE)
        } else {
            if open_count == 0 {
                self.reopen(parent, name)?;
            }
            self.open_count.set(open_count + 1);
            Ok(())
        }
    }
}

impl FsRef<File<'static>> {
    pub fn maybe_reopen(&self, parent: &FatDirectory, name: &str) -> Result<(), Status> {
        if self.open_count.get() == 0 { Ok(()) } else { self.reopen(parent, name) }
    }

    fn reopen(&self, parent: &FatDirectory, name: &str) -> Result<(), Status> {
        let file = parent.find_child(name)?.ok_or(Status::NOT_FOUND)?.to_file();
        // SAFETY: We hold `self.filesystem` which is Rc<FatFilesystem>, ensuring the
        // filesystem outlives this FsRef.
        self.inner.replace(Some(unsafe { <File<'static> as ErasedLifetime>::erase(file) }));
        Ok(())
    }

    pub fn open(&self, parent: Option<&FatDirectory>, name: &str) -> Result<(), Status> {
        let open_count = self.open_count.get();
        if open_count == usize::MAX {
            Err(Status::UNAVAILABLE)
        } else {
            if open_count == 0 {
                self.reopen(parent.ok_or(Status::BAD_HANDLE)?, name)?;
            }
            self.open_count.set(open_count + 1);
            Ok(())
        }
    }
}

pub type FatfsDirRef = FsRef<Dir<'static>>;
pub type FatfsFileRef = FsRef<File<'static>>;
