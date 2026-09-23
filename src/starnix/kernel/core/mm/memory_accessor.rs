// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::mm::PAGE_SIZE;
use crate::vfs::{FsStr, FsString};
use smallvec::SmallVec;
use starnix_types::user_buffer::{UserBuffer, UserBuffers, UserBuffers32};
use starnix_uapi::errors::Errno;
use starnix_uapi::user_address::{
    ArchSpecific, MappingMultiArchUserRef, MultiArchUserRef, UserAddress, UserAddress32,
    UserCString, UserRef,
};
use starnix_uapi::user_value::UserValue;
use starnix_uapi::{PATH_MAX, UIO_MAXIOV, errno, error, uapi};
use std::ffi::CStr;
use std::mem::MaybeUninit;
use usercopy::slice_to_maybe_uninit_mut;
use zerocopy::{FromBytes, Immutable, IntoBytes};

pub type IOVecPtr = MultiArchUserRef<uapi::iovec, uapi::arch32::iovec>;

pub trait MemoryAccessor {
    /// Reads exactly `bytes.len()` bytes of memory from `addr` into `bytes`.
    ///
    /// In case of success, the number of bytes read will always be `bytes.len()`.
    ///
    /// Consider using `MemoryAccessorExt::read_memory_to_*` methods if you do not require control
    /// over the allocation.
    fn read_memory<'a>(
        &self,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno>;

    /// Reads bytes starting at `addr`, continuing until either a null byte is read, `bytes.len()`
    /// bytes have been read or no more bytes can be read from the target.
    ///
    /// This is used, for example, to read null-terminated strings where the exact length is not
    /// known, only the maximum length is.
    ///
    /// Returns the bytes that have been read to on success.
    fn read_memory_partial_until_null_byte<'a>(
        &self,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno>;

    /// Reads bytes starting at `addr`, continuing until either `bytes.len()` bytes have been read
    /// or no more bytes can be read from the target.
    ///
    /// This is used, for example, to read null-terminated strings where the exact length is not
    /// known, only the maximum length is.
    ///
    /// Consider using `MemoryAccessorExt::read_memory_partial_to_*` methods if you do not require
    /// control over the allocation.
    fn read_memory_partial<'a>(
        &self,
        addr: UserAddress,
        bytes: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a mut [u8], Errno>;

    /// Writes the provided bytes to `addr`.
    ///
    /// In case of success, the number of bytes written will always be `bytes.len()`.
    ///
    /// # Parameters
    /// - `addr`: The address to write to.
    /// - `bytes`: The bytes to write from.
    fn write_memory(&self, addr: UserAddress, bytes: &[u8]) -> Result<usize, Errno>;

    /// Writes bytes starting at `addr`, continuing until either `bytes.len()` bytes have been
    /// written or no more bytes can be written.
    ///
    /// # Parameters
    /// - `addr`: The address to write to.
    /// - `bytes`: The bytes to write from.
    fn write_memory_partial(&self, addr: UserAddress, bytes: &[u8]) -> Result<usize, Errno>;

    /// Writes zeros starting at `addr` and continuing for `length` bytes.
    ///
    /// Returns the number of bytes that were zeroed.
    fn zero(&self, addr: UserAddress, length: usize) -> Result<usize, Errno>;
}

pub trait TaskMemoryAccessor: MemoryAccessor {
    /// Returns the maximum valid address for this memory accessor.
    fn maximum_valid_address(&self) -> Option<UserAddress>;
}

// TODO(https://fxbug.dev/42079727): replace this with MaybeUninit::as_bytes_mut.
#[inline]
fn object_as_mut_bytes<T: FromBytes + Sized>(
    object: &mut MaybeUninit<T>,
) -> &mut [MaybeUninit<u8>] {
    // SAFETY: T is FromBytes, which means that any bit pattern is valid. Interpreting
    // MaybeUninit<T> as [MaybeUninit<u8>] is safe because T's alignment requirements
    // are larger than u8.
    unsafe {
        std::slice::from_raw_parts_mut(
            object.as_mut_ptr() as *mut MaybeUninit<u8>,
            std::mem::size_of::<T>(),
        )
    }
}

