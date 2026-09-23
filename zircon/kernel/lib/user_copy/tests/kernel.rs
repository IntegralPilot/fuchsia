// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

/// Test suite for Rust user_copy implementation.
#[cfg(ktest)]
#[unittest::suite(name = "user_copy_rust")]
mod tests {
    use crate::user_copy::{
        UserInIovec, UserInOutIovec, UserInOutPtr, UserInPtr, UserOutIovec, UserOutPtr,
        UserStringView,
    };
    use crate::user_memory::UserMemory;
    use core::mem::MaybeUninit;
    use unittest::{assert_eq, assert_nonnull, assert_null, assert_true, unwrap_ok};
    use zx_status::Status;
    use zx_types::zx_iovec_t;

    /// Test UserPtr offsets.
    #[test]
    fn offsets() {
        let base = 0x1000 as *mut u32;

        // UserInPtr offset tests
        let in_ptr = UserInPtr::new(base as *const u32);
        assert_nonnull!(in_ptr);
        assert_eq!(in_ptr.byte_offset(8).as_ptr(), 0x1008 as *const u32);
        assert_eq!(in_ptr.byte_offset(-4).as_ptr(), 0x0ffc as *const u32);
        assert_eq!(in_ptr.element_offset(3).as_ptr(), 0x100c as *const u32);

        let null_in = UserInPtr::<u32>::new(core::ptr::null());
        assert_null!(null_in.byte_offset(8));
        assert_null!(null_in.element_offset(3));

        let def_in = UserInPtr::<u32>::default();
        assert_null!(def_in);

        // UserOutPtr offset tests
        let out_ptr = UserOutPtr::new(base);
        assert_nonnull!(out_ptr);
        assert_eq!(out_ptr.byte_offset(8).as_ptr(), 0x1008 as *mut u32);
        assert_eq!(out_ptr.byte_offset(-4).as_ptr(), 0x0ffc as *mut u32);
        assert_eq!(out_ptr.element_offset(3).as_ptr(), 0x100c as *mut u32);

        let null_out = UserOutPtr::<u32>::new(core::ptr::null_mut());
        assert_null!(null_out.byte_offset(8));
        assert_null!(null_out.element_offset(3));

        let def_out = UserOutPtr::<u32>::default();
        assert_null!(def_out);

        // UserInOutPtr offset tests
        let inout_ptr = UserInOutPtr::new(base);
        assert_nonnull!(inout_ptr);
        assert_eq!(inout_ptr.byte_offset(8).as_ptr(), 0x1008 as *mut u32);
        assert_eq!(inout_ptr.byte_offset(-4).as_ptr(), 0x0ffc as *mut u32);
        assert_eq!(inout_ptr.element_offset(3).as_ptr(), 0x100c as *mut u32);

        let null_inout = UserInOutPtr::<u32>::new(core::ptr::null_mut());
        assert_null!(null_inout.byte_offset(8));
        assert_null!(null_inout.element_offset(3));

        let def_inout = UserInOutPtr::<u32>::default();
        assert_null!(def_inout);

        // UserStringView Default test
        let def_sv = UserStringView::default();
        assert_null!(def_sv.data);
        assert_true!(def_sv.is_empty());
    }

    /// Test UserPtr default without trait bounds on T.
    #[test]
    fn default_unconstrained() {
        struct NoTraits(u8);
        let in_ptr = UserInPtr::<NoTraits>::default();
        assert_null!(in_ptr);
        let out_ptr = UserOutPtr::<NoTraits>::default();
        assert_null!(out_ptr);
        let inout_ptr = UserInOutPtr::<NoTraits>::default();
        assert_null!(inout_ptr);
    }

    /// Test CopyOut.
    #[test]
    fn copy_out() {
        let user = UserMemory::create(4096).unwrap();
        unwrap_ok!(user.commit_and_map(0..4096));

        let out_ptr = UserOutPtr::<u32>::new(user.base() as *mut u32);
        assert_nonnull!(out_ptr);
        unwrap_ok!(out_ptr.write(0xDEADBEEF));

        let mut temp = [MaybeUninit::<u8>::uninit(); 4];
        let bytes = unwrap_ok!(user.vmo_read(&mut temp, 0));
        let val = u32::from_ne_bytes(<[u8; 4]>::try_from(bytes).unwrap());
        assert_eq!(val, 0xDEADBEEF);
    }

