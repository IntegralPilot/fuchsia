// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use linux_uapi::{__IncompleteArrayField, __u8, __u32};
use serde::{Deserialize, Serialize};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

#[derive(Debug, Deserialize, Serialize)]
pub struct FscryptOutput {
    pub identifier: [u8; 16],
}

#[repr(C)]
#[derive(Copy, Clone, Default, KnownLayout, FromBytes, Immutable, IntoBytes)]
pub struct fscrypt_remove_key_arg {
    pub key_spec: fscrypt_key_specifier,
    pub removal_status_flags: __u32,
    pub __reserved: [__u32; 5usize],
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
        unsafe {
            ::std::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}
impl Default for fscrypt_key_specifier {
    fn default() -> Self {
        let mut s = ::std::mem::MaybeUninit::<Self>::uninit();
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
        unsafe {
            ::std::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}