// TODO(https://fxbug.dev/42079727): replace this with MaybeUninit::slice_as_bytes_mut.
#[inline]
fn slice_as_mut_bytes<T: FromBytes + Sized>(
    slice: &mut [MaybeUninit<T>],
) -> &mut [MaybeUninit<u8>] {
    // SAFETY: T is FromBytes, which means that any bit pattern is valid. Interpreting T as u8
    // is safe because T's alignment requirements are larger than u8.
    unsafe {
        std::slice::from_raw_parts_mut(
            slice.as_mut_ptr() as *mut MaybeUninit<u8>,
            slice.len() * std::mem::size_of::<T>(),
        )
    }
}

/// Holds the number of _elements_ read by the callback to [`read_to_vec`].
///
/// Used to make it clear to callers that the callback should return the number
/// of elements read and not the number of bytes read.
pub struct NumberOfElementsRead(pub usize);

/// Performs a read into a `Vec` using the provided read function.
///
/// The read function returns the number of elements of type `T` read.
///
/// # Safety
///
/// The read function must only return `Ok(n)` if at least one element was read and `n` holds
/// the number of elements of type `T` read starting from the beginning of the slice.
#[inline]
pub unsafe fn read_to_vec<T: FromBytes, E>(
    max_len: usize,
    read_fn: impl FnOnce(&mut [MaybeUninit<T>]) -> Result<NumberOfElementsRead, E>,
) -> Result<Vec<T>, E> {
    let mut buffer = Vec::with_capacity(max_len);
    // We can't just pass `spare_capacity_mut` because `Vec::with_capacity`
    // returns a `Vec` with _at least_ the requested capacity.
    let NumberOfElementsRead(read_elements) = read_fn(&mut buffer.spare_capacity_mut()[..max_len])?;
    debug_assert!(read_elements <= max_len, "read_elements={read_elements}, max_len={max_len}");
    // SAFETY: The new length is equal to the number of elements successfully
    // initialized (since `read_fn` returned successfully).
    unsafe { buffer.set_len(read_elements) }
    Ok(buffer)
}

/// Performs a read into an array using the provided read function.
///
//
/// The read function returns `Ok(())` if the buffer was fully read to.
///
/// # Safety
///
/// The read function must only return `Ok(())` if all the bytes were read to.
#[inline]
pub unsafe fn read_to_array<T: FromBytes, E, const N: usize>(
    read_fn: impl FnOnce(&mut [MaybeUninit<T>]) -> Result<(), E>,
) -> Result<[T; N], E> {
    // TODO(https://fxbug.dev/129314): replace with MaybeUninit::uninit_array.
    let buffer: MaybeUninit<[MaybeUninit<T>; N]> = MaybeUninit::uninit();
    // SAFETY: We are converting from an uninitialized array to an array
    // of uninitialized elements which is the same. See
    // https://doc.rust-lang.org/std/mem/union.MaybeUninit.html#initializing-an-array-element-by-element.
    let mut buffer = unsafe { buffer.assume_init() };
    read_fn(&mut buffer)?;
    // SAFETY: This is safe because we have initialized all the elements in
    // the array (since `read_fn` returned successfully).
    //
    // TODO(https://fxbug.deb/129309): replace with MaybeUninit::array_assume_init.
    let buffer = buffer.map(|a| unsafe { a.assume_init() });
    Ok(buffer)
}

/// Performs a read into an object using the provided read function.
///
/// The read function returns `Ok(())` if the buffer was fully read to.
///
/// # Safety
///
/// The read function must only return `Ok(())` if all the bytes were read to.
#[inline]
pub unsafe fn read_to_object_as_bytes<T: FromBytes, E>(
    read_fn: impl FnOnce(&mut [MaybeUninit<u8>]) -> Result<(), E>,
) -> Result<T, E> {
    let mut object = MaybeUninit::uninit();
    read_fn(object_as_mut_bytes(&mut object))?;
    // SAFETY: The call to `read_fn` succeeded so we know that `object`
    // has been initialized.
    let object = unsafe { object.assume_init() };
    Ok(object)
}