    /// Test CopyIn.
    #[test]
    fn copy_in() {
        let user = UserMemory::create(4096).unwrap();
        unwrap_ok!(user.commit_and_map(0..4096));
        unwrap_ok!(user.vmo_write(&0xDEADBEEF_u32.to_ne_bytes(), 0));

        let in_ptr = UserInPtr::<u32>::new(user.base() as *const u32);
        assert_nonnull!(in_ptr);
        let val = unwrap_ok!(in_ptr.read());
        assert_eq!(val, 0xDEADBEEF);
    }

    /// Test CopyFromUser.
    #[test]
    fn copy_from_user() {
        let user = UserMemory::create(4096).unwrap();
        unwrap_ok!(user.commit_and_map(0..4096));
        unwrap_ok!(user.vmo_write(&0xDEADBEEF_u32.to_ne_bytes(), 0));

        let in_ptr = UserInPtr::<u32>::new(user.base() as *const u32);
        assert_nonnull!(in_ptr);
        let mut temp = MaybeUninit::uninit();
        let val_ref = unwrap_ok!(in_ptr.copy_from_user(&mut temp));
        assert_eq!(*val_ref, 0xDEADBEEF);
    }

    /// Test CopySliceFromUser.
    #[test]
    fn copy_slice_from_user() {
        let user = UserMemory::create(4096).unwrap();
        unwrap_ok!(user.commit_and_map(0..4096));
        let vals = [10u32, 20u32, 30u32];
        let mut bytes = [0u8; 12];
        for i in 0..3 {
            bytes[i * 4..(i + 1) * 4].copy_from_slice(&vals[i].to_ne_bytes());
        }
        unwrap_ok!(user.vmo_write(&bytes, 0));

        let in_ptr = UserInPtr::<u32>::new(user.base() as *const u32);
        assert_nonnull!(in_ptr);
        let mut out = [MaybeUninit::uninit(); 3];
        let out_slice = unwrap_ok!(in_ptr.copy_slice_from_user(&mut out));
        assert_eq!(out_slice[0], 10);
        assert_eq!(out_slice[1], 20);
        assert_eq!(out_slice[2], 30);
    }

    /// Test faults.
    #[test]
    fn faults() {
        let out_ptr = UserOutPtr::<u32>::new(core::ptr::null_mut());
        assert_null!(out_ptr);
        assert_true!(out_ptr.write(0xDEADBEEF).err() == Some(Status::INVALID_ARGS));

        let in_ptr = UserInPtr::<u32>::new(core::ptr::null());
        assert_null!(in_ptr);
        assert_true!(in_ptr.read().err() == Some(Status::INVALID_ARGS));

        let mut temp = MaybeUninit::uninit();
        assert_true!(in_ptr.copy_from_user(&mut temp).err() == Some(Status::INVALID_ARGS));

        let mut temp_slice = [MaybeUninit::uninit(); 1];
        assert_true!(
            in_ptr.copy_slice_from_user(&mut temp_slice).err() == Some(Status::INVALID_ARGS)
        );

        let bad_addr = usize::MAX as *mut u32;
        let out_ptr = UserOutPtr::<u32>::new(bad_addr);
        assert_true!(out_ptr.write(0xDEADBEEF).err() == Some(Status::INVALID_ARGS));

        let in_ptr = UserInPtr::<u32>::new(bad_addr as *const u32);
        assert_true!(in_ptr.read().err() == Some(Status::INVALID_ARGS));
    }

    /// Test IovecCapacity.
    #[test]
    fn iovec_capacity() {
        let user = UserMemory::create(4096).unwrap();
        unwrap_ok!(user.commit_and_map(0..4096));

        let vec = [
            zx_iovec_t { buffer: core::ptr::null(), capacity: 348 },
            zx_iovec_t { buffer: core::ptr::null(), capacity: 58 },
        ];

        let bytes = unsafe {
            core::slice::from_raw_parts(vec.as_ptr() as *const u8, core::mem::size_of_val(&vec))
        };
        unwrap_ok!(user.vmo_write(bytes, 0));

        let in_ptr = UserInPtr::<zx_iovec_t>::new(user.base() as *const zx_iovec_t);
        assert_nonnull!(in_ptr);

        let iovec = UserInIovec::new(in_ptr, 2);
        let total_capacity = unwrap_ok!(iovec.get_total_capacity());
        assert_eq!(total_capacity, 406);
    }

