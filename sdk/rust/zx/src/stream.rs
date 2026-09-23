// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe bindings for Zircon stream objects.

use crate::{NullableHandle, Status, StatusExt, Vmo, ok, sys};
use bitflags::bitflags;
use std::io::SeekFrom;
use std::mem::MaybeUninit;

/// An object representing a Zircon [stream](https://fuchsia.dev/fuchsia-src/concepts/objects/stream.md).
///
/// As essentially a subtype of `NullableHandle`, it can be freely interconverted.
#[derive(Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
#[repr(transparent)]
pub struct Stream(NullableHandle);
impl_handle_based!(Stream);

bitflags! {
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct StreamOptions: u32 {
        const MODE_READ = sys::ZX_STREAM_MODE_READ;
        const MODE_WRITE = sys::ZX_STREAM_MODE_WRITE;
        const MODE_APPEND = sys::ZX_STREAM_MODE_APPEND;
    }
}

bitflags! {
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct StreamReadOptions: u32 {
    }
}

bitflags! {
    #[repr(transparent)]
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
    pub struct StreamWriteOptions: u32 {
        const APPEND = sys::ZX_STREAM_APPEND;
    }
}

impl Stream {
    /// See [zx_stream_create](https://fuchsia.dev/fuchsia-src/reference/syscalls/stream_create)
    pub fn create(options: StreamOptions, vmo: &Vmo, offset: u64) -> Result<Self, Status> {
        let mut handle = 0;
        let status =
            unsafe { sys::zx_stream_create(options.bits(), vmo.raw_handle(), offset, &mut handle) };
        ok(status)?;
        unsafe { Ok(Stream::from(NullableHandle::from_raw(handle))) }
    }

    /// Wraps the
    /// [`zx_stream_readv`](https://fuchsia.dev/fuchsia-src/reference/syscalls/stream_readv)
    /// syscall.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that the buffers in `iovecs` point to valid (albeit
    /// not necessarily initialized) memory.
    pub unsafe fn readv(
        &self,
        options: StreamReadOptions,
        iovecs: &mut [sys::zx_iovec_t],
    ) -> Result<usize, Status> {
        let mut actual = 0;
        let status = unsafe {
            sys::zx_stream_readv(
                self.raw_handle(),
                options.bits(),
                iovecs.as_mut_ptr(),
                iovecs.len(),
                &mut actual,
            )
        };
        ok(status)?;
        Ok(actual)
    }

    /// Attempts to read `buffer.len()` bytes from the stream starting at the stream's current seek
    /// offset. Only the number of bytes read from the stream will be initialized in `buffer`.
    /// Returns the number of bytes read from the stream.
    ///
    /// See [zx_stream_readv](https://fuchsia.dev/fuchsia-src/reference/syscalls/stream_readv).
    pub fn read_uninit(
        &self,
        options: StreamReadOptions,
        buffer: &mut [MaybeUninit<u8>],
    ) -> Result<usize, Status> {
        // TODO(https://fxbug.dev/42079723) use MaybeUninit::slice_as_mut_ptr when stable
        let mut iovec =
            [sys::zx_iovec_t { buffer: buffer.as_mut_ptr().cast::<u8>(), capacity: buffer.len() }];
        // SAFETY: The buffer in `iovec` comes from a mutable slice so we know it's safe to pass it
        // to `readv`.
        unsafe { self.readv(options, &mut iovec) }
    }

    /// Attempts to read `length` bytes from the stream starting at the stream's current seek
    /// offset. Returns the read bytes as a `Vec`.
    ///
    /// See [zx_stream_readv](https://fuchsia.dev/fuchsia-src/reference/syscalls/stream_readv).
    pub fn read_to_vec(
        &self,
        options: StreamReadOptions,
        length: usize,
    ) -> Result<Vec<u8>, Status> {
        let mut data = Vec::with_capacity(length);
        let buffer = &mut data.spare_capacity_mut()[0..length];
        let actual = self.read_uninit(options, buffer)?;
        // SAFETY: read_uninit returns the number of bytes that were initialized.
        unsafe { data.set_len(actual) };
        Ok(data)
    }

