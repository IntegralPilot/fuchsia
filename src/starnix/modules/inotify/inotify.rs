// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::mm::MemoryAccessorExt;
use starnix_core::task::{CurrentTask, EventHandler, Kernel, WaitCanceler, WaitQueue, Waiter};
use starnix_core::vfs::buffers::{InputBuffer, OutputBuffer};
use starnix_core::vfs::{
    Anon, DirEntryHandle, FileHandle, FileObject, FileObjectState, FileOps, FsStr, FsString,
    WdNumber, fileops_impl_nonseekable, fileops_impl_noop_sync,
};
use starnix_sync::{InotifyStateLock, LockDepMutex};
use starnix_syscalls::{SUCCESS, SyscallArg, SyscallResult};
use starnix_uapi::arc_key::WeakKey;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::FileMode;
use starnix_uapi::inotify_mask::InotifyMask;
use starnix_uapi::math::round_up_to_increment;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::user_address::{UserAddress, UserRef};
use starnix_uapi::vfs::FdEvents;
use starnix_uapi::{FIONREAD, errno, error, inotify_event};
use std::collections::{HashMap, VecDeque};
use std::mem::size_of;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use zerocopy::IntoBytes;

const DATA_SIZE: usize = size_of::<inotify_event>();

// InotifyFileObject represents an inotify instance created by inotify_init(2) or inotify_init1(2).
pub struct InotifyFileObject {
    state: LockDepMutex<InotifyState, InotifyStateLock>,
}

struct InotifyState {
    events: InotifyEventQueue,

    watches: HashMap<WdNumber, DirEntryHandle>,

    // Last created WdNumber, stored as raw i32. WdNumber's are unique per inotify instance.
    last_watch_id: i32,
}

#[derive(Default)]
struct InotifyEventQueue {
    // queue can contain max_queued_events inotify events, plus one optional IN_Q_OVERFLOW event
    // if more events arrive.
    queue: VecDeque<InotifyEvent>,

    // Waiters to notify of new inotify events.
    waiters: WaitQueue,

    // Total size of InotifyEvent objects in queue, when serialized into inotify_event.
    size_bytes: usize,

    // This value is copied from /proc/sys/fs/inotify/max_queued_events on creation and is
    // constant afterwards, even if the proc file is modified.
    max_queued_events: usize,
}

// Serialized to inotify_event, see inotify(7).
#[derive(Debug, PartialEq, Eq)]
struct InotifyEvent {
    watch_id: WdNumber,

    mask: InotifyMask,

    cookie: u32,

    name: FsString,
}

impl InotifyState {
    fn next_watch_id(&mut self) -> WdNumber {
        self.last_watch_id += 1;
        WdNumber::from_raw(self.last_watch_id)
    }
}

impl InotifyFileObject {
    /// Allocate a new, empty inotify instance.
    pub fn new_file(current_task: &CurrentTask, non_blocking: bool) -> FileHandle {
        let flags =
            OpenFlags::RDONLY | if non_blocking { OpenFlags::NONBLOCK } else { OpenFlags::empty() };
        let max_queued_events =
            current_task.kernel().system_limits.inotify.max_queued_events.load(Ordering::Relaxed);
        assert!(max_queued_events >= 0);
        Anon::new_private_file(
            current_task,
            Box::new(InotifyFileObject {
                state: InotifyState {
                    events: InotifyEventQueue::new_with_max(max_queued_events as usize),
                    watches: Default::default(),
                    last_watch_id: 0,
                }
                .into(),
            }),
            flags,
            "inotify",
        )
    }

