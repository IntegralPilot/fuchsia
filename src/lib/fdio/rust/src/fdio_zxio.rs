// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod fdio_sys;
mod zxio_sys;

use std::marker::PhantomData;
use std::os::fd::RawFd;

/// Custom operations for an fd.
pub trait FdOps: Sized + Send + 'static {
    fn writev(&self, _iovecs: &[zx::sys::zx_iovec_t]) -> Result<usize, zx::Status> {
        Err(zx::Status::WRONG_TYPE)
    }

    fn isatty(&self) -> Result<bool, zx::Status> {
        Ok(false)
    }

    // NOTE: Support for more operations can be added as and when required.
}

/// A copy of `zxio_default_ops`.
const DEFAULT_ZXIO_OPS: zxio_sys::zxio_ops = zxio_sys::zxio_ops {
    destroy: Some(zxio_sys::zxio_default_destroy),
    close: Some(zxio_sys::zxio_default_close),
    release: Some(zxio_sys::zxio_default_release),
    borrow: Some(zxio_sys::zxio_default_borrow),
    clone: Some(zxio_sys::zxio_default_clone),
    wait_begin: Some(zxio_sys::zxio_default_wait_begin),
    wait_end: Some(zxio_sys::zxio_default_wait_end),
    sync: Some(zxio_sys::zxio_default_sync),
    attr_get: Some(zxio_sys::zxio_default_attr_get),
    attr_set: Some(zxio_sys::zxio_default_attr_set),
    readv: Some(zxio_sys::zxio_default_readv),
    readv_at: Some(zxio_sys::zxio_default_readv_at),
    writev: Some(zxio_sys::zxio_default_writev),
    writev_at: Some(zxio_sys::zxio_default_writev_at),
    seek: Some(zxio_sys::zxio_default_seek),
    truncate: Some(zxio_sys::zxio_default_truncate),
    flags_get: Some(zxio_sys::zxio_default_flags_get),
    flags_set: Some(zxio_sys::zxio_default_flags_set),
    vmo_get: Some(zxio_sys::zxio_default_vmo_get),
    on_mapped: Some(zxio_sys::zxio_default_on_mapped),
    get_read_buffer_available: Some(zxio_sys::zxio_default_get_read_buffer_available),
    shutdown: Some(zxio_sys::zxio_default_shutdown),
    unlink: Some(zxio_sys::zxio_default_unlink),
    token_get: Some(zxio_sys::zxio_default_token_get),
    rename: Some(zxio_sys::zxio_default_rename),
    link: Some(zxio_sys::zxio_default_link),
    link_into: Some(zxio_sys::zxio_default_link_into),
    dirent_iterator_init: Some(zxio_sys::zxio_default_dirent_iterator_init),
    dirent_iterator_next: Some(zxio_sys::zxio_default_dirent_iterator_next),
    dirent_iterator_rewind: Some(zxio_sys::zxio_default_dirent_iterator_rewind),
    dirent_iterator_destroy: Some(zxio_sys::zxio_default_dirent_iterator_destroy),
    isatty: Some(zxio_sys::zxio_default_isatty),
    get_window_size: Some(zxio_sys::zxio_default_get_window_size),
    set_window_size: Some(zxio_sys::zxio_default_set_window_size),
    advisory_lock: Some(zxio_sys::zxio_default_advisory_lock),
    watch_directory: Some(zxio_sys::zxio_default_watch_directory),
    bind: Some(zxio_sys::zxio_default_bind),
    connect: Some(zxio_sys::zxio_default_connect),
    listen: Some(zxio_sys::zxio_default_listen),
    accept: Some(zxio_sys::zxio_default_accept),
    getsockname: Some(zxio_sys::zxio_default_getsockname),
    getpeername: Some(zxio_sys::zxio_default_getpeername),
    getsockopt: Some(zxio_sys::zxio_default_getsockopt),
    setsockopt: Some(zxio_sys::zxio_default_setsockopt),
    recvmsg: Some(zxio_sys::zxio_default_recvmsg),
    sendmsg: Some(zxio_sys::zxio_default_sendmsg),
    ioctl: Some(zxio_sys::zxio_default_ioctl),
    read_link: Some(zxio_sys::zxio_default_read_link),
    create_symlink: Some(zxio_sys::zxio_default_create_symlink),
    xattr_list: Some(zxio_sys::zxio_default_xattr_list),
    xattr_get: Some(zxio_sys::zxio_default_xattr_get),
    xattr_set: Some(zxio_sys::zxio_default_xattr_set),
    xattr_remove: Some(zxio_sys::zxio_default_xattr_remove),
    open: Some(zxio_sys::zxio_default_open),
    allocate: Some(zxio_sys::zxio_default_allocate),
    enable_verity: Some(zxio_sys::zxio_default_enable_verity),
};

