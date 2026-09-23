// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

pub trait BlockContainer {
    type Data;
    type ShareableData;

    /// Returns the size of the container.
    fn len(&self) -> usize;

    /// Returns whether the container is empty or not.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Trait implemented by an Inspect container that can be read from.
pub trait ReadBytes: BlockContainer {
    /// Returns a slice of the given size at the given offset if one exists of the exact size.
    fn get_slice_at(&self, offset: usize, size: usize) -> Option<&[u8]>;

    /// Returns a slice of the given size at the beginning of the container if one exists of the
    /// exact size.
    #[inline]
    fn get_slice(&self, size: usize) -> Option<&[u8]> {
        self.get_slice_at(0, size)
    }

    /// Returns the value at the given offset, if one exists.
    #[inline]
    fn get_value<T: ContainerValue>(&self, offset: usize) -> Option<T> {
        self.get_slice_at(offset, std::mem::size_of::<T>())
            .and_then(|slice| T::read_from_bytes(slice).ok())
    }
}

pub trait CopyBytes: BlockContainer {
    fn copy_bytes_at(&self, offset: usize, dst: &mut [u8]);

    fn copy_bytes(&self, dst: &mut [u8]) {
        self.copy_bytes_at(0, dst)
    }
}

/// Trait implemented by primitive types that can be read from and written to an Inspect container.
pub trait ContainerValue:
    FromBytes + IntoBytes + KnownLayout + Immutable + Copy + private::Sealed + 'static
{
}

mod private {
    pub trait Sealed {}
}

macro_rules! impl_container_value {
    ($($type:ty),*) => {
        $(
            impl private::Sealed for $type {}
            impl ContainerValue for $type {}
        )*
    };
}

impl_container_value!(u8, u16, u32, u64, i64, f64);

/// Trait implemented by container to which bytes can be written.
pub trait WriteBytes {
    /// Returns an exclusive reference to a slice of the given size at the given offset if one
    /// exists of the exact size.
    fn get_slice_mut_at(&mut self, offset: usize, size: usize) -> Option<&mut [u8]>;

    /// Returns an exclusive reference to a slice of the given size at the beginning of the
    /// container if one exists of the exact size.
    #[inline]
    fn get_slice_mut(&mut self, size: usize) -> Option<&mut [u8]> {
        self.get_slice_mut_at(0, size)
    }

    #[inline]
    fn copy_from_slice_at(&mut self, offset: usize, bytes: &[u8]) {
        // TODO: error
        if let Some(slice) = self.get_slice_mut_at(offset, bytes.len()) {
            slice.copy_from_slice(bytes);
        }
    }

    #[inline]
    fn copy_from_slice(&mut self, bytes: &[u8]) {
        self.copy_from_slice_at(0, bytes);
    }

    /// Sets the value at the given offset, if in bounds.
    #[inline]
    fn set_value<T: ContainerValue>(
        &mut self,
        offset: usize,
        value: T,
    ) -> Result<(), crate::Error> {
        if let Some(slice) = self.get_slice_mut_at(offset, std::mem::size_of::<T>()) {
            slice.copy_from_slice(value.as_bytes());
            Ok(())
        } else {
            Err(crate::Error::InvalidOffset(offset))
        }
    }
}

impl BlockContainer for Vec<u8> {
    type Data = Self;
    type ShareableData = ();

    /// The number of bytes in the buffer.
    #[inline]
    fn len(&self) -> usize {
        self.as_slice().len()
    }
}

impl ReadBytes for Vec<u8> {
    #[inline]
    fn get_slice_at(&self, offset: usize, size: usize) -> Option<&[u8]> {
        self.as_slice().get_slice_at(offset, size)
    }
}

impl CopyBytes for Vec<u8> {
    #[inline]
    fn copy_bytes_at(&self, offset: usize, dst: &mut [u8]) {
        if let Some(slice) = self.as_slice().get_slice_at(offset, dst.len()) {
            dst.copy_from_slice(slice);
        }
    }
}

impl BlockContainer for [u8] {
    type Data = ();
    type ShareableData = ();

    #[inline]
    fn len(&self) -> usize {
        <[u8]>::len(self)
    }
}

impl ReadBytes for [u8] {
    #[inline]
    fn get_slice_at(&self, offset: usize, size: usize) -> Option<&[u8]> {
        let upper_bound = offset.checked_add(size)?;
        if offset >= self.len() || upper_bound > self.len() {
            return None;
        }
        Some(&self[offset..upper_bound])
    }
}

impl<const N: usize> BlockContainer for [u8; N] {
    type Data = Self;
    type ShareableData = ();

    /// The number of bytes in the buffer.
    #[inline]
    fn len(&self) -> usize {
        self.as_slice().len()
    }
}

impl<const N: usize> ReadBytes for [u8; N] {
    #[inline]
    fn get_slice_at(&self, offset: usize, size: usize) -> Option<&[u8]> {
        self.as_slice().get_slice_at(offset, size)
    }
}

/// Trait implemented by an Inspect container that can be written to.
impl<const N: usize> WriteBytes for [u8; N] {
    #[inline]
    fn get_slice_mut_at(&mut self, offset: usize, size: usize) -> Option<&mut [u8]> {
        if offset >= self.len() {
            return None;
        }
        let upper_bound = offset.checked_add(size)?;
        if upper_bound > self.len() {
            return None;
        }
        Some(&mut self[offset..upper_bound])
    }
}