    /// Adds a watch to the inotify instance.
    ///
    /// Attaches an InotifyWatcher to the DirEntry's FsNode.
    /// Inotify keeps the DirEntryHandle in case it is evicted from dcache.
    pub fn add_watch(
        &self,
        dir_entry: DirEntryHandle,
        mask: InotifyMask,
        inotify_file: &FileHandle,
    ) -> Result<WdNumber, Errno> {
        let weak_key = WeakKey::from(inotify_file);
        if let Some(watch_id) = dir_entry.node.ensure_watchers().maybe_update(mask, &weak_key)? {
            return Ok(watch_id);
        }

        let watch_id;
        {
            let mut state = self.state.lock();
            watch_id = state.next_watch_id();
            state.watches.insert(watch_id, dir_entry.clone());
        }
        dir_entry.node.ensure_watchers().add(mask, watch_id, weak_key);
        Ok(watch_id)
    }

    /// Removes a watch to the inotify instance.
    ///
    /// Detaches the corresponding InotifyWatcher from FsNode.
    pub fn remove_watch(&self, watch_id: WdNumber, file: &FileHandle) -> Result<(), Errno> {
        let dir_entry;
        {
            let mut state = self.state.lock();
            dir_entry = state.watches.remove(&watch_id).ok_or_else(|| errno!(EINVAL))?;
            state.events.enqueue(InotifyEvent::new(
                watch_id,
                InotifyMask::IGNORED,
                0,
                FsString::default(),
            ));
        }
        dir_entry.node.ensure_watchers().remove(&WeakKey::from(file));
        Ok(())
    }

    fn notify(
        &self,
        watch_id: WdNumber,
        event_mask: Option<InotifyMask>,
        cookie: u32,
        name: &FsStr,
        remove_watcher_after_notify: bool,
    ) {
        // Holds a DirEntry pending deletion to be dropped after releasing the state mutex.
        #[allow(clippy::collection_is_never_read)]
        let _dir_entry: Option<DirEntryHandle>;
        {
            let mut state = self.state.lock();
            if let Some(event_mask) = event_mask {
                state.events.enqueue(InotifyEvent::new(
                    watch_id,
                    event_mask,
                    cookie,
                    name.to_owned(),
                ));
            }
            if remove_watcher_after_notify {
                _dir_entry = state.watches.remove(&watch_id);
                state.events.enqueue(InotifyEvent::new(
                    watch_id,
                    InotifyMask::IGNORED,
                    0,
                    FsString::default(),
                ));
            }
        }
    }

    fn available(&self) -> usize {
        let state = self.state.lock();
        state.events.size_bytes
    }
}

impl FileOps for InotifyFileObject {
    fileops_impl_nonseekable!();
    fileops_impl_noop_sync!();

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        offset: usize,
        _data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        debug_assert!(offset == 0);
        error!(EINVAL)
    }

    fn read(
        &self,
        file: &FileObject,
        current_task: &CurrentTask,
        offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        debug_assert!(offset == 0);
        file.blocking_op(current_task, FdEvents::POLLIN | FdEvents::POLLHUP, None, || {
            let mut state = self.state.lock();
            if let Some(front) = state.events.front() {
                if data.available() < front.size() {
                    return error!(EINVAL);
                }
            } else {
                return error!(EAGAIN);
            }

            let mut bytes_read: usize = 0;
            while let Some(front) = state.events.front() {
                if data.available() < front.size() {
                    break;
                }
                // Linux always dequeues an available event as long as there's enough buffer space to
                // copy it out, even if the copy below fails. Emulate this behaviour.
                bytes_read += state.events.dequeue().unwrap().write_to(data)?;
            }
            Ok(bytes_read)
        })
    }

    fn ioctl(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        request: u32,
        arg: SyscallArg,
    ) -> Result<SyscallResult, Errno> {
        let user_addr = UserAddress::from(arg);
        match request {
            FIONREAD => {
                let addr = UserRef::<i32>::new(user_addr);
                let size = i32::try_from(self.available()).unwrap_or(i32::MAX);
                current_task.write_object(addr, &size).map(|_| SUCCESS)
            }
            _ => error!(ENOTTY),
        }
    }

    fn wait_async(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        waiter: &Waiter,
        events: FdEvents,
        handler: EventHandler,
    ) -> Option<WaitCanceler> {
        Some(self.state.lock().events.waiters.wait_async_fd_events(waiter, events, handler))
    }

    fn query_events(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
    ) -> Result<FdEvents, Errno> {
        if self.available() > 0 { Ok(FdEvents::POLLIN) } else { Ok(FdEvents::empty()) }
    }

    fn close(self: Box<Self>, file: &FileObjectState, _current_task: &CurrentTask) {
        let dir_entries = {
            let mut state = self.state.lock();
            state.watches.drain().map(|(_key, value)| value).collect::<Vec<_>>()
        };

        for dir_entry in dir_entries {
            dir_entry.node.ensure_watchers().remove_by_ref(&file.weak_handle);
        }
    }

    fn extra_fdinfo(&self, file: &FileHandle, _current_task: &CurrentTask) -> Option<FsString> {
        let state = self.state.lock();
        let mut info = String::new();
        for dir_entry in state.watches.values() {
            let ino = dir_entry.node.ino;
            let sdev = dir_entry.node.fs().dev_id;
            if let Some(watcher) = dir_entry.node.ensure_watchers().get(&WeakKey::from(file)) {
                let wd = watcher.watch_id;
                let mask = watcher.mask;
                info.push_str(&format!(
                    "inotify wd:{} ino:{:x} sdev:{:x} mask:{:x}\n",
                    wd.raw(),
                    ino,
                    sdev.bits(),
                    mask.bits()
                ));
            }
        }
        Some(info.into())
    }
}