    /// Wraps the
    /// [`zx_stream_readv_at`](https://fuchsia.dev/fuchsia-src/reference/syscalls/stream_readv_at)
    /// syscall.
    ///
    /// # Safety
    ///
    /// The caller is responsible for ensuring that the buffers in `iovecs` point to valid (albeit
    /// not necessarily initialized) memory.
    pub unsafe fn readv_at(
        &self,
        options: StreamReadOptions,
        offset: u64,
        iovecs: &mut [sys::zx_iovec_t],
    ) -> Result<usize, Status> {
        let mut actual = 0;
        let status = unsafe {
            sys::zx_stream_readv_at(
                self.raw_handle(),
                options.bits(),
                offset,
                iovecs.as_mut_ptr(),
                iovecs.len(),
                &mut actual,
            )
        };
        ok(status)?;
        Ok(actual)
    }

    /// Attempts to read `buffer.len()` bytes from the stream starting at `offset`. Only the number
    /// of bytes read from the stream will be initialized in `buffer`. Returns the number of bytes
    /// read from the stream.
    ///
    /// See
    /// [zx_stream_readv_at](https://fuchsia.dev/fuchsia-src/reference/syscalls/stream_readv_at).
    pub fn read_at_uninit(
        &self,
        options: StreamReadOptions,
        offset: u64,
        buffer: &mut [MaybeUninit<u8>],
    ) -> Result<usize, Status> {
        // TODO(https://fxbug.dev/42079723) Use MaybeUninit::slice_as_mut_ptr when stable.
        let mut iovec =
            [sys::zx_iovec_t { buffer: buffer.as_mut_ptr().cast::<u8>(), capacity: buffer.len() }];
        // SAFETY: The buffer in `iovec` comes from a mutable slice so we know it's safe to pass it
        // to `readv_at`.
        unsafe { self.readv_at(options, offset, &mut iovec) }
    }

    /// Attempts to read `length` bytes from the stream starting at `offset`. Returns the read bytes
    /// as a `Vec`.
    ///
    /// See
    /// [zx_stream_readv_at](https://fuchsia.dev/fuchsia-src/reference/syscalls/stream_readv_at).
    pub fn read_at_to_vec(
        &self,
        options: StreamReadOptions,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, Status> {
        let mut data = Vec::with_capacity(length);
        let buffer = &mut data.spare_capacity_mut()[0..length];
        let actual = self.read_at_uninit(options, offset, buffer)?;
        // SAFETY: read_at_uninit returns the number of bytes that were initialized.
        unsafe { data.set_len(actual) };
        Ok(data)
    }

    /// See [zx_stream_seek](https://fuchsia.dev/fuchsia-src/reference/syscalls/stream_seek)
    pub fn seek(&self, pos: SeekFrom) -> Result<u64, Status> {
        let (whence, offset) = match pos {
            SeekFrom::Start(start) => (
                sys::ZX_STREAM_SEEK_ORIGIN_START,
                start.try_into().map_err(|_| Status::OUT_OF_RANGE)?,
            ),
            SeekFrom::End(end) => (sys::ZX_STREAM_SEEK_ORIGIN_END, end),
            SeekFrom::Current(current) => (sys::ZX_STREAM_SEEK_ORIGIN_CURRENT, current),
        };
        let mut pos = 0;
        let status = unsafe { sys::zx_stream_seek(self.raw_handle(), whence, offset, &mut pos) };
        ok(status)?;
        Ok(pos)
    }

    /// Wraps the
    /// [`zx_stream_writev`](https://fuchsia.dev/fuchsia-src/reference/syscalls/stream_writev)
    /// syscall.
    pub fn writev(
        &self,
        options: StreamWriteOptions,
        iovecs: &[sys::zx_iovec_t],
    ) -> Result<usize, Status> {
        let mut actual = 0;
        let status = unsafe {
            sys::zx_stream_writev(
                self.raw_handle(),
                options.bits(),
                iovecs.as_ptr(),
                iovecs.len(),
                &mut actual,
            )
        };
        ok(status)?;
        Ok(actual)
    }

    /// Writes `buffer` to the stream at the stream's current seek offset. Returns the number of
    /// bytes written.
    ///
    /// See [zx_stream_writev](https://fuchsia.dev/fuchsia-src/reference/syscalls/stream_writev).
    pub fn write(&self, options: StreamWriteOptions, buffer: &[u8]) -> Result<usize, Status> {
        let iovec = [sys::zx_iovec_t { buffer: buffer.as_ptr(), capacity: buffer.len() }];
        self.writev(options, &iovec)
    }

    /// Wraps the
    /// [`zx_stream_writev_at`](https://fuchsia.dev/fuchsia-src/reference/syscalls/stream_writev_at)
    /// syscall.
    pub fn writev_at(
        &self,
        options: StreamWriteOptions,
        offset: u64,
        iovecs: &[sys::zx_iovec_t],
    ) -> Result<usize, Status> {
        let mut actual = 0;
        let status = unsafe {
            sys::zx_stream_writev_at(
                self.raw_handle(),
                options.bits(),
                offset,
                iovecs.as_ptr(),
                iovecs.len(),
                &mut actual,
            )
        };
        ok(status)?;
        Ok(actual)
    }

    /// Writes `buffer` to the stream at `offset``. Returns the number of bytes written.
    ///
    /// See
    /// [zx_stream_writev_at](https://fuchsia.dev/fuchsia-src/reference/syscalls/stream_writev_at).
    pub fn write_at(
        &self,
        options: StreamWriteOptions,
        offset: u64,
        buffer: &[u8],
    ) -> Result<usize, Status> {
        let iovec = [sys::zx_iovec_t { buffer: buffer.as_ptr(), capacity: buffer.len() }];
        self.writev_at(options, offset, &iovec)
    }
}

unsafe_handle_properties!(object: Stream,
    props: [
        {query_ty: STREAM_MODE_APPEND, tag: StreamModeAppendTag, prop_ty: u8, get: get_mode_append, set: set_mode_append},
    ]
);

impl std::io::Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut iovec = [sys::zx_iovec_t { buffer: buf.as_mut_ptr(), capacity: buf.len() }];
        // SAFETY: The buffer in `iovec` comes from a mutable slice so we know it's safe to pass it
        // to `readv`.
        Ok(unsafe { self.readv(StreamReadOptions::empty(), &mut iovec) }
            .map_err(|s| s.into_io_error())?)
    }

    fn read_vectored(&mut self, bufs: &mut [std::io::IoSliceMut<'_>]) -> std::io::Result<usize> {
        // SAFETY: `zx_iovec_t` and `IoSliceMut` have the same layout.
        let mut iovecs = unsafe {
            std::slice::from_raw_parts_mut(bufs.as_mut_ptr().cast::<sys::zx_iovec_t>(), bufs.len())
        };
        // SAFETY: `IoSliceMut` can only be constructed from a mutable slice so we know it's safe to
        // pass to `readv`.
        Ok(unsafe { self.readv(StreamReadOptions::empty(), &mut iovecs) }
            .map_err(|s| s.into_io_error())?)
    }
}