pub trait MemoryAccessorExt: MemoryAccessor {
    /// Reads exactly `bytes.len()` bytes of memory from `addr` into `bytes`.
    ///
    /// In case of success, the number of bytes read will always be `bytes.len()`.
    ///
    /// Consider using `MemoryAccessorExt::read_memory_to_*` methods if you do not require control
    /// over the allocation.
    fn read_memory_to_slice(&self, addr: UserAddress, bytes: &mut [u8]) -> Result<(), Errno> {
        let bytes_len = bytes.len();
        self.read_memory(addr, slice_to_maybe_uninit_mut(bytes))
            .map(|bytes_read| debug_assert_eq!(bytes_read.len(), bytes_len))
    }

    /// Read exactly `len` bytes of memory, returning them as a a Vec.
    fn read_memory_to_vec(&self, addr: UserAddress, len: usize) -> Result<Vec<u8>, Errno> {
        // SAFETY: `self.read_memory` only returns `Ok` if all bytes were read to.
        unsafe {
            read_to_vec::<u8, _>(len, |buf| {
                self.read_memory(addr, buf).map(|bytes_read| {
                    debug_assert_eq!(bytes_read.len(), len);
                    NumberOfElementsRead(len)
                })
            })
        }
    }

    /// Read up to `max_len` bytes from `addr`, returning them as a Vec.
    fn read_memory_partial_to_vec(
        &self,
        addr: UserAddress,
        max_len: usize,
    ) -> Result<Vec<u8>, Errno> {
        // SAFETY: `self.read_memory_partial` returns the bytes read.
        unsafe {
            read_to_vec::<u8, _>(max_len, |buf| {
                self.read_memory_partial(addr, buf)
                    .map(|bytes_read| NumberOfElementsRead(bytes_read.len()))
            })
        }
    }

    /// Read exactly `N` bytes from `addr`, returning them as an array.
    fn read_memory_to_array<const N: usize>(&self, addr: UserAddress) -> Result<[u8; N], Errno> {
        // SAFETY: `self.read_memory` only returns `Ok` if all bytes were read to.
        unsafe {
            read_to_array(|buf| {
                self.read_memory(addr, buf).map(|bytes_read| debug_assert_eq!(bytes_read.len(), N))
            })
        }
    }

    /// Read the contents of `buffer`, returning them as a Vec.
    fn read_buffer(&self, buffer: &UserBuffer) -> Result<Vec<u8>, Errno> {
        self.read_memory_to_vec(buffer.address, buffer.length)
    }

    /// Read an instance of T from `user`.
    fn read_object<T: FromBytes>(&self, user: UserRef<T>) -> Result<T, Errno> {
        // SAFETY: `self.read_memory` only returns `Ok` if all bytes were read to.
        unsafe {
            read_to_object_as_bytes(|buf| {
                self.read_memory(user.addr(), buf)
                    .map(|bytes_read| debug_assert_eq!(bytes_read.len(), std::mem::size_of::<T>()))
            })
        }
    }

    fn read_multi_arch_ptr<T64, T32>(
        &self,
        user: MultiArchUserRef<MultiArchUserRef<T64, T32>, MultiArchUserRef<T64, T32>>,
    ) -> Result<MultiArchUserRef<T64, T32>, Errno> {
        let address = if user.is_arch32() {
            self.read_object::<UserAddress32>(user.addr().into())?.into()
        } else {
            self.read_object::<UserAddress>(user.addr().into())?
        };
        Ok(MultiArchUserRef::<T64, T32>::new(&user, address))
    }

    /// Read an instance of T64 from `user` where the object has a different representation in 32
    /// and 64 bits.
    fn read_multi_arch_object<T, T64: FromBytes + TryInto<T>, T32: FromBytes + TryInto<T>>(
        &self,
        user: MappingMultiArchUserRef<T, T64, T32>,
    ) -> Result<T, Errno> {
        match user {
            MappingMultiArchUserRef::<T, T64, T32>::Arch64(user, _) => {
                self.read_object(user)?.try_into().map_err(|_| errno!(EINVAL))
            }
            MappingMultiArchUserRef::<T, T64, T32>::Arch32(user) => {
                self.read_object(user)?.try_into().map_err(|_| errno!(EINVAL))
            }
        }
    }