impl InotifyEventQueue {
    fn new_with_max(max_queued_events: usize) -> Self {
        InotifyEventQueue {
            queue: Default::default(),
            waiters: Default::default(),
            size_bytes: 0,
            max_queued_events,
        }
    }

    fn enqueue(&mut self, mut event: InotifyEvent) {
        if self.queue.len() > self.max_queued_events {
            return;
        }
        if self.queue.len() == self.max_queued_events {
            // If this event will overflow the queue, discard it and enqueue IN_Q_OVERFLOW instead.
            event = InotifyEvent::new(
                WdNumber::from_raw(-1),
                InotifyMask::Q_OVERFLOW,
                0,
                FsString::default(),
            );
        }
        if Some(&event) == self.queue.back() {
            // From https://man7.org/linux/man-pages/man7/inotify.7.html
            // If successive output inotify events produced on the inotify file
            // descriptor are identical (same wd, mask, cookie, and name), then
            // they are coalesced into a single event if the older event has not
            // yet been read.
            return;
        }
        self.size_bytes += event.size();
        self.queue.push_back(event);
        self.waiters.notify_fd_events(FdEvents::POLLIN);
    }

    fn front(&self) -> Option<&InotifyEvent> {
        self.queue.front()
    }

    fn dequeue(&mut self) -> Option<InotifyEvent> {
        let maybe_event = self.queue.pop_front();
        if let Some(event) = maybe_event.as_ref() {
            self.size_bytes -= event.size();
        }
        maybe_event
    }
}

impl InotifyEvent {
    // Creates a new InotifyEvent and pads name with at least 1 null-byte, aligned to DATA_SIZE.
    fn new(watch_id: WdNumber, mask: InotifyMask, cookie: u32, mut name: FsString) -> Self {
        if !name.is_empty() {
            let len = round_up_to_increment(name.len() + 1, DATA_SIZE)
                .expect("padded name should not overflow");
            name.resize(len, 0);
        }
        InotifyEvent { watch_id, mask, cookie, name }
    }

    fn size(&self) -> usize {
        DATA_SIZE + self.name.len()
    }

    fn write_to(&self, data: &mut dyn OutputBuffer) -> Result<usize, Errno> {
        let event = inotify_event {
            wd: self.watch_id.raw(),
            mask: self.mask.bits(),
            cookie: self.cookie,
            len: self.name.len().try_into().map_err(|_| errno!(EINVAL))?,
            // name field is zero-sized; the bytes for the name follows the struct linearly in memory.
            name: Default::default(),
        };

        let mut bytes_written = data.write(event.as_bytes())?;
        if !self.name.is_empty() {
            bytes_written += data.write(self.name.as_bytes())?;
        }

        debug_assert!(bytes_written == self.size());
        Ok(bytes_written)
    }
}

