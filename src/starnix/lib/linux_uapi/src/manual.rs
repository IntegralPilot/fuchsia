// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{__IncompleteArrayField, __u8, __u32};
use zerocopy::{FromBytes, Immutable, IntoBytes, KnownLayout};

#[repr(C)]
#[derive(Copy, Clone, FromBytes, Immutable, KnownLayout, IntoBytes)]
pub struct fscrypt_key_specifier {
    pub type_: __u32,
    pub __reserved: __u32,
    pub u: fscrypt_key_specifier__bindgen_ty_1,
}

#[repr(C)]
#[derive(Copy, Clone, FromBytes, Immutable, KnownLayout, IntoBytes)]
pub union fscrypt_key_specifier__bindgen_ty_1 {
    pub __reserved: [__u8; 32usize],
    pub descriptor: fscrypt_descriptor,
    pub identifier: fscrypt_identifier,
}

#[repr(C)]
#[derive(Copy, Clone, Default, FromBytes, Immutable, KnownLayout, IntoBytes)]
pub struct fscrypt_descriptor {
    pub value: [__u8; 8usize],
    pub __bindgen_padding_0: [u8; 24usize],
}

#[repr(C)]
#[derive(Copy, Clone, Default, FromBytes, Immutable, KnownLayout, IntoBytes)]
pub struct fscrypt_identifier {
    pub value: [__u8; 16usize],
    pub __bindgen_padding_0: [u8; 16usize],
}

impl Default for fscrypt_key_specifier__bindgen_ty_1 {
    fn default() -> Self {
        let mut s = ::std::mem::MaybeUninit::<Self>::uninit();
        // SAFETY: this is what bindgen would generate
        unsafe {
            ::std::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}
impl Default for fscrypt_key_specifier {
    fn default() -> Self {
        let mut s = ::std::mem::MaybeUninit::<Self>::uninit();
        // SAFETY: this is what bindgen would generate
        unsafe {
            ::std::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

#[repr(C)]
#[derive(Clone, FromBytes, Immutable, KnownLayout, IntoBytes)]
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
        // SAFETY: this is what bindgen would generate
        unsafe {
            ::std::ptr::write_bytes(s.as_mut_ptr(), 0, 1);
            s.assume_init()
        }
    }
}

crate::check_same_layout! {
    crate::statfs64 = crate::statfs {
        f_type => f_type,
        f_bsize => f_bsize,
        f_blocks => f_blocks,
        f_bfree => f_bfree,
        f_bavail => f_bavail,
        f_files => f_files,
        f_ffree => f_ffree,
        f_fsid => f_fsid,
        f_namelen => f_namelen,
        f_frsize => f_frsize,
        f_flags => f_flags,
    }
}

macro_rules! impl_debug {
    {} => {};
    {
        ,
        $($token:tt)*
    } => {
        impl_debug! { $($token)* }
    };
    {
        $name:ident
        $($token:tt)*
    } => {
        impl std::fmt::Debug for crate::$name {
            fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
                fmt.debug_struct(std::any::type_name::<Self>()).finish()
            }
        }
        #[cfg(target_arch = "aarch64")]
        impl std::fmt::Debug for crate::arch32::$name {
            fn fmt(&self, fmt: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
                fmt.debug_struct(std::any::type_name::<Self>()).finish()
            }
        }
        impl_debug! { $($token)* }
    };
}

impl_debug! {
    fuse_open_out__bindgen_ty_1,
    fuse_in_header__bindgen_ty_1,
}