    /// Read exactly `len` objects from `user`, returning them as a Vec.
    fn read_multi_arch_objects_to_vec<
        T,
        T64: FromBytes + TryInto<T>,
        T32: FromBytes + TryInto<T>,
    >(
        &self,
        user: MappingMultiArchUserRef<T, T64, T32>,
        len: usize,
    ) -> Result<Vec<T>, Errno> {
        match user {
            MappingMultiArchUserRef::<T, T64, T32>::Arch64(user, _) => self
                .read_objects_to_vec(user, len)?
                .into_iter()
                .map(TryInto::<T>::try_into)
                .collect::<Result<Vec<T>, _>>()
                .map_err(|_| errno!(EINVAL)),
            MappingMultiArchUserRef::<T, T64, T32>::Arch32(user) => self
                .read_objects_to_vec(user, len)?
                .into_iter()
                .map(TryInto::<T>::try_into)
                .collect::<Result<Vec<T>, _>>()
                .map_err(|_| errno!(EINVAL)),
        }
    }

    /// Reads the first `partial` bytes of an object, leaving any remainder 0-filled.
    ///
    /// This is used for reading size-versioned structures where the user can specify an older
    /// version of the structure with a smaller size.
    ///
    /// Returns EINVAL if the input size is larger than the object (assuming the input size is from
    /// the user who has specified something we don't support).
    fn read_object_partial<T: FromBytes>(
        &self,
        user: UserRef<T>,
        partial_size: usize,
    ) -> Result<T, Errno> {
        let full_size = std::mem::size_of::<T>();
        if partial_size > full_size {
            return error!(EINVAL);
        }

        // This implementation involves an extra memcpy compared to read_object but avoids unsafe
        // code. This isn't currently called very often.
        let mut object = MaybeUninit::uninit();
        let (to_read, to_zero) = object_as_mut_bytes(&mut object).split_at_mut(partial_size);
        self.read_memory(user.addr(), to_read)?;

        // Zero pad out to the correct size.
        to_zero.fill(MaybeUninit::new(0));

        // SAFETY: `T` implements `FromBytes` so any bit pattern is valid and all
        // bytes of `object` have been initialized.
        Ok(unsafe { object.assume_init() })
    }