struct InotifyImpl {
    next_cookie: std::sync::atomic::AtomicU32,
}

impl starnix_core::vfs::inotify_hook::NotifyHook for InotifyImpl {
    fn notify(
        &self,
        watchers: &starnix_core::vfs::inotify_hook::InotifyWatchers,
        mut event_mask: InotifyMask,
        cookie: u32,
        name: &FsStr,
        mode: FileMode,
        is_dead: bool,
    ) {
        if cookie != 0 {
            // From https://man7.org/linux/man-pages/man7/inotify.7.html,
            // cookie is only used for rename events.
            debug_assert!(
                event_mask.contains(InotifyMask::MOVE_FROM)
                    || event_mask.contains(InotifyMask::MOVE_TO)
            );
        }
        // Clone inotify references so that we don't hold watchers lock when notifying.
        struct InotifyWatch {
            watch_id: WdNumber,
            file: FileHandle,
            should_send_event: bool,
            should_remove: bool,
        }
        let mut watches: Vec<InotifyWatch> = vec![];
        {
            let mut watchers = watchers.watchers.lock();
            watchers.retain(|inotify, watcher| {
                let mut should_remove = event_mask.contains(InotifyMask::DELETE_SELF);
                let should_send_event = watcher.mask.contains(event_mask)
                    && !(is_dead && watcher.mask.contains(InotifyMask::EXCL_UNLINK));
                if should_send_event {
                    should_remove = should_remove || watcher.mask.contains(InotifyMask::ONESHOT);
                }
                if should_send_event || should_remove {
                    if let Some(file) = inotify.0.upgrade() {
                        watches.push(InotifyWatch {
                            watch_id: watcher.watch_id,
                            file,
                            should_send_event,
                            should_remove,
                        });
                    } else {
                        should_remove = true;
                    }
                }
                !should_remove
            });
        }

        if mode.is_dir() {
            // Linux does not report IN_ISDIR with IN_DELETE_SELF or IN_MOVE_SELF for directories.
            if event_mask != InotifyMask::DELETE_SELF && event_mask != InotifyMask::MOVE_SELF {
                event_mask |= InotifyMask::ISDIR;
            }
        }

        for watch in watches {
            let inotify = watch
                .file
                .downcast_file::<InotifyFileObject>()
                .expect("failed to downcast to inotify");
            let mask = watch.should_send_event.then_some(event_mask);
            inotify.notify(watch.watch_id, mask, cookie, name, watch.should_remove);
        }
    }

    fn get_next_cookie(&self) -> u32 {
        let mut cookie = self.next_cookie.fetch_add(1, Ordering::Relaxed);
        while cookie == 0 {
            cookie = self.next_cookie.fetch_add(1, Ordering::Relaxed);
        }
        cookie
    }
}