    /// Test IovecForeach.
    #[test]
    fn iovec_foreach() {
        let user = UserMemory::create(4096).unwrap();
        unwrap_ok!(user.commit_and_map(0..4096));

        let vec = [
            zx_iovec_t { buffer: core::ptr::null(), capacity: 7 },
            zx_iovec_t { buffer: core::ptr::null(), capacity: 11 },
            zx_iovec_t { buffer: core::ptr::null(), capacity: 13 },
        ];

        let bytes = unsafe {
            core::slice::from_raw_parts(vec.as_ptr() as *const u8, core::mem::size_of_val(&vec))
        };
        unwrap_ok!(user.vmo_write(bytes, 0));

        let in_ptr = UserInPtr::<zx_iovec_t>::new(user.base() as *const zx_iovec_t);
        assert_nonnull!(in_ptr);

        let iovec = UserInIovec::new(in_ptr, 3);
        let mut product = 2usize;
        let res = iovec.for_each(|_buf, cap| {
            product = product.wrapping_mul(cap);
            Ok(())
        });
        unwrap_ok!(res);
        assert_eq!(product, 2002);
    }

    /// Test StringView.
    #[test]
    fn string_view() {
        let user = UserMemory::create(4096).unwrap();
        unwrap_ok!(user.commit_and_map(0..4096));
        let k_string = b"Hello, Fuchsia!\0";
        unwrap_ok!(user.vmo_write(k_string, 0));

        let in_ptr = UserInPtr::<u8>::new(user.base() as *const u8);
        assert_nonnull!(in_ptr);

        let sv = UserStringView { data: in_ptr, length: k_string.len() };
        let mut buf = [MaybeUninit::uninit(); 32];
        let out_slice = unwrap_ok!(sv.copy_slice_from_user(&mut buf));
        assert_true!(out_slice == k_string);

        // Buffer too small should return INVALID_ARGS
        let mut small_buf = [MaybeUninit::uninit(); 5];
        assert_true!(sv.copy_slice_from_user(&mut small_buf).err() == Some(Status::INVALID_ARGS));

        // Test copy_user_string
        let str_slice = unwrap_ok!(sv.copy_user_string(&mut buf));
        assert_true!(str_slice == k_string);
        assert_eq!(unsafe { buf[k_string.len()].assume_init() }, 0);

        // copy_user_string truncates to small_buf.len() - 1 and null terminates
        let str_trunc = unwrap_ok!(sv.copy_user_string(&mut small_buf));
        assert_true!(str_trunc == &k_string[..4]);
        assert_eq!(unsafe { small_buf[4].assume_init() }, 0);

        // Empty string view returns empty slice without error
        let empty_sv = UserStringView { data: in_ptr, length: 0 };
        let empty_slice = unwrap_ok!(empty_sv.copy_user_string(&mut buf));
        assert_true!(empty_slice.is_empty());
        let slice_ptr = empty_slice.as_ptr();
        assert_eq!(unsafe { buf[0].assume_init() }, 0);
        assert_eq!(slice_ptr, buf.as_ptr() as *const u8);
    }