    /// Read exactly `objects.len()` objects into `objects` from `user`.
    fn read_objects<'a, T: FromBytes>(
        &self,
        user: UserRef<T>,
        objects: &'a mut [MaybeUninit<T>],
    ) -> Result<&'a mut [T], Errno> {
        let objects_len = objects.len();
        self.read_memory(user.addr(), slice_as_mut_bytes(objects)).map(|bytes_read| {
            debug_assert_eq!(bytes_read.len(), objects_len * std::mem::size_of::<T>());
            // SAFETY: `T` implements `FromBytes` and all bytes have been initialized.
            unsafe {
                std::slice::from_raw_parts_mut(bytes_read.as_mut_ptr() as *mut T, objects_len)
            }
        })
    }

    /// Read exactly `objects.len()` objects into `objects` from `user`.
    fn read_objects_to_slice<T: FromBytes>(
        &self,
        user: UserRef<T>,
        objects: &mut [T],
    ) -> Result<(), Errno> {
        let objects_len = objects.len();
        self.read_objects(user, slice_to_maybe_uninit_mut(objects))
            .map(|objects_read| debug_assert_eq!(objects_read.len(), objects_len))
    }

    /// Read exactly `len` objects from `user`, returning them as a Vec.
    fn read_objects_to_vec<T: FromBytes>(
        &self,
        user: UserRef<T>,
        len: usize,
    ) -> Result<Vec<T>, Errno> {
        // SAFETY: `self.read_objects` only returns `Ok` if all bytes were read to.
        unsafe {
            read_to_vec::<T, _>(len, |buf| {
                self.read_objects(user, buf).map(|objects_read| {
                    debug_assert_eq!(objects_read.len(), len);
                    NumberOfElementsRead(len)
                })
            })
        }
    }

    /// Read exactly `len` objects from `user`, returning them as a SmallVec.
    fn read_objects_to_smallvec<T: Clone + FromBytes, const N: usize>(
        &self,
        user: UserRef<T>,
        len: usize,
    ) -> Result<SmallVec<[T; N]>, Errno> {
        if len > N {
            Ok(SmallVec::<[T; N]>::from_vec(self.read_objects_to_vec(user, len)?))
        } else {
            // TODO(https://github.com/rust-lang/rust/issues/96097) use MaybeUninit::uninit_array
            // SAFETY: We are converting from an uninitialized array to an array of uninitialized
            // elements which is the same. See
            // https://doc.rust-lang.org/std/mem/union.MaybeUninit.html#initializing-an-array-element-by-element.
            let mut buffer: [MaybeUninit<T>; N] = unsafe { MaybeUninit::uninit().assume_init() };

            self.read_objects(user, &mut buffer[..len])?;

            // TODO(https://github.com/rust-lang/rust/issues/96097) use MaybeUninit::transpose
            // SAFETY: MaybeUninit<[T; N]> and [MaybeUninit<T>; N] have the same layout.
            let buffer: MaybeUninit<[T; N]> = unsafe { std::mem::transmute_copy(&buffer) };

            // SAFETY: `read_objects` guarantees that the first `len` entries are initialized.
            Ok(unsafe { SmallVec::from_buf_and_len_unchecked(buffer, len) })
        }
    }

    /// Read exactly `N` objects from `user`, returning them as an array.
    fn read_objects_to_array<T: Copy + FromBytes, const N: usize>(
        &self,
        user: UserRef<T>,
    ) -> Result<[T; N], Errno> {
        // SAFETY: `self.read_objects` only returns `Ok` if all bytes were read to.
        unsafe {
            read_to_array(|buf| {
                self.read_objects(user, buf).map(|objects_read| {
                    debug_assert_eq!(objects_read.len(), N);
                })
            })
        }
    }

    /// Read exactly `iovec_count` `UserBuffer`s from `iovec_addr`.
    ///
    /// Fails if `iovec_count` is greater than `UIO_MAXIOV`.
    fn read_iovec<T: Copy + Eq + IntoBytes + FromBytes + Immutable + TryInto<usize>>(
        &self,
        iovec_addr: IOVecPtr,
        iovec_count: UserValue<T>,
    ) -> Result<UserBuffers, Errno> {
        let iovec_count = iovec_count.raw().try_into().map_err(|_| errno!(EINVAL))?;
        if iovec_count > UIO_MAXIOV as usize {
            return error!(EINVAL);
        }

        if iovec_addr.is_arch32() {
            let ub32s: UserBuffers32 =
                self.read_objects_to_smallvec(iovec_addr.addr().into(), iovec_count)?;
            Ok(ub32s.iter().map(|&ub32| ub32.into()).collect())
        } else {
            self.read_objects_to_smallvec(iovec_addr.addr().into(), iovec_count)
        }
    }

    /// Read up to `max_size` bytes from `string`, stopping at the first discovered null byte and
    /// returning the results as a Vec.
    fn read_c_string_to_vec(
        &self,
        string: UserCString,
        max_size: usize,
    ) -> Result<FsString, Errno> {
        let chunk_size = std::cmp::min(*PAGE_SIZE as usize, max_size);

        let mut buf = Vec::with_capacity(chunk_size);
        let mut index = 0;
        loop {
            // This operation should never overflow: we should fail to read before that.
            let addr = string.addr().checked_add(index).ok_or_else(|| errno!(EFAULT))?;
            let read = self.read_memory_partial_until_null_byte(
                addr,
                &mut buf.spare_capacity_mut()[index..][..chunk_size],
            )?;
            let read_len = read.len();

            // Check if the last byte read is the null byte.
            if read.last() == Some(&0) {
                let null_index = index + read_len - 1;
                // SAFETY: Bytes until `null_index` have been initialized.
                unsafe { buf.set_len(null_index) }
                if buf.len() > max_size {
                    return error!(ENAMETOOLONG);
                }

                return Ok(buf.into());
            }
            index += read_len;

            if read_len < chunk_size || index >= max_size {
                // There's no more for us to read.
                return error!(ENAMETOOLONG);
            }

            // Trigger a capacity increase.
            buf.reserve(index + chunk_size);
        }
    }

    /// Read a path from `path`, returning it as a `FsString`.
    ///
    /// A convenience function that enforces the path length limit.
    fn read_path(&self, path: UserCString) -> Result<FsString, Errno> {
        self.read_c_string_to_vec(path, PATH_MAX as usize)
    }

    /// Read a path from `path`, returning it as a `FsString`, if the path is non-null.
    ///
    /// A convenience function that enforces the path length limit.
    fn read_path_if_non_null(&self, path: UserCString) -> Result<FsString, Errno> {
        if path.is_null() {
            Ok(Default::default())
        } else {
            self.read_c_string_to_vec(path, PATH_MAX as usize)
        }
    }

    /// Read `len` bytes from `start` and parse the region as null-delimited CStrings, for example
    /// how `argv` is stored.
    ///
    /// There can be an arbitrary number of null bytes in between `start` and `end`.
    fn read_nul_delimited_c_string_list(
        &self,
        start: UserAddress,
        len: usize,
    ) -> Result<Vec<FsString>, Errno> {
        let buf = self.read_memory_to_vec(start, len)?;
        let mut buf = &buf[..];

        let mut list = vec![];
        while !buf.is_empty() {
            let len_consumed = match CStr::from_bytes_until_nul(buf) {
                Ok(segment) => {
                    // Return the string without the null to match our other APIs, but advance the
                    // "cursor" of the buf variable past the null byte.
                    list.push(segment.to_bytes().into());
                    segment.to_bytes_with_nul().len()
                }
                Err(_) => {
                    // If we didn't find a null byte, then the whole rest of the buffer is the
                    // last string.
                    list.push(buf.into());
                    buf.len()
                }
            };
            buf = &buf[len_consumed..];
        }

        Ok(list)
    }

    /// Read up to `buffer.len()` bytes from `string`, stopping at the first discovered null byte
    /// and returning the result as a slice that ends before that null.
    ///
    /// Consider using `read_c_string_to_vec` if you do not require control over the allocation.
    fn read_c_string<'a>(
        &self,
        string: UserCString,
        buffer: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a FsStr, Errno> {
        let buffer = self.read_memory_partial_until_null_byte(string.addr(), buffer)?;
        // Make sure the last element holds the null byte.
        if let Some((null_byte, buffer)) = buffer.split_last() {
            if null_byte == &0 {
                return Ok(buffer.into());
            }
        }

        error!(ENAMETOOLONG)
    }

    /// Returns a default initialized string if `addr` is null, otherwise
    /// behaves as `read_c_string`.
    fn read_c_string_if_non_null<'a>(
        &self,
        addr: UserCString,
        buffer: &'a mut [MaybeUninit<u8>],
    ) -> Result<&'a FsStr, Errno> {
        if addr.is_null() { Ok(Default::default()) } else { self.read_c_string(addr, buffer) }
    }

    fn write_object<T: IntoBytes + Immutable>(
        &self,
        user: UserRef<T>,
        object: &T,
    ) -> Result<usize, Errno> {
        self.write_memory(user.addr(), object.as_bytes())
    }

    fn write_objects<T: IntoBytes + Immutable>(
        &self,
        user: UserRef<T>,
        objects: &[T],
    ) -> Result<usize, Errno> {
        self.write_memory(user.addr(), objects.as_bytes())
    }

    fn write_multi_arch_ptr<Addr, T64, T32>(
        &self,
        user: Addr,
        object: MultiArchUserRef<T64, T32>,
    ) -> Result<usize, Errno>
    where
        Addr: Into<UserAddress>,
    {
        if object.is_arch32() {
            let value = u32::try_from(object.ptr()).map_err(|_| errno!(EINVAL))?;
            self.write_memory(user.into(), value.as_bytes())
        } else {
            self.write_memory(user.into(), object.ptr().as_bytes())
        }
    }

    fn write_multi_arch_object<
        T,
        T64: IntoBytes + Immutable + TryFrom<T>,
        T32: IntoBytes + Immutable + TryFrom<T>,
    >(
        &self,
        user: MappingMultiArchUserRef<T, T64, T32>,
        object: T,
    ) -> Result<usize, Errno> {
        match user {
            MappingMultiArchUserRef::<T, T64, T32>::Arch64(user, _) => {
                self.write_object(user, &T64::try_from(object).map_err(|_| errno!(EINVAL))?)
            }
            MappingMultiArchUserRef::<T, T64, T32>::Arch32(user) => {
                self.write_object(user, &T32::try_from(object).map_err(|_| errno!(EINVAL))?)
            }
        }
    }
}

impl MemoryAccessorExt for dyn MemoryAccessor + '_ {}
impl<T: MemoryAccessor> MemoryAccessorExt for T {}
