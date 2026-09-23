// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Stream-based Fuchsia VFS directory watcher

#![deny(missing_docs)]

use flex_client::{MessageBuf, ProxyHasDomain};
use flex_fuchsia_io as fio;
use futures::stream::{FusedStream, Stream};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};
use thiserror::Error;

#[cfg(not(feature = "fdomain"))]
use fuchsia_async as fasync;

#[derive(Debug, Error, Clone)]
#[allow(missing_docs)]
pub enum WatcherCreateError {
    #[error("while sending watch request: {0}")]
    SendWatchRequest(#[source] fidl::Error),

    #[error("watch failed with status: {0}")]
    WatchError(#[source] zx_status::Status),

    #[error("while converting client end to fasync channel: {0}")]
    ChannelConversion(#[source] zx_status::Status),
}

#[derive(Debug, Error)]
#[cfg_attr(not(feature = "fdomain"), derive(Eq, PartialEq))]
#[allow(missing_docs)]
pub enum WatcherStreamError {
    #[cfg(not(feature = "fdomain"))]
    #[error("read from watch channel failed with status: {0}")]
    ChannelRead(#[from] zx_status::Status),
    #[cfg(feature = "fdomain")]
    #[error("read from watch channel failed: {0}")]
    ChannelRead(#[from] flex_client::Error),
}

/// Describes the type of event that occurred in the directory being watched.
#[repr(C)]
#[derive(Copy, Clone, Eq, PartialEq)]
pub struct WatchEvent(fio::WatchEvent);

impl WatchEvent {
    /// The directory being watched has been deleted. The name returned for this event
    /// will be `.` (dot), as it is referring to the directory itself.
    pub const DELETED: Self = Self(fio::WatchEvent::Deleted);
    /// A file was added.
    pub const ADD_FILE: Self = Self(fio::WatchEvent::Added);
    /// A file was removed.
    pub const REMOVE_FILE: Self = Self(fio::WatchEvent::Removed);
    /// A file existed at the time the Watcher was created.
    pub const EXISTING: Self = Self(fio::WatchEvent::Existing);
    /// All existing files have been enumerated.
    pub const IDLE: Self = Self(fio::WatchEvent::Idle);

    const fn assoc_const_name(&self) -> &'static str {
        match self.0 {
            fio::WatchEvent::Deleted => "DELETED",
            fio::WatchEvent::Added => "ADD_FILE",
            fio::WatchEvent::Removed => "REMOVE_FILE",
            fio::WatchEvent::Existing => "EXISTING",
            fio::WatchEvent::Idle => "IDLE",
        }
    }
}

impl std::fmt::Debug for WatchEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WatchEvent({})", self.assoc_const_name())
    }
}

/// A message containing a `WatchEvent` and the filename (relative to the directory being watched)
/// that triggered the event.
#[derive(Debug, Eq, PartialEq)]
pub struct WatchMessage {
    /// The event that occurred.
    pub event: WatchEvent,
    /// The filename that triggered the message.
    pub filename: PathBuf,
}

#[derive(Debug, Eq, PartialEq)]
enum WatcherState {
    Watching,
    TerminateOnNextPoll,
    Terminated,
}

/// Provides a Stream of WatchMessages corresponding to filesystem events for a given directory.
/// After receiving an error, the stream will return the error, and then will terminate. After it's
/// terminated, the stream is fused and will continue to return None when polled.
#[derive(Debug)]
#[must_use = "futures/streams must be polled"]
pub struct Watcher {
    ch: flex_client::AsyncChannel,
    // If idx >= buf.bytes().len(), you must call reset_buf() before get_next_msg().
    buf: MessageBuf,
    idx: usize,
    state: WatcherState,
}

impl Unpin for Watcher {}

impl Watcher {
    /// Creates a new `Watcher` for the directory given by `dir`.
    pub async fn new(dir: &fio::DirectoryProxy) -> Result<Watcher, WatcherCreateError> {
        Self::new_with_mask(dir, fio::WatchMask::all()).await
    }

    /// Creates a new `Watcher` for the directory given by `dir`, only returning events specified
    /// by `mask`.
    pub async fn new_with_mask(
        dir: &fio::DirectoryProxy,
        mask: fio::WatchMask,
    ) -> Result<Watcher, WatcherCreateError> {
        let (client_end, server_end) = dir.domain().create_endpoints();
        let options = 0u32;
        let status = dir
            .watch(mask, options, server_end)
            .await
            .map_err(WatcherCreateError::SendWatchRequest)?;
        zx_status::Status::ok(status).map_err(WatcherCreateError::WatchError)?;
        let mut buf = MessageBuf::new();
        buf.ensure_capacity_bytes(fio::MAX_BUF as usize);
        Ok(Watcher {
            #[cfg(not(feature = "fdomain"))]
            ch: fasync::Channel::from_channel(client_end.into_channel()),
            #[cfg(feature = "fdomain")]
            ch: client_end.into_channel(),
            buf,
            idx: 0,
            state: WatcherState::Watching,
        })
    }

    fn reset_buf(&mut self) {
        self.idx = 0;
        self.buf.clear();
    }

    fn get_next_msg(&mut self) -> WatchMessage {
        assert!(self.idx < self.buf.bytes().len());
        let next_msg = VfsWatchMsg::from_raw(&self.buf.bytes()[self.idx..])
            .expect("Invalid buffer received by Watcher!");
        self.idx += next_msg.len();

        let mut pathbuf = PathBuf::new();
        pathbuf.push(OsStr::from_bytes(next_msg.name()));
        let event = next_msg.event();
        WatchMessage { event, filename: pathbuf }
    }
}

impl FusedStream for Watcher {
    fn is_terminated(&self) -> bool {
        self.state == WatcherState::Terminated
    }
}

impl Stream for Watcher {
    type Item = Result<WatchMessage, WatcherStreamError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        // Once this stream has hit an error, it's likely unrecoverable at this level and should be
        // closed. Clients can attempt to recover by creating a new Watcher.
        if this.state == WatcherState::TerminateOnNextPoll {
            this.state = WatcherState::Terminated;
        }
        if this.state == WatcherState::Terminated {
            return Poll::Ready(None);
        }
        if this.idx >= this.buf.bytes().len() {
            this.reset_buf();
        }
        if this.idx == 0 {
            match this.ch.recv_from(cx, &mut this.buf) {
                Poll::Ready(Ok(())) => {}
                Poll::Ready(Err(e)) => {
                    self.state = WatcherState::TerminateOnNextPoll;
                    return Poll::Ready(Some(Err(e.into())));
                }
                Poll::Pending => return Poll::Pending,
            }
        }
        Poll::Ready(Some(Ok(this.get_next_msg())))
    }
}

#[repr(C)]
#[derive(Default)]
struct IncompleteArrayField<T>(::std::marker::PhantomData<T>);
impl<T> IncompleteArrayField<T> {
    #[inline]
    pub unsafe fn as_ptr(&self) -> *const T {
        unsafe { ::std::mem::transmute(self) }
    }
    #[inline]
    pub unsafe fn as_slice(&self, len: usize) -> &[T] {
        unsafe { ::std::slice::from_raw_parts(self.as_ptr(), len) }
    }
}
impl<T> ::std::fmt::Debug for IncompleteArrayField<T> {
    fn fmt(&self, fmt: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        fmt.write_str("IncompleteArrayField")
    }
}

#[repr(C)]
#[derive(Debug)]
struct vfs_watch_msg_t {
    event: fio::WatchEvent,
    len: u8,
    name: IncompleteArrayField<u8>,
}

#[derive(Debug)]
struct VfsWatchMsg<'a> {
    inner: &'a vfs_watch_msg_t,
}

