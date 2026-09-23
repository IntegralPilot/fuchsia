// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use linux_uapi::{
    __IncompleteArrayField, __u8, __u32, FS_IOC_ADD_ENCRYPTION_KEY, FS_IOC_SET_ENCRYPTION_POLICY,
    FSCRYPT_KEY_SPEC_TYPE_IDENTIFIER, FSCRYPT_MODE_AES_256_CTS, FSCRYPT_MODE_AES_256_XTS,
    FSCRYPT_POLICY_FLAGS_PAD_16, fscrypt_policy_v2,
};
use std::os::fd::AsRawFd;
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

pub const MASTER_ENCRYPTION_KEY: [u8; 64] = [1; 64];
pub const LINK_FILE_NAME: &str = "link.txt";
pub const TARGET_FILE_NAME: &str = "foo.txt";

pub fn add_encryption_key_with_key_bytes(
    root_dir: &std::fs::File,
    raw_key: &[u8],
) -> (i32, Vec<u8>) {
    let key_spec =
        fscrypt_key_specifier { type_: FSCRYPT_KEY_SPEC_TYPE_IDENTIFIER, ..Default::default() };
    let arg = fscrypt_add_key_arg { key_spec: key_spec, raw_size: 64, ..Default::default() };
    let mut arg_vec = arg.as_bytes().to_vec();
    arg_vec.extend(raw_key);
    // SAFETY: `arg_vec` is valid to write to for the first `sizeof(fscrypt_key_specifier)` bytes
    let ret = unsafe {
        libc::ioctl(
            root_dir.as_raw_fd(),
            FS_IOC_ADD_ENCRYPTION_KEY.try_into().unwrap(),
            arg_vec.as_mut_ptr(),
        )
    };
    (ret, arg_vec)
}

pub fn set_encryption_policy(dir: &std::fs::File, identifier: [u8; 16]) -> i32 {
    // SAFETY: basic FFI call with no invariants.
    let ret = unsafe {
        let policy = fscrypt_policy_v2 {
            version: 2,
            contents_encryption_mode: FSCRYPT_MODE_AES_256_XTS as u8,
            filenames_encryption_mode: FSCRYPT_MODE_AES_256_CTS as u8,
            flags: FSCRYPT_POLICY_FLAGS_PAD_16 as u8,
            master_key_identifier: identifier,
            ..Default::default()
        };
        libc::ioctl(dir.as_raw_fd(), FS_IOC_SET_ENCRYPTION_POLICY.try_into().unwrap(), &policy)
    };
    ret
}

#[repr(C)]
#[derive(Copy, Clone, KnownLayout, FromBytes, Immutable, IntoBytes)]
pub struct fscrypt_key_specifier {
    pub type_: __u32,
    pub __reserved: __u32,
    pub u: fscrypt_key_specifier__bindgen_ty_1,
}

#[repr(C)]
#[derive(Copy, Clone, KnownLayout, FromBytes, Immutable, IntoBytes)]
pub union fscrypt_key_specifier__bindgen_ty_1 {
    pub __reserved: [__u8; 32usize],
    pub descriptor: fscrypt_descriptor,
    pub identifier: fscrypt_identifier,
}

#[repr(C)]
#[derive(Copy, Clone, Default, KnownLayout, FromBytes, Immutable, IntoBytes)]
pub struct fscrypt_descriptor {
    pub value: [__u8; 8usize],
    pub __bindgen_padding_0: [u8; 24usize],
}

#[repr(C)]
#[derive(Copy, Clone, Default, KnownLayout, FromBytes, Immutable, IntoBytes)]
pub struct fscrypt_identifier {
    pub value: [__u8; 16usize],
    pub __bindgen_padding_0: [u8; 16usize],
}

impl Default for fscrypt_key_specifier__bindgen_ty_1 {
    fn default() -> Self {
        let mut s = ::std::mem::MaybeUninit::<Self>::uninit();
        // SAFETY: FromBytes implies that any bit patterns are sound for this type
        unsafe {
            ::std::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}
impl Default for fscrypt_key_specifier {
    fn default() -> Self {
        let mut s = ::std::mem::MaybeUninit::<Self>::uninit();
        // SAFETY: FromBytes implies that any bit patterns are sound for this type
        unsafe {
            ::std::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

#[repr(C)]
#[derive(Clone, KnownLayout, FromBytes, Immutable, IntoBytes)]
pub struct fscrypt_add_key_arg {
    pub key_spec: fscrypt_key_specifier,
    pub raw_size: __u32,
    pub key_id: __u32,
    pub __reserved: [__u32; 7usize],
    pub __flags: __u32,
    pub raw: __IncompleteArrayField<__u8>,
}
impl Default for fscrypt_add_key_arg {
    fn default() -> Self {
        let mut s = ::std::mem::MaybeUninit::<Self>::uninit();
        // SAFETY: FromBytes implies that any bit patterns are sound for this type
        unsafe {
            ::std::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}