impl std::io::Seek for Stream {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        Ok(Self::seek(&self, pos).map_err(|s| s.into_io_error())? as u64)
    }
}

impl std::io::Write for Stream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        Ok(Self::write(&self, StreamWriteOptions::empty(), buf).map_err(|s| s.into_io_error())?)
    }

    fn write_vectored(&mut self, bufs: &[std::io::IoSlice<'_>]) -> std::io::Result<usize> {
        // SAFETY: `zx_iovec_t` and `IoSliceMut` have the same layout.
        let iovecs = unsafe {
            std::slice::from_raw_parts(bufs.as_ptr().cast::<sys::zx_iovec_t>(), bufs.len())
        };
        Ok(self.writev(StreamWriteOptions::empty(), &iovecs).map_err(|s| s.into_io_error())?)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate as zx;

    #[test]
    fn create() {
        let vmo = zx::Vmo::create_with_opts(zx::VmoOptions::RESIZABLE, 0).unwrap();

        let stream =
            Stream::create(StreamOptions::MODE_READ | StreamOptions::MODE_WRITE, &vmo, 0).unwrap();

        let basic_info = stream.basic_info().unwrap();
        assert_eq!(basic_info.object_type, zx::ObjectType::STREAM);
        assert!(basic_info.rights.contains(zx::Rights::READ));
        assert!(basic_info.rights.contains(zx::Rights::WRITE));
    }

    #[test]
    fn create_readonly() {
        let vmo = zx::Vmo::create_with_opts(zx::VmoOptions::RESIZABLE, 0).unwrap();

        let stream = Stream::create(StreamOptions::MODE_READ, &vmo, 0).unwrap();

        let basic_info = stream.basic_info().unwrap();
        assert!(basic_info.rights.contains(zx::Rights::READ));
        assert!(!basic_info.rights.contains(zx::Rights::WRITE));
    }

    #[test]
    fn create_offset() {
        let vmo = zx::Vmo::create_with_opts(zx::VmoOptions::RESIZABLE, 0).unwrap();
        let stream = Stream::create(StreamOptions::MODE_READ, &vmo, 24).unwrap();
        assert_eq!(stream.seek(SeekFrom::Current(0)).unwrap(), 24);
    }

    #[test]
    fn create_with_mode_append() {
        let size: u64 = zx::system_get_page_size().into();
        let vmo = zx::Vmo::create(size).unwrap();
        let stream =
            Stream::create(StreamOptions::MODE_WRITE | StreamOptions::MODE_APPEND, &vmo, 0)
                .unwrap();
        assert_eq!(stream.get_mode_append().unwrap(), 1);
    }

    #[test]
    fn get_and_set_mode_append() {
        let size: u64 = zx::system_get_page_size().into();
        let vmo = zx::Vmo::create(size).unwrap();
        let stream = Stream::create(StreamOptions::MODE_WRITE, &vmo, 0).unwrap();
        assert_eq!(stream.get_mode_append().unwrap(), 0);
        stream.set_mode_append(&1).unwrap();
        assert_eq!(stream.get_mode_append().unwrap(), 1);
        stream.set_mode_append(&0).unwrap();
        assert_eq!(stream.get_mode_append().unwrap(), 0);
    }

    #[test]
    fn read_uninit() {
        const DATA: &'static [u8] = b"vmo-contents";
        let vmo = zx::Vmo::create(DATA.len() as u64).unwrap();
        vmo.write(DATA, 0).unwrap();
        let stream = Stream::create(StreamOptions::MODE_READ, &vmo, 0).unwrap();

        // Read from the stream.
        let mut data = Vec::with_capacity(5);
        let bytes_read =
            stream.read_uninit(StreamReadOptions::empty(), data.spare_capacity_mut()).unwrap();
        assert_eq!(bytes_read, 5);
        unsafe { data.set_len(5) };
        assert_eq!(data, DATA[0..5]);

        // Try to read more data than is available in the stream.
        let mut data = Vec::with_capacity(10);
        let bytes_read =
            stream.read_uninit(StreamReadOptions::empty(), data.spare_capacity_mut()).unwrap();
        assert_eq!(bytes_read, 7);
        unsafe { data.set_len(7) };
        assert_eq!(data, DATA[5..]);

        // Try to read at the end of the stream.
        let mut data = Vec::with_capacity(10);
        let bytes_read =
            stream.read_uninit(StreamReadOptions::empty(), data.spare_capacity_mut()).unwrap();
        assert_eq!(bytes_read, 0);
    }

    #[test]
    fn read_to_vec() {
        const DATA: &'static [u8] = b"vmo-contents";
        let vmo = zx::Vmo::create(DATA.len() as u64).unwrap();
        vmo.write(DATA, 0).unwrap();
        let stream = Stream::create(StreamOptions::MODE_READ, &vmo, 0).unwrap();

        let data = stream.read_to_vec(StreamReadOptions::empty(), DATA.len()).unwrap();
        assert_eq!(data, DATA);
    }

    #[test]
    fn read_at_uninit() {
        const DATA: &'static [u8] = b"vmo-contents";
        let vmo = zx::Vmo::create(DATA.len() as u64).unwrap();
        vmo.write(DATA, 0).unwrap();
        let stream = Stream::create(StreamOptions::MODE_READ, &vmo, 0).unwrap();

        // Read from the stream.
        let mut data = Vec::with_capacity(5);
        let bytes_read = stream
            .read_at_uninit(StreamReadOptions::empty(), 0, data.spare_capacity_mut())
            .unwrap();
        assert_eq!(bytes_read, 5);
        unsafe { data.set_len(5) };
        assert_eq!(data, DATA[0..5]);

        // Try to read beyond the end of the stream.
        let mut data = Vec::with_capacity(10);
        let bytes_read = stream
            .read_at_uninit(StreamReadOptions::empty(), 5, data.spare_capacity_mut())
            .unwrap();
        assert_eq!(bytes_read, 7);
        unsafe { data.set_len(7) };
        assert_eq!(data, DATA[5..]);

        // Try to read starting beyond the end of the stream.
        let mut data = Vec::with_capacity(10);
        let bytes_read = stream
            .read_at_uninit(StreamReadOptions::empty(), 20, data.spare_capacity_mut())
            .unwrap();
        assert_eq!(bytes_read, 0);
    }

    #[test]
    fn read_at_to_vec() {
        const DATA: &'static [u8] = b"vmo-contents";
        let vmo = zx::Vmo::create(DATA.len() as u64).unwrap();
        vmo.write(DATA, 0).unwrap();
        let stream = Stream::create(StreamOptions::MODE_READ, &vmo, 0).unwrap();

        let data = stream.read_at_to_vec(StreamReadOptions::empty(), 5, DATA.len()).unwrap();
        assert_eq!(data, DATA[5..]);
    }

    #[test]
    fn write() {
        const DATA: &'static [u8] = b"vmo-contents";
        let vmo = zx::Vmo::create_with_opts(zx::VmoOptions::UNBOUNDED, 0).unwrap();
        let stream =
            Stream::create(StreamOptions::MODE_READ | StreamOptions::MODE_WRITE, &vmo, 0).unwrap();

        let bytes_written = stream.write(zx::StreamWriteOptions::empty(), DATA).unwrap();
        assert_eq!(bytes_written, DATA.len());

        let data = stream.read_at_to_vec(StreamReadOptions::empty(), 0, DATA.len()).unwrap();
        assert_eq!(data, DATA);
    }

    #[test]
    fn write_at() {
        const DATA: &'static [u8] = b"vmo-contents";
        let vmo = zx::Vmo::create_with_opts(zx::VmoOptions::UNBOUNDED, 0).unwrap();
        let stream =
            Stream::create(StreamOptions::MODE_READ | StreamOptions::MODE_WRITE, &vmo, 0).unwrap();

        let bytes_written =
            stream.write_at(zx::StreamWriteOptions::empty(), 0, &DATA[0..3]).unwrap();
        assert_eq!(bytes_written, 3);

        let bytes_written =
            stream.write_at(zx::StreamWriteOptions::empty(), 3, &DATA[3..]).unwrap();
        assert_eq!(bytes_written, DATA.len() - 3);

        let data = stream.read_at_to_vec(StreamReadOptions::empty(), 0, DATA.len()).unwrap();
        assert_eq!(data, DATA);
    }

    #[test]
    fn std_io_read_write_seek() {
        const DATA: &'static str = "stream-contents";
        let vmo = zx::Vmo::create_with_opts(zx::VmoOptions::UNBOUNDED, 0).unwrap();
        let mut stream =
            Stream::create(StreamOptions::MODE_READ | StreamOptions::MODE_WRITE, &vmo, 0).unwrap();

        std::io::Write::write_all(&mut stream, DATA.as_bytes()).unwrap();
        assert_eq!(std::io::Seek::stream_position(&mut stream).unwrap(), DATA.len() as u64);
        std::io::Seek::rewind(&mut stream).unwrap();
        assert_eq!(std::io::read_to_string(&mut stream).unwrap(), DATA);
        assert_eq!(std::io::Seek::stream_position(&mut stream).unwrap(), DATA.len() as u64);
    }

    #[test]
    fn std_io_read_vectored() {
        const DATA: &'static [u8] = b"stream-contents";
        let vmo = zx::Vmo::create_with_opts(zx::VmoOptions::UNBOUNDED, 0).unwrap();
        let mut stream =
            Stream::create(StreamOptions::MODE_READ | StreamOptions::MODE_WRITE, &vmo, 0).unwrap();
        assert_eq!(stream.write(StreamWriteOptions::empty(), DATA).unwrap(), DATA.len());
        std::io::Seek::rewind(&mut stream).unwrap();

        let mut buf1 = [0; 6];
        let mut buf2 = [0; 1];
        let mut buf3 = [0; 8];
        let mut bufs = [
            std::io::IoSliceMut::new(&mut buf1),
            std::io::IoSliceMut::new(&mut buf2),
            std::io::IoSliceMut::new(&mut buf3),
        ];
        assert_eq!(std::io::Read::read_vectored(&mut stream, &mut bufs).unwrap(), DATA.len());
        assert_eq!(buf1, DATA[0..6]);
        assert_eq!(buf2, DATA[6..7]);
        assert_eq!(buf3, DATA[7..]);
    }

    #[test]
    fn std_io_write_vectored() {
        let vmo = zx::Vmo::create_with_opts(zx::VmoOptions::UNBOUNDED, 0).unwrap();
        let mut stream =
            Stream::create(StreamOptions::MODE_READ | StreamOptions::MODE_WRITE, &vmo, 0).unwrap();

        let bufs = [
            std::io::IoSlice::new(b"stream"),
            std::io::IoSlice::new(b"-"),
            std::io::IoSlice::new(b"contents"),
        ];
        assert_eq!(std::io::Write::write_vectored(&mut stream, &bufs).unwrap(), 15);
        std::io::Seek::rewind(&mut stream).unwrap();
        assert_eq!(stream.read_to_vec(StreamReadOptions::empty(), 15).unwrap(), b"stream-contents");
    }
}
