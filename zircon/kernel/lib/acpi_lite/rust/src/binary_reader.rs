// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use crate::structures::VariableSized;

// Light-weight class for decoding structs in a safe manner.
//
// Each operation returns a pointer to a valid struct or None indicating
// that the read would return an invalid structure, such as a structure out of
// bounds of the original input buffer.
//
// BinaryReader supports a common requirement in ACPI of variable-length
// structures, where a struct consists of a header followed by a payload.
// To support such structures, we require a |size| method returning the
// size of the header + payload.
//
// Successful reads consume bytes from the buffer, while failed reads don't
// modify internal state.
pub struct BinaryReader<'a> {
    buffer: &'a [u8],
}

impl<'a> BinaryReader<'a> {
    // Construct a BinaryReader from the given slice.
    pub fn new(buffer: &'a [u8]) -> Self {
        Self { buffer }
    }

    /// # Safety
    /// The caller must ensure that `ptr` points to a valid block of memory
    /// of at least `size` bytes, and that the memory remains valid for the
    /// lifetime `'a`.
    pub unsafe fn from_ptr(ptr: *const u8, size: usize) -> Self {
        // SAFETY: The caller guarantees `ptr` and `size` are valid.
        Self { buffer: unsafe { core::slice::from_raw_parts(ptr, size) } }
    }

    // Construct a BinaryReader from a valid structure with a size() method.
    pub fn from_variable_sized<T>(header: &'a T) -> Self
    where
        T: VariableSized,
    {
        let size = header.size();
        let ptr = header as *const T as *const u8;
        // SAFETY: `header` is a valid reference, and `T::size()` is assumed
        // to return the size of the memory backing it.
        unsafe { Self::from_ptr(ptr, size) }
    }

    // Construct a BinaryReader from a class with a size() method, skipping the header T.
    pub fn from_payload_of_struct<T>(header: &'a T) -> Self
    where
        T: VariableSized,
    {
        let size = header.size();
        let struct_size = core::mem::size_of::<T>();
        if size < struct_size {
            return Self { buffer: &[] };
        }
        let ptr = header as *const T as *const u8;
        // SAFETY: `header` is a valid reference, and `T::size()` is assumed
        // to return the size of the memory backing it. We offset by `struct_size`
        // which is safe because `size >= struct_size`.
        unsafe { Self::from_ptr(ptr.add(struct_size), size - struct_size) }
    }

    // Read a fixed-length structure.
    pub fn read_fixed_length<T>(&mut self) -> Option<&'a T>
    where
        T: zerocopy::FromBytes + zerocopy::Unaligned + zerocopy::Immutable + zerocopy::KnownLayout,
    {
        let size = core::mem::size_of::<T>();
        if self.buffer.len() < size {
            return None;
        }
        let (prefix, suffix) = self.buffer.split_at(size);
        let r = zerocopy::Ref::<_, T>::from_bytes(prefix).ok()?;
        self.buffer = suffix;
        Some(zerocopy::Ref::into_ref(r))
    }

    // Read a variable length structure, where the size is determined by T::size().
    pub fn read<T>(&mut self) -> Option<&'a T>
    where
        T: VariableSized
            + zerocopy::FromBytes
            + zerocopy::Unaligned
            + zerocopy::Immutable
            + zerocopy::KnownLayout,
    {
        let size = core::mem::size_of::<T>();
        if self.buffer.len() < size {
            return None;
        }
        let prefix = &self.buffer[..size];
        let r = zerocopy::Ref::<_, T>::from_bytes(prefix).ok()?;
        let val = zerocopy::Ref::into_ref(r);
        let desired_size = val.size();
        if desired_size < size || desired_size > self.buffer.len() {
            return None;
        }
        self.buffer = &self.buffer[desired_size..];
        Some(val)
    }

    // Discard the given number of bytes.
    //
    // Return true if the bytes could be discarded, or false if there are insufficient bytes.
    pub fn skip_bytes(&mut self, bytes: usize) -> bool {
        if self.buffer.len() < bytes {
            return false;
        }
        self.buffer = &self.buffer[bytes..];
        true
    }

    // Return true if all the bytes of the reader have been consumed.
    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

// Convert a pointer to type |Src| to a pointer of type |Dest|, ensuring that the size of |Src|
// is valid.
//
// We require that the type |Dest| has a field |header| at offset 0 of type |Src|.
pub trait DowncastFrom<Src> {
    /// # Safety
    ///
    /// The caller must ensure that `src` actually points to a structure of type `Self`.
    unsafe fn downcast_from(src: &Src) -> Option<&Self>;
}

#[derive(Copy, Clone, zerocopy::FromBytes, zerocopy::Immutable, zerocopy::KnownLayout)]
#[repr(C, packed)]
// A "packed" type wraps a plain type, but instructs the compiler to treat it as unaligned data.
pub struct Unaligned<T>(pub T);

unsafe impl<T> zerocopy::Unaligned for Unaligned<T> {
    fn only_derive_is_allowed_to_implement_this_trait() {}
}

macro_rules! impl_downcast_from {
    ($Src:ty => $($Target:ty),+ $(,)?) => {
        $(
            impl $crate::binary_reader::DowncastFrom<$Src> for $Target {
                unsafe fn downcast_from(src: &$Src) -> Option<&Self> {
                    if $crate::structures::VariableSized::size(src) < core::mem::size_of::<Self>() {
                        return None;
                    }
                    // SAFETY: The caller must ensure that `src` points to a block of memory
                    // of at least `src.size()` bytes.
                    let bytes = unsafe {
                        core::slice::from_raw_parts(
                            src as *const $Src as *const u8,
                            $crate::structures::VariableSized::size(src),
                        )
                    };
                    let bytes = &bytes[..core::mem::size_of::<Self>()];
                    let r = zerocopy::Ref::<_, Self>::from_bytes(bytes).ok()?;
                    Some(zerocopy::Ref::into_ref(r))
                }
            }
        )+
    };
}