impl<'a> VfsWatchMsg<'a> {
    fn from_raw(buf: &'a [u8]) -> Option<VfsWatchMsg<'a>> {
        if buf.len() < ::std::mem::size_of::<vfs_watch_msg_t>() {
            return None;
        }
        // This is safe as long as the buffer is at least as large as a vfs_watch_msg_t, which we
        // just verified. Further, we verify that the buffer has enough bytes to hold the
        // "incomplete array field" member.
        let m = unsafe { VfsWatchMsg { inner: &*(buf.as_ptr() as *const vfs_watch_msg_t) } };
        if buf.len() < ::std::mem::size_of::<vfs_watch_msg_t>() + m.namelen() {
            return None;
        }
        Some(m)
    }

    fn len(&self) -> usize {
        ::std::mem::size_of::<vfs_watch_msg_t>() + self.namelen()
    }

    fn event(&self) -> WatchEvent {
        WatchEvent(self.inner.event)
    }

    fn namelen(&self) -> usize {
        self.inner.len as usize
    }

    fn name(&self) -> &'a [u8] {
        // This is safe because we verified during construction that the inner name field has at
        // least namelen() bytes in it.
        unsafe { self.inner.name.as_slice(self.namelen()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use fuchsia_async::{DurationExt, TimeoutExt};

    use futures::prelude::*;
    use std::fmt::Debug;
    use std::fs::File;
    use std::path::Path;
    use std::sync::Arc;
    use tempfile::tempdir;
    use vfs::ObjectRequestRef;
    use vfs::directory::dirents_sink;
    use vfs::directory::entry::{EntryInfo, GetEntryInfo};
    use vfs::directory::entry_container::{Directory, DirectoryWatcher};
    use vfs::directory::immutable::connection::ImmutableConnection;
    use vfs::directory::traversal_position::TraversalPosition;
    use vfs::execution_scope::ExecutionScope;
    use vfs::node::Node;

    fn one_step<'a, S, OK, ERR>(s: &'a mut S) -> impl Future<Output = OK> + 'a
    where
        S: Stream<Item = Result<OK, ERR>> + Unpin,
        ERR: Debug,
    {
        let f = s.next();
        let f = f.on_timeout(zx::MonotonicDuration::from_millis(500).after_now(), || {
            panic!("timeout waiting for watcher")
        });
        f.map(|next| {
            next.expect("the stream yielded no next item")
                .unwrap_or_else(|e| panic!("Error waiting for watcher: {:?}", e))
        })
    }

    #[fuchsia::test]
    async fn test_existing() {
        let tmp_dir = tempdir().unwrap();
        let _ = File::create(tmp_dir.path().join("file1")).unwrap();

        let dir = crate::directory::open_in_namespace(
            tmp_dir.path().to_str().unwrap(),
            fio::PERM_READABLE,
        )
        .unwrap();
        let mut w = Watcher::new(&dir).await.unwrap();

        let msg = one_step(&mut w).await;
        assert_eq!(WatchEvent::EXISTING, msg.event);
        assert_eq!(Path::new("."), msg.filename);

        let msg = one_step(&mut w).await;
        assert_eq!(WatchEvent::EXISTING, msg.event);
        assert_eq!(Path::new("file1"), msg.filename);

        let msg = one_step(&mut w).await;
        assert_eq!(WatchEvent::IDLE, msg.event);
    }

    #[fuchsia::test]
    async fn test_add() {
        let tmp_dir = tempdir().unwrap();

        let dir = crate::directory::open_in_namespace(
            tmp_dir.path().to_str().unwrap(),
            fio::PERM_READABLE,
        )
        .unwrap();
        let mut w = Watcher::new(&dir).await.unwrap();

        loop {
            let msg = one_step(&mut w).await;
            match msg.event {
                WatchEvent::EXISTING => continue,
                WatchEvent::IDLE => break,
                _ => panic!("Unexpected watch event!"),
            }
        }

        let _ = File::create(tmp_dir.path().join("file1")).unwrap();
        let msg = one_step(&mut w).await;
        assert_eq!(WatchEvent::ADD_FILE, msg.event);
        assert_eq!(Path::new("file1"), msg.filename);
    }

    #[fuchsia::test]
    async fn test_remove() {
        let tmp_dir = tempdir().unwrap();

        let filename = "file1";
        let filepath = tmp_dir.path().join(filename);
        let _ = File::create(&filepath).unwrap();

        let dir = crate::directory::open_in_namespace(
            tmp_dir.path().to_str().unwrap(),
            fio::PERM_READABLE,
        )
        .unwrap();
        let mut w = Watcher::new(&dir).await.unwrap();

        loop {
            let msg = one_step(&mut w).await;
            match msg.event {
                WatchEvent::EXISTING => continue,
                WatchEvent::IDLE => break,
                _ => panic!("Unexpected watch event!"),
            }
        }

        ::std::fs::remove_file(&filepath).unwrap();
        let msg = one_step(&mut w).await;
        assert_eq!(WatchEvent::REMOVE_FILE, msg.event);
        assert_eq!(Path::new(filename), msg.filename);
    }

    struct MockDirectory;

    impl MockDirectory {
        fn new() -> Arc<Self> {
            Arc::new(Self)
        }
    }

    impl GetEntryInfo for MockDirectory {
        fn entry_info(&self) -> EntryInfo {
            EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)
        }
    }

    impl Node for MockDirectory {
        async fn get_attributes(
            &self,
            _query: fio::NodeAttributesQuery,
        ) -> Result<fio::NodeAttributes2, zx::Status> {
            unimplemented!();
        }

        fn close(self: Arc<Self>) {}
    }

    impl Directory for MockDirectory {
        fn open(
            self: Arc<Self>,
            scope: ExecutionScope,
            _path: vfs::path::Path,
            flags: fio::Flags,
            object_request: ObjectRequestRef<'_>,
        ) -> Result<(), zx::Status> {
            object_request.take().create_connection_sync::<ImmutableConnection<_>, _>(
                scope,
                self.clone(),
                flags,
            );
            Ok(())
        }

        async fn read_dirents(
            &self,
            _pos: &TraversalPosition,
            _sink: Box<dyn dirents_sink::Sink>,
        ) -> Result<(TraversalPosition, Box<dyn dirents_sink::Sealed>), zx::Status> {
            unimplemented!("Not implemented");
        }

        fn register_watcher(
            self: Arc<Self>,
            _scope: ExecutionScope,
            _mask: fio::WatchMask,
            _watcher: DirectoryWatcher,
        ) -> Result<(), zx::Status> {
            // Don't do anything, just throw out the watcher, which should close the channel, to
            // generate a PEER_CLOSED error.
            Ok(())
        }

        fn unregister_watcher(self: Arc<Self>, _key: usize) {
            unimplemented!("Not implemented");
        }
    }

    #[fuchsia::test]
    async fn test_error() {
        let test_dir = MockDirectory::new();
        let client = vfs::directory::serve_read_only(test_dir, ExecutionScope::new());
        let mut w = Watcher::new(&client).await.unwrap();
        let msg = w.next().await.expect("the stream yielded no next item");
        assert!(!w.is_terminated());
        assert_matches!(msg, Err(WatcherStreamError::ChannelRead(zx::Status::PEER_CLOSED)));
        assert!(!w.is_terminated());
        assert_matches!(w.next().await, None);
        assert!(w.is_terminated());
    }
}
