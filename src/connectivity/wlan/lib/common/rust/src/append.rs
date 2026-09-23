// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::mem::size_of;
use std::ops::Deref;
use thiserror::Error;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout, Ref, Unaligned};

#[derive(Error, Debug)]
#[error("Buffer is too small for the written data")]
pub struct BufferTooSmall;

pub trait Append {
    fn append_bytes(&mut self, bytes: &[u8]) -> Result<(), BufferTooSmall>;

    fn append_bytes_zeroed(&mut self, len: usize) -> Result<&mut [u8], BufferTooSmall>;

    fn can_append(&self, bytes: usize) -> bool;

    fn append_value<T>(&mut self, value: &T) -> Result<(), BufferTooSmall>
    where
        T: IntoBytes + Immutable + ?Sized,
    {
        self.append_bytes(value.as_bytes())
    }

    fn append_byte(&mut self, byte: u8) -> Result<(), BufferTooSmall> {
        self.append_bytes(&[byte])
    }

    fn append_value_zeroed<T>(&mut self) -> Result<Ref<&mut [u8], T>, BufferTooSmall>
    where
        T: FromBytes + KnownLayout + Immutable + Unaligned,
    {
        let bytes = self.append_bytes_zeroed(size_of::<T>())?;
        Ok(Ref::from_bytes(bytes).unwrap())
    }

    fn append_array_zeroed<T>(
        &mut self,
        num_elems: usize,
    ) -> Result<Ref<&mut [u8], [T]>, BufferTooSmall>
    where
        T: FromBytes + Immutable + Unaligned,
    {
        let bytes = self.append_bytes_zeroed(size_of::<T>() * num_elems)?;
        Ok(Ref::from_bytes(bytes).unwrap())
    }
}

impl Append for Vec<u8> {
    fn append_bytes(&mut self, bytes: &[u8]) -> Result<(), BufferTooSmall> {
        self.extend_from_slice(bytes);
        Ok(())
    }

    fn append_bytes_zeroed(&mut self, len: usize) -> Result<&mut [u8], BufferTooSmall> {
        let old_len = self.len();
        self.resize(old_len + len, 0);
        Ok(&mut self[old_len..])
    }

    fn can_append(&self, _bytes: usize) -> bool {
        true
    }
}

pub trait TrackedAppend: Append {
    fn bytes_appended(&self) -> usize;
}

#[derive(Debug)]
pub struct VecCursor {
    written: usize,
    inner: Vec<u8>,
}

impl VecCursor {
    pub fn new() -> Self {
        Self { written: 0, inner: vec![] }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self { written: 0, inner: Vec::with_capacity(capacity) }
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    pub fn into_vec(self) -> Vec<u8> {
        self.inner
    }
}

impl From<VecCursor> for Vec<u8> {
    fn from(other: VecCursor) -> Self {
        other.inner
    }
}

impl Deref for VecCursor {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.inner[..]
    }
}

impl Append for VecCursor {
    fn append_bytes(&mut self, bytes: &[u8]) -> Result<(), BufferTooSmall> {
        self.inner.extend_from_slice(bytes);
        self.written += bytes.len();
        Ok(())
    }

    fn append_bytes_zeroed(&mut self, len: usize) -> Result<&mut [u8], BufferTooSmall> {
        let old_len = self.len();
        self.inner.resize(old_len + len, 0);
        self.written += len;
        Ok(&mut self.inner[old_len..])
    }

    fn can_append(&self, _bytes: usize) -> bool {
        true
    }
}

impl TrackedAppend for VecCursor {
    fn bytes_appended(&self) -> usize {
        self.written
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    pub fn append_to_vec() {
        let mut data = VecCursor::new();
        assert_eq!(0, data.bytes_appended());

        data.append_bytes(&[1, 2, 3]).unwrap();
        assert_eq!(3, data.bytes_appended());

        let bytes = data.append_bytes_zeroed(2).unwrap();
        bytes[0] = 4;
        assert_eq!(5, data.bytes_appended());

        data.append_value(&0x0706_u16).unwrap();
        assert_eq!(7, data.bytes_appended());

        data.append_byte(8).unwrap();
        assert_eq!(8, data.bytes_appended());

        let mut bytes = data.append_value_zeroed::<[u8; 2]>().unwrap();
        bytes[0] = 9;
        assert_eq!(10, data.bytes_appended());

        data.append_value(&[0x0c0b_u16, 0x0e0d_u16]).unwrap();
        assert_eq!(14, data.bytes_appended());

        let mut arr = data.append_array_zeroed::<[u8; 2]>(2).unwrap();
        arr[0] = [15, 16];
        assert_eq!(18, data.bytes_appended());

        #[rustfmt::skip]
        assert_eq!(
            &[
                1, 2, 3,
                4, 0,
                6, 7,
                8,
                9, 0,
                11, 12, 13, 14,
                15, 16, 0, 0
            ],
            &data[..]
        );
    }
}