    /// Test IovecCopyToSlice.
    #[test]
    fn iovec_copy_to_slice() {
        let user = UserMemory::create(4096).unwrap();
        unwrap_ok!(user.commit_and_map(0..4096));

        let vec = [
            zx_iovec_t { buffer: 0x1234 as *const u8, capacity: 100 },
            zx_iovec_t { buffer: 0x5678 as *const u8, capacity: 200 },
        ];

        let bytes = unsafe {
            core::slice::from_raw_parts(vec.as_ptr() as *const u8, core::mem::size_of_val(&vec))
        };
        unwrap_ok!(user.vmo_write(bytes, 0));

        let in_ptr = UserInPtr::<zx_iovec_t>::new(user.base() as *const zx_iovec_t);
        let iovec = UserInIovec::new(in_ptr, 2);

        let mut out = [MaybeUninit::uninit(); 2];
        let out_slice = unwrap_ok!(iovec.copy_to_slice(&mut out));
        assert_eq!(out_slice.len(), 2);
        assert_eq!(out_slice[0].data.as_ptr(), 0x1234 as *const u8);
        assert_eq!(out_slice[0].len, 100);
        assert_eq!(out_slice[1].data.as_ptr(), 0x5678 as *const u8);
        assert_eq!(out_slice[1].len, 200);

        // UserOutIovec copy_to_slice test
        let out_iovec = UserOutIovec::new(in_ptr, 2);
        let mut out_buf = [MaybeUninit::uninit(); 2];
        let out_vec = unwrap_ok!(out_iovec.copy_to_slice(&mut out_buf));
        assert_eq!(out_vec.len(), 2);
        assert_eq!(out_vec[0].data.as_ptr(), 0x1234 as *mut u8);
        assert_eq!(out_vec[0].len, 100);
        assert_eq!(out_vec[1].data.as_ptr(), 0x5678 as *mut u8);
        assert_eq!(out_vec[1].len, 200);

        // UserInOutIovec copy_to_slice test
        let inout_iovec = UserInOutIovec::new(in_ptr, 2);
        let mut inout_buf = [MaybeUninit::uninit(); 2];
        let inout_vec = unwrap_ok!(inout_iovec.copy_to_slice(&mut inout_buf));
        assert_eq!(inout_vec.len(), 2);
        assert_eq!(inout_vec[0].data.as_ptr(), 0x1234 as *mut u8);
        assert_eq!(inout_vec[0].len, 100);
        assert_eq!(inout_vec[1].data.as_ptr(), 0x5678 as *mut u8);
        assert_eq!(inout_vec[1].len, 200);

        // Destination slice too small should return INVALID_ARGS
        let mut small_out = [MaybeUninit::uninit(); 1];
        assert_true!(iovec.copy_to_slice(&mut small_out).err() == Some(Status::INVALID_ARGS));
    }

    /// Test UserInPtr copy_user_string.
    #[test]
    fn user_string() {
        let user = UserMemory::create(4096).unwrap();
        unwrap_ok!(user.commit_and_map(0..4096));

        let test_name = b"my_resource_name";
        unwrap_ok!(user.vmo_write(test_name, 0));

        let in_ptr = UserInPtr::<u8>::new(user.base() as *const u8);
        let null_ptr = UserInPtr::<u8>::new(core::ptr::null());

        // Empty buffer returns INVALID_ARGS
        let mut empty_buf: [MaybeUninit<u8>; 0] = [];
        assert_true!(
            in_ptr.copy_user_string(test_name.len(), &mut empty_buf).err()
                == Some(Status::INVALID_ARGS)
        );

        // src_len == 0 returns Ok(&[]) without reading user memory (works even with null pointer)
        let mut buf = [MaybeUninit::uninit(); 32];
        let empty_slice = unwrap_ok!(null_ptr.copy_user_string(0, &mut buf));
        assert_true!(empty_slice.is_empty());
        let slice_ptr = empty_slice.as_ptr();
        assert_eq!(unsafe { buf[0].assume_init() }, 0);
        assert_eq!(slice_ptr, buf.as_ptr() as *const u8);

        // src_len > 0 with null pointer returns INVALID_ARGS
        assert_true!(
            null_ptr.copy_user_string(test_name.len(), &mut buf).err()
                == Some(Status::INVALID_ARGS)
        );

        // Valid string copy
        let str_slice = unwrap_ok!(in_ptr.copy_user_string(test_name.len(), &mut buf));
        assert_true!(str_slice == &test_name[..]);
        // verify null-termination byte
        assert_eq!(unsafe { buf[test_name.len()].assume_init() }, 0);

        // Large src_len truncates to buf.len() - 1, copying the minimum amount of bytes
        let mut small_buf = [MaybeUninit::uninit(); 5];
        let trunc_slice = unwrap_ok!(in_ptr.copy_user_string(test_name.len(), &mut small_buf));
        assert_true!(trunc_slice == &test_name[..4]);
        assert_eq!(unsafe { small_buf[4].assume_init() }, 0);

        // src_len == buf.len() also truncates to buf.len() - 1 to leave room for null terminator
        let mut exact_buf = [MaybeUninit::uninit(); 6];
        let str_trunc = unwrap_ok!(in_ptr.copy_user_string(6, &mut exact_buf));
        assert_true!(str_trunc == &test_name[..5]);
        assert_eq!(unsafe { exact_buf[5].assume_init() }, 0);
    }
}