/// Bind `ops` to a specific file descriptor.
///
/// NOTE: Due to the underlying use of fdio, error cases might also clobber errno.
pub fn bind_to_fd_with_ops<T: FdOps>(ops: T, fd: RawFd) -> Result<(), zx::Status> {
    struct AssertCompatible<T>(PhantomData<T>);

    impl<T> AssertCompatible<T> {
        const SIZE_OK: () = assert!(
            std::mem::size_of::<T>() <= std::mem::size_of::<zxio_sys::zxio_private>(),
            "bad size"
        );
        const ALIGNMENT_OK: () = assert!(
            std::mem::align_of::<T>() <= std::mem::align_of::<zxio_sys::zxio_private>(),
            "bad alignment"
        );
    }

    let () = AssertCompatible::<T>::SIZE_OK;
    let () = AssertCompatible::<T>::ALIGNMENT_OK;

    if fd < 0 {
        // fdio_bind_to_fd supports finding the next available fd when provided with a negative
        // number, but due to lack of use-cases for this in Rust this is currently unsupported by
        // this function.
        return Err(zx::Status::INVALID_ARGS);
    }

    let mut storage = std::ptr::null_mut();
    let fdio = unsafe { fdio_sys::fdio_zxio_create(&mut storage) };

    if fdio.is_null() {
        return Err(zx::Status::INTERNAL);
    }

    // NOTE: We now own `fdio`, so we must take care not to leak.

    unsafe {
        let reserved: *mut _ = &mut (*storage.cast::<zxio_sys::zxio_storage>()).reserved;
        reserved.cast::<T>().write(ops);
    }

    struct Adapter<T>(PhantomData<T>);
    impl<T: FdOps> Adapter<T> {
        unsafe fn to_data(zxio: *mut zxio_sys::zxio_t) -> *mut T {
            let storage = zxio.cast::<zxio_sys::zxio_storage>();
            let reserved: *mut _ = unsafe { &mut (*storage).reserved };
            reserved.cast::<T>()
        }

        unsafe extern "C" fn destroy(io: *mut zxio_sys::zxio_t) {
            unsafe { std::ptr::drop_in_place(Self::to_data(io)) };
        }

        unsafe extern "C" fn writev(
            io: *mut zxio_sys::zxio_t,
            vector: *const zxio_sys::zx_iovec_t,
            vector_count: usize,
            _flags: zxio_sys::zxio_flags_t,
            out_actual: *mut usize,
        ) -> zxio_sys::zx_status_t {
            let data = unsafe { &*Self::to_data(io) };
            match data.writev(unsafe {
                std::slice::from_raw_parts(vector.cast::<zx::sys::zx_iovec_t>(), vector_count)
            }) {
                Ok(count) => {
                    unsafe { *out_actual = count };
                    zx::sys::ZX_OK
                }
                Err(status) => status.into_raw(),
            }
        }

        unsafe extern "C" fn isatty(
            io: *mut zxio_sys::zxio_t,
            tty: *mut bool,
        ) -> zxio_sys::zx_status_t {
            let data = unsafe { &*Self::to_data(io) };
            match data.isatty() {
                Ok(result) => {
                    unsafe { *tty = result };
                    zx::sys::ZX_OK
                }
                Err(status) => status.into_raw(),
            }
        }

        const OPS: zxio_sys::zxio_ops = zxio_sys::zxio_ops {
            destroy: Some(Self::destroy),
            writev: Some(Self::writev),
            isatty: Some(Self::isatty),
            ..DEFAULT_ZXIO_OPS
        };
    }

    unsafe {
        zxio_sys::zxio_init(storage.cast::<zxio_sys::zxio_tag>(), &Adapter::<T>::OPS);
    }

    // The fdio object is always consumed.
    let bound_fd = unsafe { fdio_sys::fdio_bind_to_fd(fdio, fd, 0) };
    if bound_fd < 0 {
        return Err(zx::Status::BAD_STATE);
    }
    // We requested a specific fd, we expect to have gotten it, or failed.
    assert_eq!(bound_fd, fd);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct MockFdOps {
        dropped_counter: Arc<AtomicUsize>,
        writev_cb: Option<Box<dyn Fn(&[zx::sys::zx_iovec_t]) + Send + 'static>>,
        isatty_cb: Option<Box<dyn Fn() -> bool + Send + 'static>>,
    }

    impl Drop for MockFdOps {
        fn drop(&mut self) {
            self.dropped_counter.fetch_add(1, Ordering::Relaxed);
        }
    }

    impl FdOps for MockFdOps {
        fn writev(&self, iovecs: &[zx::sys::zx_iovec_t]) -> Result<usize, zx::Status> {
            if let Some(cb) = self.writev_cb.as_ref() {
                cb(iovecs);
            }
            Ok(iovecs.iter().map(|v| v.capacity).sum())
        }

        fn isatty(&self) -> Result<bool, zx::Status> {
            if let Some(cb) = self.isatty_cb.as_ref() { Ok(cb()) } else { Ok(false) }
        }
    }

    #[fuchsia::test]
    fn test_bind_to_fd_with_ops() {
        let writev_called = Arc::new(AtomicBool::new(false));
        let isatty_called = Arc::new(AtomicBool::new(false));
        let dropped_counter = Arc::new(AtomicUsize::new(0));

        {
            let writev_called = writev_called.clone();
            let isatty_called = isatty_called.clone();
            let ops = MockFdOps {
                dropped_counter: dropped_counter.clone(),
                writev_cb: Some(Box::new(move |iovecs| {
                    writev_called.store(true, Ordering::Relaxed);
                    assert_eq!(iovecs.len(), 1);
                    let written_data = unsafe {
                        std::slice::from_raw_parts(
                            iovecs[0].buffer as *const u8,
                            iovecs[0].capacity,
                        )
                    };
                    assert_eq!(written_data, b"hello\n");
                })),
                isatty_cb: Some(Box::new(move || {
                    isatty_called.store(true, Ordering::Relaxed);
                    true
                })),
            };

            // Bind stdout.
            bind_to_fd_with_ops(ops, 1).unwrap();
        }

        assert!(!writev_called.load(Ordering::Relaxed));
        assert!(!isatty_called.load(Ordering::Relaxed));
        println!("hello");
        assert!(writev_called.load(Ordering::Relaxed));

        assert_eq!(unsafe { libc::isatty(1) }, 1);
        assert!(isatty_called.load(Ordering::Relaxed));

        assert_eq!(dropped_counter.load(Ordering::Relaxed), 0);

        // Binding another set of mock operations should cause the previous ones
        // to be dropped.
        bind_to_fd_with_ops(
            MockFdOps {
                dropped_counter: dropped_counter.clone(),
                writev_cb: None,
                isatty_cb: None,
            },
            1,
        )
        .unwrap();

        assert_eq!(dropped_counter.load(Ordering::Relaxed), 1);
    }

    #[fuchsia::test]
    fn test_bind_failure() {
        let dropped_counter = Arc::new(AtomicUsize::new(0));
        assert_eq!(
            bind_to_fd_with_ops(
                MockFdOps {
                    dropped_counter: dropped_counter.clone(),
                    writev_cb: None,
                    isatty_cb: None
                },
                -1
            ),
            Err(zx::Status::INVALID_ARGS)
        );
        assert_eq!(dropped_counter.load(Ordering::Relaxed), 1);

        assert_eq!(
            bind_to_fd_with_ops(
                MockFdOps {
                    dropped_counter: dropped_counter.clone(),
                    writev_cb: None,
                    isatty_cb: None
                },
                1234
            ),
            Err(zx::Status::BAD_STATE)
        );
        assert_eq!(dropped_counter.load(Ordering::Relaxed), 2);
    }
}