pub fn inotify_init(kernel: &Kernel) {
    kernel.expando.get_or_init(|| {
        Arc::new(InotifyImpl { next_cookie: std::sync::atomic::AtomicU32::new(1) })
            as Arc<dyn starnix_core::vfs::inotify_hook::NotifyHook>
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use starnix_core::testing::spawn_kernel_and_run_with_pkgfs;
    use starnix_core::vfs::buffers::VecOutputBuffer;

    #[::fuchsia::test]
    fn inotify_event() {
        let event = InotifyEvent::new(WdNumber::from_raw(1), InotifyMask::ACCESS, 0, "".into());
        let mut buffer = VecOutputBuffer::new(DATA_SIZE + 100);
        let bytes_written = event.write_to(&mut buffer).expect("write_to buffer");

        assert_eq!(bytes_written, DATA_SIZE);
        assert_eq!(buffer.bytes_written(), DATA_SIZE);
    }

    #[::fuchsia::test]
    fn inotify_event_with_name() {
        // Create a name that is shorter than DATA_SIZE of 16.
        let name = "file1";
        let event = InotifyEvent::new(WdNumber::from_raw(1), InotifyMask::ACCESS, 0, name.into());
        let mut buffer = VecOutputBuffer::new(DATA_SIZE + 100);
        let bytes_written = event.write_to(&mut buffer).expect("write_to buffer");

        assert!(bytes_written > DATA_SIZE);
        assert_eq!(bytes_written % DATA_SIZE, 0);
        assert_eq!(buffer.bytes_written(), bytes_written);
    }

    #[::fuchsia::test]
    fn inotify_event_queue() {
        let mut event_queue = InotifyEventQueue::new_with_max(10);

        event_queue.enqueue(InotifyEvent::new(
            WdNumber::from_raw(1),
            InotifyMask::ACCESS,
            0,
            "".into(),
        ));

        assert_eq!(event_queue.queue.len(), 1);
        assert_eq!(event_queue.size_bytes, DATA_SIZE);

        let event = event_queue.dequeue();

        assert_eq!(
            event,
            Some(InotifyEvent::new(WdNumber::from_raw(1), InotifyMask::ACCESS, 0, "".into()))
        );
        assert_eq!(event_queue.queue.len(), 0);
        assert_eq!(event_queue.size_bytes, 0);
    }

    #[::fuchsia::test]
    fn inotify_event_queue_coalesce_events() {
        let mut event_queue = InotifyEventQueue::new_with_max(10);

        // Generate 2 identical events. They should combine into 1.
        event_queue.enqueue(InotifyEvent::new(
            WdNumber::from_raw(1),
            InotifyMask::ACCESS,
            0,
            "".into(),
        ));
        event_queue.enqueue(InotifyEvent::new(
            WdNumber::from_raw(1),
            InotifyMask::ACCESS,
            0,
            "".into(),
        ));

        assert_eq!(event_queue.queue.len(), 1);
    }

    #[::fuchsia::test]
    fn inotify_event_queue_max_queued_events() {
        let mut event_queue = InotifyEventQueue::new_with_max(1);

        // Generate 2 events, but the second event overflows the queue.
        event_queue.enqueue(InotifyEvent::new(
            WdNumber::from_raw(1),
            InotifyMask::ACCESS,
            0,
            "".into(),
        ));
        event_queue.enqueue(InotifyEvent::new(
            WdNumber::from_raw(1),
            InotifyMask::MODIFY,
            0,
            "".into(),
        ));

        assert_eq!(event_queue.queue.len(), 2);
        assert_eq!(event_queue.queue.get(0).unwrap().mask, InotifyMask::ACCESS);
        assert_eq!(event_queue.queue.get(1).unwrap().mask, InotifyMask::Q_OVERFLOW);

        // More events cannot be added to the queue.
        event_queue.enqueue(InotifyEvent::new(
            WdNumber::from_raw(1),
            InotifyMask::ATTRIB,
            0,
            "".into(),
        ));
        assert_eq!(event_queue.queue.len(), 2);
        assert_eq!(event_queue.queue.get(0).unwrap().mask, InotifyMask::ACCESS);
        assert_eq!(event_queue.queue.get(1).unwrap().mask, InotifyMask::Q_OVERFLOW);

        // Dequeue 1 event.
        let _event = event_queue.dequeue();
        assert_eq!(event_queue.queue.len(), 1);

        // More events still cannot make it to the queue. This is because they would cause an overflow,
        // but there is already a Q_OVERFLOW event in the queue so we do not enqueue another one.
        event_queue.enqueue(InotifyEvent::new(
            WdNumber::from_raw(1),
            InotifyMask::DELETE,
            0,
            "".into(),
        ));
        assert_eq!(event_queue.queue.len(), 1);
        assert_eq!(event_queue.queue.get(0).unwrap().mask, InotifyMask::Q_OVERFLOW);
    }

    #[::fuchsia::test]
    async fn notify_from_watchers() {
        spawn_kernel_and_run_with_pkgfs(async |current_task| {
            inotify_init(current_task.kernel());
            let file = InotifyFileObject::new_file(&current_task, true);
            let inotify =
                file.downcast_file::<InotifyFileObject>().expect("failed to downcast to inotify");

            // Use root as the watched directory.
            let root = current_task.fs().root().entry;
            assert!(inotify.add_watch(root.clone(), InotifyMask::ALL_EVENTS, &file).is_ok());

            {
                let watchers = root.node.ensure_watchers().watchers.lock();
                assert_eq!(watchers.len(), 1);
            }

            // Generate 1 event.
            root.node.notify(InotifyMask::ACCESS, 0, Default::default(), FileMode::IFREG, false);

            assert_eq!(inotify.available(), DATA_SIZE);
            {
                let state = inotify.state.lock();
                assert_eq!(state.watches.len(), 1);
                assert_eq!(state.events.queue.len(), 1);
            }

            // Generate another event.
            root.node.notify(InotifyMask::ATTRIB, 0, Default::default(), FileMode::IFREG, false);

            assert_eq!(inotify.available(), DATA_SIZE * 2);
            {
                let state = inotify.state.lock();
                assert_eq!(state.events.queue.len(), 2);
            }

            // Read 1 event.
            let mut buffer = VecOutputBuffer::new(DATA_SIZE);
            let bytes_read = file.read(&current_task, &mut buffer).expect("read into buffer");

            assert_eq!(bytes_read, DATA_SIZE);
            assert_eq!(inotify.available(), DATA_SIZE);
            {
                let state = inotify.state.lock();
                assert_eq!(state.events.queue.len(), 1);
            }

            // Read other event.
            buffer.reset();
            let bytes_read = file.read(&current_task, &mut buffer).expect("read into buffer");

            assert_eq!(bytes_read, DATA_SIZE);
            assert_eq!(inotify.available(), 0);
            {
                let state = inotify.state.lock();
                assert_eq!(state.events.queue.len(), 0);
            }
        })
        .await;
    }

    #[::fuchsia::test]
    async fn notify_deletion_from_watchers() {
        spawn_kernel_and_run_with_pkgfs(async |current_task| {
            inotify_init(current_task.kernel());
            let file = InotifyFileObject::new_file(&current_task, true);
            let inotify =
                file.downcast_file::<InotifyFileObject>().expect("failed to downcast to inotify");

            // Use root as the watched directory.
            let root = current_task.fs().root().entry;
            assert!(inotify.add_watch(root.clone(), InotifyMask::ALL_EVENTS, &file).is_ok());

            {
                let watchers = root.node.ensure_watchers().watchers.lock();
                assert_eq!(watchers.len(), 1);
            }

            root.node.notify(
                InotifyMask::DELETE_SELF,
                0,
                Default::default(),
                FileMode::IFREG,
                false,
            );

            {
                let watchers = root.node.ensure_watchers().watchers.lock();
                assert_eq!(watchers.len(), 0);
            }

            {
                let state = inotify.state.lock();
                assert_eq!(state.watches.len(), 0);
                assert_eq!(state.events.queue.len(), 2);

                assert_eq!(state.events.queue.get(0).unwrap().mask, InotifyMask::DELETE_SELF);
                assert_eq!(state.events.queue.get(1).unwrap().mask, InotifyMask::IGNORED);
            }
        })
        .await;
    }

    #[::fuchsia::test]
    async fn notify_deletion_without_delete_self_mask() {
        spawn_kernel_and_run_with_pkgfs(async |current_task| {
            inotify_init(current_task.kernel());
            let file = InotifyFileObject::new_file(&current_task, true);
            let inotify =
                file.downcast_file::<InotifyFileObject>().expect("failed to downcast to inotify");

            let root = current_task.fs().root().entry;
            // Watch for CREATE/DELETE/MODIFY without DELETE_SELF (like Java LinuxWatchService).
            assert!(
                inotify
                    .add_watch(
                        root.clone(),
                        InotifyMask::CREATE | InotifyMask::DELETE | InotifyMask::MODIFY,
                        &file
                    )
                    .is_ok()
            );

            root.node.notify(
                InotifyMask::DELETE_SELF,
                0,
                Default::default(),
                FileMode::IFDIR,
                false,
            );

            {
                let watchers = root.node.ensure_watchers().watchers.lock();
                assert_eq!(watchers.len(), 0);
            }

            {
                let state = inotify.state.lock();
                assert_eq!(state.watches.len(), 0);
                assert_eq!(state.events.queue.len(), 1);
                assert_eq!(state.events.queue.get(0).unwrap().mask, InotifyMask::IGNORED);
            }
        })
        .await;
    }

    #[::fuchsia::test]
    async fn inotify_on_same_file() {
        spawn_kernel_and_run_with_pkgfs(async |current_task| {
            inotify_init(current_task.kernel());
            let file = InotifyFileObject::new_file(&current_task, true);
            let file_key = WeakKey::from(&file);
            let inotify =
                file.downcast_file::<InotifyFileObject>().expect("failed to downcast to inotify");

            // Use root as the watched directory.
            let root = current_task.fs().root().entry;

            // Cannot add with both MASK_ADD and MASK_CREATE.
            assert!(
                inotify
                    .add_watch(
                        root.clone(),
                        InotifyMask::MODIFY | InotifyMask::MASK_ADD | InotifyMask::MASK_CREATE,
                        &file
                    )
                    .is_err()
            );

            assert!(
                inotify
                    .add_watch(root.clone(), InotifyMask::MODIFY | InotifyMask::MASK_CREATE, &file)
                    .is_ok()
            );

            {
                let watchers = root.node.ensure_watchers().watchers.lock();
                assert_eq!(watchers.len(), 1);
                assert!(watchers.get(&file_key).unwrap().mask.contains(InotifyMask::MODIFY));
            }

            // Replaces existing mask.
            assert!(inotify.add_watch(root.clone(), InotifyMask::ACCESS, &file).is_ok());

            {
                let watchers = root.node.ensure_watchers().watchers.lock();
                assert_eq!(watchers.len(), 1);
                assert!(watchers.get(&file_key).unwrap().mask.contains(InotifyMask::ACCESS));
                assert!(!watchers.get(&file_key).unwrap().mask.contains(InotifyMask::MODIFY));
            }

            // Merges with existing mask.
            assert!(
                inotify
                    .add_watch(root.clone(), InotifyMask::MODIFY | InotifyMask::MASK_ADD, &file)
                    .is_ok()
            );

            {
                let watchers = root.node.ensure_watchers().watchers.lock();
                assert_eq!(watchers.len(), 1);
                assert!(watchers.get(&file_key).unwrap().mask.contains(InotifyMask::ACCESS));
                assert!(watchers.get(&file_key).unwrap().mask.contains(InotifyMask::MODIFY));
            }
        })
        .await;
    }

    #[::fuchsia::test]
    async fn coalesce_events() {
        spawn_kernel_and_run_with_pkgfs(async |current_task| {
            inotify_init(current_task.kernel());
            let file = InotifyFileObject::new_file(&current_task, true);
            let inotify =
                file.downcast_file::<InotifyFileObject>().expect("failed to downcast to inotify");

            // Use root as the watched directory.
            let root = current_task.fs().root().entry;
            assert!(inotify.add_watch(root.clone(), InotifyMask::ALL_EVENTS, &file).is_ok());

            {
                let watchers = root.node.ensure_watchers().watchers.lock();
                assert_eq!(watchers.len(), 1);
            }

            // Generate 2 identical events. They should combine into 1.
            root.node.notify(InotifyMask::ACCESS, 0, Default::default(), FileMode::IFREG, false);
            root.node.notify(InotifyMask::ACCESS, 0, Default::default(), FileMode::IFREG, false);

            assert_eq!(inotify.available(), DATA_SIZE);
            {
                let state = inotify.state.lock();
                assert_eq!(state.watches.len(), 1);
                assert_eq!(state.events.queue.len(), 1);
            }
        })
        .await;
    }
}
