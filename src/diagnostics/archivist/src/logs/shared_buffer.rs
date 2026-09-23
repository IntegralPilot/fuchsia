// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod cursor;

pub use cursor::{FilterCursor, FilterCursorStream};

use crate::identity::ComponentIdentity;
use crate::logs::repository::ARCHIVIST_MONIKER;
use crate::logs::stats::{LogStreamStats, SaturationCurve};
use derivative::Derivative;
use diagnostics_log_encoding::encode::add_dropped_count;
use diagnostics_log_encoding::{Header, TRACING_FORMAT_LOG_RECORD_TYPE};
use fidl_fuchsia_diagnostics::{ComponentSelector, StreamMode};
use fidl_fuchsia_logger::MAX_DATAGRAM_LEN_BYTES;
use fuchsia_async as fasync;
use fuchsia_async::condition::{Condition, ConditionGuard};
use fuchsia_inspect::Node;
use fuchsia_sync::Mutex;
use futures::channel::oneshot;
use log::debug;
use ring_buffer::{self, RingBuffer, ring_buffer_record_len};
use std::collections::VecDeque;
use std::mem::ManuallyDrop;
use std::ops::{Deref, DerefMut, Range};
use std::sync::{Arc, Weak};
use std::time::Duration;
use zerocopy::FromBytes;

// Aim to keep 25% of the buffer free. This is expressed as a fraction: numerator / denominator.
const SPACE_THRESHOLD_NUMERATOR: usize = 1;
const SPACE_THRESHOLD_DENOMINATOR: usize = 4;

// The default amount of time that Archivist will sleep for to reduce how often it wakes up to
// handle log messages.
const DEFAULT_SLEEP_TIME: Duration = Duration::from_millis(200);

pub fn create_ring_buffer(capacity: usize) -> ring_buffer::Reader {
    RingBuffer::create(calculate_real_size_given_desired_capacity(capacity))
}

fn calculate_real_size_given_desired_capacity(capacity: usize) -> usize {
    // We always keep spare space in the buffer so that we don't drop messages.  This is controlled
    // by SPACE_THRESHOLD_NUMERATOR & SPACE_THRESHOLD_DENOMINATOR.  Round up capacity so that
    // `capacity` reflects the actual amount of log data we can store.
    let page_size = zx::system_get_page_size() as usize;
    (capacity * SPACE_THRESHOLD_DENOMINATOR
        / (SPACE_THRESHOLD_DENOMINATOR - SPACE_THRESHOLD_NUMERATOR))
        .next_multiple_of(page_size)
}

const IOB_PEER_CLOSED_KEY_BASE: u64 = 0x8000_0000_0000_0000;

pub type OnInactive = Box<dyn Fn(Arc<ComponentIdentity>) + Send + Sync>;

pub struct SharedBuffer {
    inner: Condition<Inner>,

    // Sockets. This *must* be locked after `inner` and the lock *must* be dropped before
    // `InnerGuard` is dropped.
    sockets: Mutex<Slab<Socket>>,

    // Callback for when a container is inactive (i.e. no logs and no sockets).
    on_inactive: OnInactive,

    // The port that we use to service the sockets.
    port: zx::Port,

    // Event used for termination.
    event: zx::Event,

    // The thread that we use to service the sockets.
    socket_thread: Mutex<Option<std::thread::JoinHandle<()>>>,

    // The task responsible for monitoring the space in the IOBuffer and popping messages when it
    // gets full. It will also wake cursors whenever new data arrives.
    _buffer_monitor_task: fasync::Task<()>,

    /// Metric for tracking buffer saturation.
    pub(crate) saturation_curve: Arc<SaturationCurve>,
}

struct InnerGuard<'a> {
    buffer: &'a Arc<SharedBuffer>,

    guard: ManuallyDrop<ConditionGuard<'a, Inner>>,

    // The list of components that should be reported as inactive once the lock is dropped.
    on_inactive: Vec<Arc<ComponentIdentity>>,

    // If true, wake all the wakers registered with the ConditionGuard.
    wake: bool,
}

impl Drop for InnerGuard<'_> {
    fn drop(&mut self) {
        if self.wake {
            for waker in self.guard.drain_wakers() {
                waker.wake();
            }
        }
        // SAFETY: This is the only place we drop the guard.
        unsafe {
            ManuallyDrop::drop(&mut self.guard);
        }
        for identity in self.on_inactive.drain(..) {
            (*self.buffer.on_inactive)(identity);
        }
    }
}

impl Deref for InnerGuard<'_> {
    type Target = Inner;

    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl DerefMut for InnerGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

struct Inner {
    // The ring buffer.
    ring_buffer: Arc<RingBuffer>,

    // Registered containers.
    containers: Containers,

    // Socket thread message queue.
    thread_msg_queue: VecDeque<ThreadMessage>,

    // The index in the buffer that we have scanned for messages.
    last_scanned: u64,

    // The ID of the message that was last scanned.
    last_scanned_message_id: u64,

    // A copy of the tail index in the ring buffer.
    tail: u64,

    // The ID of the message at the tail.  This is just a counter that increments one per message.
    tail_message_id: u64,

    // The IOBuffer peers that we must watch.
    iob_peers: Slab<(ContainerId, zx::Iob)>,

    // True, when the buffer has been terminated.
    terminated: bool,
}

enum ThreadMessage {
    // The thread should terminate.
    Terminate,

    // Process all pending socket messages and report via the Sender when done.
    Flush(oneshot::Sender<()>),
}

pub struct SharedBufferOptions {
    // To reduce how often Archivist wakes when new messages are written, Archivist will sleep for
    // this time. This will impact how quickly messages show up via the cursors.
    pub sleep_time: Duration,
    /// The size of the circular buffer for the saturation curve.
    pub saturation_curve_size: usize,
}

impl Default for SharedBufferOptions {
    fn default() -> Self {
        Self { sleep_time: DEFAULT_SLEEP_TIME, saturation_curve_size: 254 }
    }
}

impl SharedBuffer {
    /// Returns a new shared buffer and the container for Archivist.
    pub fn new(
        ring_buffer: ring_buffer::Reader,
        on_inactive: OnInactive,
        options: SharedBufferOptions,
        inspect_node: &Node,
    ) -> Arc<Self> {
        let saturation_curve =
            Arc::new(SaturationCurve::new(inspect_node, options.saturation_curve_size));
        let this = Arc::new_cyclic(|weak: &Weak<Self>| Self {
            inner: Condition::new(Inner {
                ring_buffer: Arc::clone(&ring_buffer),
                containers: Containers::default(),
                thread_msg_queue: VecDeque::default(),
                last_scanned: 0,
                last_scanned_message_id: 0,
                tail: 0,
                tail_message_id: 0,
                iob_peers: Slab::default(),
                terminated: false,
            }),
            sockets: Mutex::new(Slab::default()),
            on_inactive,
            port: zx::Port::create(),
            event: zx::Event::create(),
            socket_thread: Mutex::default(),
            saturation_curve,
            _buffer_monitor_task: fasync::Task::spawn(Self::buffer_monitor_task(
                Weak::clone(weak),
                ring_buffer,
                options.sleep_time,
            )),
        });

        *this.socket_thread.lock() = Some({
            let this = Arc::clone(&this);
            std::thread::spawn(move || this.socket_thread(options.sleep_time))
        });
        this
    }

    pub fn new_container_buffer(
        self: &Arc<Self>,
        identity: Arc<ComponentIdentity>,
        stats: Arc<LogStreamStats>,
    ) -> ContainerBuffer {
        let mut inner = self.inner.lock();
        let Inner { containers, ring_buffer, .. } = &mut *inner;
        ContainerBuffer {
            shared_buffer: Arc::clone(self),
            container_id: containers.new_container(ring_buffer, Arc::clone(&identity), stats),
        }
    }

    pub async fn flush(&self) {
        let (sender, receiver) = oneshot::channel();
        self.inner.lock().thread_msg_queue.push_back(ThreadMessage::Flush(sender));
        self.event.signal(zx::Signals::empty(), zx::Signals::USER_0).unwrap();
        // Ignore failure if Archivist is shutting down.
        let _ = receiver.await;
    }

    /// Returns the number of registered containers in use by the buffer.
    #[cfg(test)]
    pub fn container_count(&self) -> usize {
        self.inner.lock().containers.len()
    }

    /// Terminates the socket thread. The socket thread will drain the sockets before terminating.
    /// Returns a future that may be awaited if the caller wants to wait for the socket thread to
    /// terminate.
    pub fn terminate(self: &Arc<Self>) -> impl Future<Output = ()> {
        {
            let mut inner = InnerGuard::new(self);
            self.flush_sockets(&mut inner);
            inner.thread_msg_queue.push_back(ThreadMessage::Terminate);
        }
        self.event.signal(zx::Signals::empty(), zx::Signals::USER_0).unwrap();
        let join_handle = self.socket_thread.lock().take().unwrap();
        fasync::unblock(|| {
            let _ = join_handle.join();
        })
    }

    /// Returns a cursor that will return messages that match selector. There will be a limited
    /// attempt to reorder messages by timestamp.  If no selectors are specified, *all* messages
    /// will be returned.
    pub fn cursor(
        self: &Arc<Self>,
        mode: StreamMode,
        selectors: Vec<ComponentSelector>,
    ) -> FilterCursor {
        InnerGuard::new(self).cursor(mode, selectors)
    }

    fn socket_thread(self: Arc<Self>, sleep_time: Duration) {
        const INTERRUPT_KEY: u64 = u64::MAX;
        let mut sockets_ready = Vec::new();
        let mut iob_peer_closed = Vec::new();
        let mut interrupt_needs_arming = true;
        let mut msg = None;

        loop {
            let mut deadline = if msg.is_some() {
                zx::MonotonicInstant::INFINITE_PAST
            } else {
                if interrupt_needs_arming {
                    self.event
                        .wait_async(
                            &self.port,
                            INTERRUPT_KEY,
                            zx::Signals::USER_0,
                            zx::WaitAsyncOpts::empty(),
                        )
                        .unwrap();
                    interrupt_needs_arming = false;
                }

                // Wait so that we're not constantly waking up for every log message that is queued.
                // Ignore errors here.
                let _ = self
                    .event
                    .wait_one(zx::Signals::USER_0, zx::MonotonicInstant::after(sleep_time.into()));
                zx::MonotonicInstant::INFINITE
            };

            // Gather the list of sockets that are ready to read.
            loop {
                match self.port.wait(deadline) {
                    Ok(packet) => {
                        if packet.key() == INTERRUPT_KEY {
                            interrupt_needs_arming = true;
                            // To maintain proper ordering, we must capture the message here whilst
                            // we are still gathering the list of sockets that are ready to read.
                            // If we wait till later, we introduce windows where we might miss a
                            // socket that should be read.
                            if msg.is_none() {
                                msg = self.inner.lock().thread_msg_queue.pop_front();
                            }
                        } else if packet.key() & IOB_PEER_CLOSED_KEY_BASE != 0 {
                            iob_peer_closed.push(packet.key() as u32);
                        } else {
                            sockets_ready.push(SocketId(packet.key() as u32))
                        }
                    }
                    Err(zx::Status::TIMED_OUT) => break,
                    Err(status) => panic!("port wait error: {status:?}"),
                }
                deadline = zx::MonotonicInstant::INFINITE_PAST;
            }

            let mut inner = InnerGuard::new(&self);

            if !iob_peer_closed.is_empty() {
                // See the comment on `is_active()` for why this is required.
                inner.update_message_ids(inner.ring_buffer.head());

                for iob_peer_closed in iob_peer_closed.drain(..) {
                    let container_id = inner.iob_peers.free(iob_peer_closed).0;
                    if let Some(container) = inner.containers.get_mut(container_id) {
                        container.iob_count -= 1;
                        if container.iob_count == 0 && !container.is_active() {
                            if container.should_free() {
                                inner.containers.free(container_id);
                            } else {
                                let identity = Arc::clone(&container.identity);
                                inner.on_inactive.push(identity);
                            }
                        }
                    }
                }
            }

            {
                let mut sockets = self.sockets.lock();
                for socket_id in sockets_ready.drain(..) {
                    inner.read_socket(&mut sockets, socket_id, |socket| {
                        socket
                            .socket
                            .wait_async(
                                &self.port,
                                socket_id.0 as u64,
                                zx::Signals::OBJECT_READABLE | zx::Signals::OBJECT_PEER_CLOSED,
                                zx::WaitAsyncOpts::empty(),
                            )
                            .unwrap();
                    });
                }
            }

            // Now that we've processed the sockets, we can process the message.
            if let Some(m) = msg.take() {
                match m {
                    ThreadMessage::Terminate => {
                        inner.terminated = true;
                        inner.wake = true;
                        return;
                    }
                    ThreadMessage::Flush(sender) => {
                        let _ = sender.send(());
                    }
                }

                // See if there's another message.
                msg = inner.thread_msg_queue.pop_front();
                if msg.is_none() {
                    // If there are no more messages, we must clear the signal so that we get
                    // notified when the next message arrives. This must be done whilst we are
                    // holding the lock.
                    self.event.signal(zx::Signals::USER_0, zx::Signals::empty()).unwrap();
                }
            }
        }
    }

    async fn buffer_monitor_task(
        this: Weak<Self>,
        mut ring_buffer: ring_buffer::Reader,
        sleep_time: Duration,
    ) {
        let mut last_head = 0;
        loop {
            // Sleep to limit how often we wake up.
            fasync::Timer::new(sleep_time).await;
            let head = ring_buffer.wait(last_head).await;
            let Some(this) = this.upgrade() else { return };
            let mut inner = InnerGuard::new(&this);
            inner.check_space(head);
            last_head = head;
        }
    }

    /// Flushes sockets and returns the head index.
    fn flush_sockets(&self, inner: &mut InnerGuard<'_>) -> u64 {
        let mut sockets = self.sockets.lock();
        let mut socket_id = 0;
        while let Some(next_id) = sockets.next_used(socket_id) {
            // The socket doesn't need to be rearmed here.
            inner.read_socket(&mut sockets, SocketId(next_id), |_| {});
            socket_id = next_id + 1;
        }
        let head = inner.ring_buffer.head();
        inner.update_message_ids(head);
        head
    }
}

impl Inner {
    // Returns list of containers that no longer have any messages.
    fn ingest(&mut self, msg: &[u8], container_id: ContainerId) {
        if msg.len() < std::mem::size_of::<Header>() {
            debug!("message too short ({})", msg.len());
            if let Some(container) = self.containers.get(container_id) {
                container.stats.increment_invalid(msg.len());
            }
            return;
        }

        let header = Header::read_from_bytes(&msg[..std::mem::size_of::<Header>()]).unwrap();

        // NOTE: Some tests send messages that are bigger than the header indicates. We ignore the
        // tail of any such messages.
        let msg_len = header.size_words() as usize * 8;

        // Check the correct type and size.
        if header.raw_type() != TRACING_FORMAT_LOG_RECORD_TYPE || msg.len() < msg_len {
            debug!("bad type or size ({}, {}, {})", header.raw_type(), msg_len, msg.len());
            if let Some(container) = self.containers.get(container_id) {
                container.stats.increment_invalid(msg.len());
            }
            return;
        }

        let Some(container) = self.containers.get_mut(container_id) else {
            return;
        };

        let mut data;
        let msg = if container.dropped_count > 0 {
            data = msg.to_vec();
            if !add_dropped_count(&mut data, container.dropped_count) {
                debug!("unable to add dropped count to invalid message");
                container.stats.increment_invalid(data.len());
                return;
            }
            &data
        } else {
            msg
        };

        if container.iob.write(Default::default(), 0, msg).is_err() {
            // We were unable to write the message to the buffer, most likely due to lack of
            // space. We drop this message and then we'll add to the dropped count for the next
            // message.
            container.dropped_count += 1
        } else {
            container.dropped_count = 0;
        }
    }

    /// Parses the first message within `range` returns (container_id, message, timestamp).
    ///
    /// # Panics
    ///
    /// This will panic if the ring buffer has been corrupted. Only the kernel and Archivist can
    /// write to the ring buffer and so we trust both not to corrupt the ring buffer.
    ///
    /// # Safety
    ///
    /// `range` *must* be within the written range of the ring buffer so that there is no concurrent
    /// write access to that range. The returned slice is only valid whilst this remains true.
    unsafe fn parse_message(
        &self,
        range: Range<u64>,
    ) -> (ContainerId, &[u8], Option<zx::BootInstant>) {
        let (tag, msg) = unsafe { self.ring_buffer.first_message_in(range) }
            .expect("Unable to read message from ring buffer");
        (
            ContainerId(tag as u32),
            msg,
            (msg.len() >= 16)
                .then(|| zx::BootInstant::from_nanos(i64::read_from_bytes(&msg[8..16]).unwrap())),
        )
    }
}

impl<'a> InnerGuard<'a> {
    fn new(buffer: &'a Arc<SharedBuffer>) -> Self {
        Self {
            buffer,
            guard: ManuallyDrop::new(buffer.inner.lock()),
            on_inactive: Vec::new(),
            wake: false,
        }
    }

    /// Pops a message and returns its total size. The caller should call `update_message_ids(head)`
    /// prior to calling this.
    ///
    /// NOTE: This will pop the oldest message in terms of when it was inserted which is *not*
    /// necessarily the message with the *oldest* timestamp because we might not have received the
    /// messages in perfect timestamp order. This should be close enough for all use cases we care
    /// about, and besides, the timestamps can't be trusted anyway.
    fn pop(&mut self, head: u64) -> Option<usize> {
        if head == self.tail {
            return None;
        }

        // SAFETY: There can be no concurrent writes between `tail..head` and the *only* place
        // we increment the tail index is just before we leave this function.
        let record_len = {
            let (container_id, message, _) = unsafe { self.parse_message(self.tail..head) };
            let record_len = ring_buffer_record_len(message.len());

            let container = self.containers.get_mut(container_id).unwrap();

            container.stats.increment_rolled_out(record_len);
            container.msg_ids.start += 1;
            if !container.is_active() {
                if container.should_free() {
                    self.containers.free(container_id);
                } else {
                    let identity = Arc::clone(&container.identity);
                    self.on_inactive.push(identity);
                }
            }

            record_len
        };

        // NOTE: This should go last. After incrementing `tail`, the `message` can be overwritten.
        self.ring_buffer.increment_tail(record_len);
        self.tail += record_len as u64;
        self.tail_message_id += 1;

        // The caller should have called `update_message_ids(head)` prior to calling this.
        assert!(self.last_scanned >= self.tail);

        Some(record_len)
    }

    /// Reads a socket. Calls `rearm` to rearm the socket once it has been drained.
    fn read_socket(
        &mut self,
        sockets: &mut Slab<Socket>,
        socket_id: SocketId,
        rearm: impl FnOnce(&mut Socket),
    ) {
        let Some(socket) = sockets.get_mut(socket_id.0) else { return };
        let container_id = socket.container_id;

        loop {
            self.check_space(self.ring_buffer.head());

            let mut data = Vec::with_capacity(MAX_DATAGRAM_LEN_BYTES as usize);

            // Read directly into the buffer leaving space for the header.
            let len = match socket.socket.read_uninit(data.spare_capacity_mut()) {
                Ok(d) => d.len(),
                Err(zx::Status::SHOULD_WAIT) => {
                    // The socket has been drained.
                    rearm(socket);
                    return;
                }
                Err(_) => break,
            };

            // SAFETY: `read_uninit` will have written to `len` bytes.
            unsafe {
                data.set_len(len);
            }

            let container = self.containers.get_mut(container_id).unwrap();
            if data.len() < 16 {
                container.stats.increment_invalid(data.len());
                continue;
            }

            let header = Header::read_from_bytes(&data[..std::mem::size_of::<Header>()]).unwrap();
            let msg_len = header.size_words() as usize * 8;
            if header.raw_type() != TRACING_FORMAT_LOG_RECORD_TYPE || msg_len != data.len() {
                debug!("bad type or size ({}, {}, {})", header.raw_type(), msg_len, data.len());
                container.stats.increment_invalid(data.len());
                continue;
            }

            if container.dropped_count > 0 && !add_dropped_count(&mut data, container.dropped_count)
            {
                debug!("unable to add dropped count to invalid message");
                container.stats.increment_invalid(data.len());
                continue;
            }

            if container.iob.write(Default::default(), 0, &data).is_err() {
                // We were unable to write the message to the buffer, most likely due to lack of
                // space. We drop this message and then we'll add to the dropped count for the next
                // message.
                container.dropped_count += 1
            } else {
                container.dropped_count = 0;
            }
        }

        // This path is taken when the socket should be closed.

        // See the comment on `is_active()` for why this is required.
        self.update_message_ids(self.ring_buffer.head());

        let container = self.containers.get_mut(container_id).unwrap();
        container.remove_socket(socket_id, sockets);
        if !container.is_active() {
            if container.should_free() {
                self.containers.free(container_id);
            } else {
                let identity = Arc::clone(&container.identity);
                self.on_inactive.push(identity);
            }
        }
    }

    /// Scans the ring buffer and updates `msg_ids` for the containers.
    fn update_message_ids(&mut self, head: u64) {
        while self.last_scanned < head {
            // SAFETY: This is safe because `head` must be within the ring buffer range and we make
            // sure that `self.last_scanned` is always >= `tail` in `pop()`.
            let (container_id, message, _) = unsafe { self.parse_message(self.last_scanned..head) };
            let msg_len = message.len();
            let severity = (msg_len >= 8)
                .then(|| Header::read_from_bytes(&message[0..8]).unwrap().severity().into());
            let container = self.containers.get_mut(container_id).unwrap();
            container.msg_ids.end += 1;
            if let Some(severity) = severity {
                container.stats.ingest_message(msg_len, severity);
            }
            self.last_scanned += ring_buffer_record_len(msg_len) as u64;
            self.last_scanned_message_id += 1;
            self.wake = true;
        }
        if self.wake {
            let boot_time = zx::BootInstant::get().into_nanos();
            let count = self.last_scanned_message_id - self.tail_message_id;
            self.buffer.saturation_curve.record(boot_time, count);
        }
    }

    /// Ensures the buffer keeps the required amount of space.
    fn check_space(&mut self, head: u64) {
        self.update_message_ids(head);
        let capacity = self.ring_buffer.capacity();
        let mut space = capacity
            .checked_sub((head - self.tail) as usize)
            .unwrap_or_else(|| panic!("bad range: {:?}", self.tail..head));
        let required_space = capacity * SPACE_THRESHOLD_NUMERATOR / SPACE_THRESHOLD_DENOMINATOR;
        while space < required_space {
            let Some(amount) = self.pop(head) else { break };
            space += amount;
        }
    }

    /// Returns a cursor.
    fn cursor(&mut self, mode: StreamMode, selectors: Vec<ComponentSelector>) -> FilterCursor {
        // NOTE: It is not safe to use on_inactive in this function because this function can be
        // called whilst locks are held which are the same locks that the on_inactive notification
        // uses.
        let (index, message_id) = match mode {
            StreamMode::Snapshot => (self.tail, self.tail_message_id),
            StreamMode::Subscribe => {
                let head = self.ring_buffer.head();
                self.update_message_ids(head);
                (self.last_scanned, self.last_scanned_message_id)
            }
            StreamMode::SnapshotThenSubscribe => (self.tail, self.tail_message_id),
        };
        FilterCursor::new(
            Arc::clone(self.buffer),
            index,
            message_id,
            matches!(mode, StreamMode::Snapshot),
            selectors,
        )
    }
}

#[derive(Default)]
struct Containers {
    slab: Slab<ContainerInfo>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ContainerId(u32);

impl Containers {
    #[cfg(test)]
    fn len(&self) -> usize {
        self.slab.len()
    }

    fn get(&self, id: ContainerId) -> Option<&ContainerInfo> {
        self.slab.get(id.0)
    }

    fn get_mut(&mut self, id: ContainerId) -> Option<&mut ContainerInfo> {
        self.slab.get_mut(id.0)
    }

    fn new_container(
        &mut self,
        buffer: &RingBuffer,
        identity: Arc<ComponentIdentity>,
        stats: Arc<LogStreamStats>,
    ) -> ContainerId {
        ContainerId(self.slab.insert(|id| {
            let (iob, _) = buffer.new_iob_writer(id as u64).unwrap();
            ContainerInfo::new(identity, stats, iob)
        }))
    }

    fn free(&mut self, id: ContainerId) {
        self.slab.free(id.0);
    }
}

#[derive(Derivative)]
#[derivative(Debug)]
struct ContainerInfo {
    // The identity of the container.
    identity: Arc<ComponentIdentity>,

    // The first and last message IDs stored in the shared buffer. The last message ID can be out of
    // date if there are concurrent writers.
    msg_ids: Range<u64>,

    // Whether the container is terminated.
    terminated: bool,

    // Inspect instrumentation.
    #[derivative(Debug = "ignore")]
    stats: Arc<LogStreamStats>,

    // The first socket ID for this container.
    first_socket_id: SocketId,

    // The IOBuffer used for writing.
    iob: zx::Iob,

    // The number of client IOBuffers.
    iob_count: usize,

    // The number of messages dropped when forwarding from a socket to an IOBuffer.
    dropped_count: u64,
}

impl ContainerInfo {
    fn new(identity: Arc<ComponentIdentity>, stats: Arc<LogStreamStats>, iob: zx::Iob) -> Self {
        Self {
            identity,
            msg_ids: 0..0,
            terminated: false,
            stats,
            first_socket_id: SocketId::NULL,
            iob,
            iob_count: 0,
            dropped_count: 0,
        }
    }

    fn should_free(&self) -> bool {
        self.terminated && !self.is_active()
    }

    // Returns true if the container is considered active which is the case if it has sockets, io
    // buffers, or buffered messages.
    //
    // NOTE: Whenever a socket or iob is closed, `update_message_ids` must called to ensure
    // `msg_ids.end` is correctly set.
    fn is_active(&self) -> bool {
        self.first_socket_id != SocketId::NULL
            || self.iob_count > 0
            || self.msg_ids.end != self.msg_ids.start
            || ARCHIVIST_MONIKER.get().is_some_and(|m| *self.identity == *m)
    }

    // # Panics
    //
    // This will panic if the socket isn't found.
    fn remove_socket(&mut self, socket_id: SocketId, sockets: &mut Slab<Socket>) {
        let Socket { prev, next, .. } = *sockets.get(socket_id.0).unwrap();
        if prev == SocketId::NULL {
            self.first_socket_id = next;
        } else {
            sockets.get_mut(prev.0).unwrap().next = next;
        }
        if next != SocketId::NULL {
            sockets
                .get_mut(next.0)
                .unwrap_or_else(|| panic!("next {next:?} has been freed!"))
                .prev = prev;
        }
        sockets.free(socket_id.0);
        self.stats.close_socket();
        debug!(identity:% = self.identity; "Socket closed.");
    }
}

pub struct ContainerBuffer {
    shared_buffer: Arc<SharedBuffer>,
    container_id: ContainerId,
}

impl ContainerBuffer {
    /// Returns the tag used by IOBuffers used for this component.
    pub fn iob_tag(&self) -> u64 {
        self.container_id.0 as u64
    }

    /// Ingests a new message.
    ///
    /// If the message is invalid, it is dropped.
    pub fn push_back(&self, msg: &[u8]) {
        self.shared_buffer.inner.lock().ingest(msg, self.container_id);
    }

    /// Returns the identity of the container if it still exists.
    pub fn identity(&self) -> Option<Arc<ComponentIdentity>> {
        self.shared_buffer
            .inner
            .lock()
            .containers
            .get(self.container_id)
            .map(|c| Arc::clone(&c.identity))
    }

    /// Returns an IOBuffer for the container.
    pub fn iob(&self) -> zx::Iob {
        let mut inner = self.shared_buffer.inner.lock();

        inner.containers.get_mut(self.container_id).unwrap().iob_count += 1;

        let (ep0, ep1) = inner.ring_buffer.new_iob_writer(self.container_id.0 as u64).unwrap();

        inner.iob_peers.insert(|idx| {
            ep1.wait_async(
                &self.shared_buffer.port,
                idx as u64 | IOB_PEER_CLOSED_KEY_BASE,
                zx::Signals::IOB_PEER_CLOSED,
                zx::WaitAsyncOpts::empty(),
            )
            .unwrap();

            (self.container_id, ep1)
        });

        ep0
    }

    /// Marks the buffer as terminated which will force all cursors to end and close all sockets.
    /// The component's data will remain in the buffer until the messages are rolled out. This will
    /// *not* drain sockets or close IOBuffers.
    pub fn terminate(&self) {
        let mut inner = InnerGuard::new(&self.shared_buffer);

        // See the comment on `is_active()` for why this is required.
        inner.update_message_ids(inner.ring_buffer.head());

        if let Some(container) = inner.containers.get_mut(self.container_id) {
            container.terminated = true;
            if container.first_socket_id != SocketId::NULL {
                let mut sockets = self.shared_buffer.sockets.lock();
                loop {
                    container.remove_socket(container.first_socket_id, &mut sockets);
                    if container.first_socket_id == SocketId::NULL {
                        break;
                    }
                }
            }
            if container.should_free() {
                inner.containers.free(self.container_id);
            }
            inner.wake = true;
        }
    }

    /// Returns true if the container has messages, sockets or IOBuffers.
    pub fn is_active(&self) -> bool {
        self.shared_buffer
            .inner
            .lock()
            .containers
            .get(self.container_id)
            .is_some_and(|c| c.is_active())
    }

    /// Adds a socket for this container.
    pub fn add_socket(&self, socket: zx::Socket) {
        let mut inner = self.shared_buffer.inner.lock();
        let Some(container) = inner.containers.get_mut(self.container_id) else { return };
        container.stats.open_socket();
        let next = container.first_socket_id;
        let mut sockets = self.shared_buffer.sockets.lock();
        let socket_id = SocketId(sockets.insert(|socket_id| {
            socket
                .wait_async(
                    &self.shared_buffer.port,
                    socket_id as u64,
                    zx::Signals::OBJECT_READABLE | zx::Signals::OBJECT_PEER_CLOSED,
                    zx::WaitAsyncOpts::empty(),
                )
                .unwrap();
            Socket { socket, container_id: self.container_id, prev: SocketId::NULL, next }
        }));
        if next != SocketId::NULL {
            sockets.get_mut(next.0).unwrap().prev = socket_id;
        }
        container.first_socket_id = socket_id;
    }
}

impl Drop for ContainerBuffer {
    fn drop(&mut self) {
        self.terminate();
    }
}

/// Implements a simple Slab allocator.
struct Slab<T> {
    slab: Vec<Slot<T>>,
    free_index: usize,
}

impl<T> Default for Slab<T> {
    fn default() -> Self {
        Self { slab: Vec::new(), free_index: usize::MAX }
    }
}

enum Slot<T> {
    Used(T),
    Free(usize),
}

impl<T> Slab<T> {
    /// Returns the number of used entries. This is not performant.
    #[cfg(test)]
    fn len(&self) -> usize {
        self.slab.iter().filter(|c| matches!(c, Slot::Used(_))).count()
    }

    fn free(&mut self, index: u32) -> T {
        let index = index as usize;
        let value = match std::mem::replace(&mut self.slab[index], Slot::Free(self.free_index)) {
            Slot::Free(_) => panic!("Slot already free"),
            Slot::Used(value) => value,
        };
        self.free_index = index;
        value
    }

    fn get(&self, id: u32) -> Option<&T> {
        self.slab.get(id as usize).and_then(|s| match s {
            Slot::Used(s) => Some(s),
            _ => None,
        })
    }

    fn get_mut(&mut self, id: u32) -> Option<&mut T> {
        self.slab.get_mut(id as usize).and_then(|s| match s {
            Slot::Used(s) => Some(s),
            _ => None,
        })
    }

    fn insert(&mut self, value: impl FnOnce(u32) -> T) -> u32 {
        let free_index = self.free_index;
        if free_index != usize::MAX {
            self.free_index = match std::mem::replace(
                &mut self.slab[free_index],
                Slot::Used(value(free_index as u32)),
            ) {
                Slot::Free(next) => next,
                _ => unreachable!(),
            };
            free_index as u32
        } else {
            // This is < rather than <= because we reserve 0xffff_ffff to be used as a NULL value
            // (see SocketId::NULL below).
            assert!(self.slab.len() < u32::MAX as usize);
            self.slab.push(Slot::Used(value(self.slab.len() as u32)));
            (self.slab.len() - 1) as u32
        }
    }

    /// Returns the next used slot starting from `id` (inclusive).
    fn next_used(&self, id: u32) -> Option<u32> {
        let mut id = id as usize;
        while id < self.slab.len() {
            if matches!(self.slab[id], Slot::Used(_)) {
                return Some(id as u32);
            }
            id += 1;
        }
        None
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SocketId(u32);

impl SocketId {
    const NULL: Self = SocketId(0xffff_ffff);
}

struct Socket {
    socket: zx::Socket,
    container_id: ContainerId,
    // Sockets are stored as a linked list for each container.
    prev: SocketId,
    next: SocketId,
}

#[cfg(test)]
mod tests {
    use super::{SharedBuffer, SharedBufferOptions, Slab, create_ring_buffer};
    use crate::identity::ComponentIdentity;
    use crate::logs::stats::LogStreamStats;
    use crate::logs::testing::make_message;
    use assert_matches::assert_matches;
    use diagnostics_assertions::{AnyProperty, assert_data_tree};
    use fidl_fuchsia_diagnostics::StreamMode;
    use fuchsia_async as fasync;
    use fuchsia_async::TimeoutExt;
    use fuchsia_inspect::Inspector;
    use futures::FutureExt;
    use futures::channel::mpsc;
    use futures::future::OptionFuture;
    use futures::stream::{FuturesUnordered, StreamExt as _};
    use moniker::ExtendedMoniker;
    use ring_buffer::MAX_MESSAGE_SIZE;
    use selectors::SelectorExt;
    use std::future::poll_fn;
    use std::iter::repeat_with;
    use std::pin::pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::task::Poll;
    use std::time::Duration;
    use zerocopy::FromBytes;

    fn test_stats() -> Arc<LogStreamStats> {
        Arc::new(LogStreamStats::new(
            &fuchsia_inspect::Node::default(),
            &ComponentIdentity::unknown(),
        ))
    }

    fn container_cursor(
        buffer: &Arc<SharedBuffer>,
        container: &super::ContainerBuffer,
        mode: StreamMode,
    ) -> Option<impl futures::Stream<Item = Box<[u8]>>> {
        let selectors = vec![container.identity()?.moniker.clone().into_component_selector()];
        let mut cursor = Box::pin(buffer.cursor(mode, selectors));
        Some(futures::stream::poll_fn(move |cx| {
            cursor.as_mut().poll_next(cx).map(|m| m.map(|m| m.parse().unwrap().2.into()))
        }))
    }

    async fn yield_to_executor() {
        let mut first_time = true;
        poll_fn(|cx| {
            if first_time {
                cx.waker().wake_by_ref();
                first_time = false;
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
    }

    #[fuchsia::test]
    async fn push_one_message() {
        let buffer = SharedBuffer::new(
            create_ring_buffer(MAX_MESSAGE_SIZE),
            Box::new(|_| {}),
            Default::default(),
            &fuchsia_inspect::Node::default(),
        );
        let container_buffer =
            buffer.new_container_buffer(Arc::new(vec!["a"].into()), test_stats());
        let msg = make_message("a", None, zx::BootInstant::from_nanos(1));
        container_buffer.push_back(msg.bytes());

        // Make sure the cursor can find it.
        let cursor = container_cursor(&buffer, &container_buffer, StreamMode::Snapshot).unwrap();
        assert_eq!(cursor.map(|item| assert_eq!(&*item, msg.bytes())).count().await, 1);
    }

    #[fuchsia::test]
    async fn message_too_short() {
        let buffer = SharedBuffer::new(
            create_ring_buffer(MAX_MESSAGE_SIZE),
            Box::new(|_| {}),
            Default::default(),
            &fuchsia_inspect::Node::default(),
        );

        let container_buffer =
            buffer.new_container_buffer(Arc::new(vec!["a"].into()), test_stats());
        container_buffer.push_back(&[0]);

        assert_eq!(
            container_cursor(&buffer, &container_buffer, StreamMode::Snapshot)
                .unwrap()
                .count()
                .await,
            0
        );
    }

    #[fuchsia::test]
    async fn bad_type() {
        let buffer = SharedBuffer::new(
            create_ring_buffer(MAX_MESSAGE_SIZE),
            Box::new(|_| {}),
            Default::default(),
            &fuchsia_inspect::Node::default(),
        );
        let container_buffer =
            buffer.new_container_buffer(Arc::new(vec!["a"].into()), test_stats());
        container_buffer.push_back(&[0x77; 16]);

        assert_eq!(
            container_cursor(&buffer, &container_buffer, StreamMode::Snapshot)
                .unwrap()
                .count()
                .await,
            0
        );
    }

    #[fuchsia::test]
    async fn message_truncated() {
        let buffer = SharedBuffer::new(
            create_ring_buffer(MAX_MESSAGE_SIZE),
            Box::new(|_| {}),
            Default::default(),
            &fuchsia_inspect::Node::default(),
        );
        let container_buffer =
            buffer.new_container_buffer(Arc::new(vec!["a"].into()), test_stats());
        let msg = make_message("a", None, zx::BootInstant::from_nanos(1));
        container_buffer.push_back(&msg.bytes()[..msg.bytes().len() - 1]);

        assert_eq!(
            container_cursor(&buffer, &container_buffer, StreamMode::Snapshot)
                .unwrap()
                .count()
                .await,
            0
        );
    }

    #[fuchsia::test]
    async fn buffer_wrapping() {
        let buffer = SharedBuffer::new(
            create_ring_buffer(MAX_MESSAGE_SIZE),
            Box::new(|_| {}),
            SharedBufferOptions { sleep_time: Duration::ZERO, ..Default::default() },
            &fuchsia_inspect::Node::default(),
        );
        let container_buffer =
            buffer.new_container_buffer(Arc::new(vec!["a"].into()), test_stats());

        // Keep writing messages until we wrap.
        let mut i = 0;
        loop {
            let msg = make_message(&format!("{i}"), None, zx::BootInstant::from_nanos(i));
            container_buffer.push_back(msg.bytes());
            i += 1;

            // Yield to the executor to allow messages to be rolled out.
            yield_to_executor().await;

            let inner = buffer.inner.lock();
            if inner.ring_buffer.head() > inner.ring_buffer.capacity() as u64 {
                break;
            }
        }

        // Read back all the messages.
        let mut cursor =
            pin!(container_cursor(&buffer, &container_buffer, StreamMode::Snapshot).unwrap());

        let mut j;
        let item = cursor.next().await;
        assert_matches!(
            item,
            Some(item) => {
                j = i64::read_from_bytes(&item[8..16]).unwrap();
                let msg = make_message(&format!("{j}"),
                                       None,
                                       zx::BootInstant::from_nanos(j));
                assert_eq!(&*item, msg.bytes());
            }
        );

        j += 1;
        while j != i {
            assert_matches!(
                cursor.next().await,
                Some(item) => {
                    assert_eq!(&*item, make_message(&format!("{j}"),
                                                         None,
                                                         zx::BootInstant::from_nanos(j)).bytes());
                }
            );
            j += 1;
        }

        assert!(cursor.next().await.is_none());
    }

    #[fuchsia::test]
    async fn on_inactive() {
        let identity = Arc::new(vec!["a"].into());
        let on_inactive = Arc::new(AtomicU64::new(0));
        let buffer = {
            let on_inactive = Arc::clone(&on_inactive);
            let identity = Arc::clone(&identity);
            Arc::new(SharedBuffer::new(
                create_ring_buffer(MAX_MESSAGE_SIZE),
                Box::new(move |i| {
                    assert_eq!(i, identity);
                    on_inactive.fetch_add(1, Ordering::Relaxed);
                }),
                SharedBufferOptions { sleep_time: Duration::ZERO, ..Default::default() },
                &fuchsia_inspect::Node::default(),
            ))
        };
        let container_a = buffer.new_container_buffer(identity, test_stats());
        let container_b = buffer.new_container_buffer(Arc::new(vec!["b"].into()), test_stats());

        let msg = make_message("a", None, zx::BootInstant::from_nanos(1));
        container_a.push_back(msg.bytes());

        // Repeatedly write messages to b until a is rolled out.
        while container_cursor(&buffer, &container_a, StreamMode::Snapshot).unwrap().count().await
            == 1
        {
            container_b.push_back(msg.bytes());

            // Yield to the executor to allow messages to be rolled out.
            yield_to_executor().await;
        }

        assert_eq!(on_inactive.load(Ordering::Relaxed), 1);
    }

    #[fuchsia::test]
    async fn terminate_drops_container() {
        // Silence a clippy warning; SharedBuffer needs an executor.
        async {}.await;

        let buffer = SharedBuffer::new(
            create_ring_buffer(MAX_MESSAGE_SIZE),
            Box::new(|_| {}),
            SharedBufferOptions { sleep_time: Duration::ZERO, ..Default::default() },
            &fuchsia_inspect::Node::default(),
        );

        // terminate when buffer has no logs.
        let container_a = buffer.new_container_buffer(Arc::new(vec!["a"].into()), test_stats());
        assert_eq!(buffer.container_count(), 1);
        container_a.terminate();

        assert_eq!(buffer.container_count(), 0);

        // terminate when buffer has logs.
        let container_a = buffer.new_container_buffer(Arc::new(vec!["a"].into()), test_stats());
        let msg = make_message("a", None, zx::BootInstant::from_nanos(1));
        container_a.push_back(msg.bytes());
        assert_eq!(buffer.container_count(), 1);
        container_a.terminate();

        // The container should still be there because it has logs.
        assert_eq!(buffer.container_count(), 1);

        // Roll out the logs.
        let container_b = buffer.new_container_buffer(Arc::new(vec!["b"].into()), test_stats());
        assert_eq!(buffer.container_count(), 2);

        // Repeatedly write messages to b until a's message is dropped and then the container will
        // be dropped.
        while buffer.container_count() != 1 {
            container_b.push_back(msg.bytes());

            // Yield to the executor to allow messages to be rolled out.
            yield_to_executor().await;
        }

        assert!(container_cursor(&buffer, &container_a, StreamMode::Subscribe).is_none());
    }

    #[fuchsia::test]
    async fn cursor_subscribe() {
        for mode in [StreamMode::Subscribe, StreamMode::SnapshotThenSubscribe] {
            let buffer = SharedBuffer::new(
                create_ring_buffer(MAX_MESSAGE_SIZE),
                Box::new(|_| {}),
                Default::default(),
                &fuchsia_inspect::Node::default(),
            );
            let container =
                Arc::new(buffer.new_container_buffer(Arc::new(vec!["a"].into()), test_stats()));
            let msg = make_message("a", None, zx::BootInstant::from_nanos(1));
            container.push_back(msg.bytes());

            let (sender, mut receiver) = mpsc::unbounded();

            // Run the cursor in a separate task so that we can test it gets woken correctly.
            {
                let container = Arc::clone(&container);
                fasync::Task::spawn(async move {
                    let mut cursor =
                        pin!(container_cursor(&container.shared_buffer, &container, mode).unwrap());
                    while let Some(item) = cursor.next().await {
                        sender.unbounded_send(item).unwrap();
                    }
                })
                .detach();
            }

            // The existing message should only be returned with SnapshotThenSubscribe
            if mode == StreamMode::SnapshotThenSubscribe {
                assert_matches!(
                    receiver.next().await,
                    Some(item) if item.as_ref() == msg.bytes()
                );
            }

            // No message should arrive. We can only use a timeout here.
            assert!(
                OptionFuture::from(Some(receiver.next()))
                    .on_timeout(Duration::from_millis(500), || None)
                    .await
                    .is_none()
            );

            container.push_back(msg.bytes());

            // The message should arrive now.
            assert_matches!(
                receiver.next().await,
                Some(item) if item.as_ref() == msg.bytes()
            );
        }
    }

    #[fuchsia::test]
    async fn drained_post_termination_cursors() {
        let buffer = SharedBuffer::new(
            create_ring_buffer(MAX_MESSAGE_SIZE),
            Box::new(|_| {}),
            Default::default(),
            &fuchsia_inspect::Node::default(),
        );
        let container =
            Arc::new(buffer.new_container_buffer(Arc::new(vec!["a"].into()), test_stats()));
        let msg = make_message("a", None, zx::BootInstant::from_nanos(1));

        let mut cursor_a =
            pin!(container_cursor(&buffer, &container, StreamMode::Subscribe).unwrap());
        let mut cursor_b =
            pin!(container_cursor(&buffer, &container, StreamMode::SnapshotThenSubscribe).unwrap());

        container.push_back(msg.bytes());
        container.push_back(msg.bytes());
        container.push_back(msg.bytes());
        container.push_back(msg.bytes());
        container.push_back(msg.bytes());

        let mut cursor_c =
            pin!(container_cursor(&buffer, &container, StreamMode::Snapshot).unwrap());
        assert!(cursor_a.next().await.is_some());
        assert!(cursor_b.next().await.is_some());
        assert!(cursor_c.next().await.is_some());

        drop(buffer.terminate());

        // All cursors should return the 4 remaining messages.
        assert_eq!(cursor_a.count().await, 4);
        assert_eq!(cursor_b.count().await, 4);
        assert_eq!(cursor_c.count().await, 4);
    }

    #[fuchsia::test]
    async fn empty_post_termination_cursors() {
        let buffer = SharedBuffer::new(
            create_ring_buffer(MAX_MESSAGE_SIZE),
            Box::new(|_| {}),
            Default::default(),
            &fuchsia_inspect::Node::default(),
        );
        let container =
            Arc::new(buffer.new_container_buffer(Arc::new(vec!["a"].into()), test_stats()));

        let cursor_a = container_cursor(&buffer, &container, StreamMode::Subscribe).unwrap();
        let cursor_b =
            container_cursor(&buffer, &container, StreamMode::SnapshotThenSubscribe).unwrap();
        let cursor_c = container_cursor(&buffer, &container, StreamMode::Snapshot).unwrap();

        drop(buffer.terminate());

        assert_eq!(cursor_a.count().await, 0);
        assert_eq!(cursor_b.count().await, 0);
        assert_eq!(cursor_c.count().await, 0);
    }

    #[fuchsia::test]
    async fn recycled_container_slot() {
        let buffer = Arc::new(SharedBuffer::new(
            create_ring_buffer(MAX_MESSAGE_SIZE),
            Box::new(|_| {}),
            SharedBufferOptions { sleep_time: Duration::ZERO, ..Default::default() },
            &fuchsia_inspect::Node::default(),
        ));
        let container_a =
            Arc::new(buffer.new_container_buffer(Arc::new(vec!["a"].into()), test_stats()));
        let msg = make_message("a", None, zx::BootInstant::from_nanos(1));
        container_a.push_back(msg.bytes());

        let mut cursor = pin!(
            container_cursor(&buffer, &container_a, StreamMode::SnapshotThenSubscribe).unwrap()
        );
        assert_matches!(cursor.next().await, Some(_));

        // Roll out all the messages.
        let container_b =
            Arc::new(buffer.new_container_buffer(Arc::new(vec!["b"].into()), test_stats()));
        while container_cursor(&buffer, &container_a, StreamMode::Snapshot).unwrap().count().await
            > 0
        {
            container_b.push_back(msg.bytes());

            // Yield to the executor to allow messages to be rolled out.
            yield_to_executor().await;
        }

        container_a.terminate();

        // This should create a new container that uses a new slot and shouldn't interfere with
        // container_a.
        let container_c =
            Arc::new(buffer.new_container_buffer(Arc::new(vec!["c"].into()), test_stats()));
        container_c.push_back(msg.bytes());
        container_c.push_back(msg.bytes());
    }

    #[fuchsia::test]
    async fn socket_increments_logstats() {
        let inspector = Inspector::default();
        let identity =
            Arc::new(ComponentIdentity::new(ExtendedMoniker::parse_str("./test").unwrap(), ""));
        let stats = Arc::new(LogStreamStats::new(inspector.root(), &identity));
        let buffer = Arc::new(SharedBuffer::new(
            create_ring_buffer(65536),
            Box::new(|_| {}),
            Default::default(),
            &fuchsia_inspect::Node::default(),
        ));
        let container_a = Arc::new(buffer.new_container_buffer(Arc::new(vec!["a"].into()), stats));
        let msg = make_message("a", None, zx::BootInstant::from_nanos(1));

        let (local, remote) = zx::Socket::create_datagram();
        container_a.add_socket(remote);

        let cursor_a = container_cursor(&buffer, &container_a, StreamMode::Subscribe).unwrap();

        // Use FuturesUnordered so that we can make sure that the cursor is woken when a message is
        // received (FuturesUnordered uses separate wakers for all the futures it manages).
        let mut futures = FuturesUnordered::new();
        futures.push(async move {
            let mut cursor_a = pin!(cursor_a);
            cursor_a.next().await
        });
        let mut next = futures.next();
        assert!(futures::poll!(&mut next).is_pending());

        local.write(msg.bytes()).unwrap();

        let cursor_b = pin!(container_cursor(&buffer, &container_a, StreamMode::Snapshot).unwrap());

        assert_eq!(cursor_b.map(|item| assert_eq!(&*item, msg.bytes())).count().await, 1);

        // If cursor_a wasn't woken, this will hang.
        next.await;
        // Validate logstats (must happen after the socket was handled)
        assert_data_tree!(
            inspector,
            root: contains {
                test: {
                    url: "",
                    last_timestamp: AnyProperty,
                    sockets_closed: 0u64,
                    sockets_opened: 1u64,
                    invalid: {
                        number: 0u64,
                        bytes: 0u64,
                    },
                    total: {
                        number: 1u64,
                        bytes: 88u64,
                    },
                    rolled_out: {
                        number: 0u64,
                        bytes: 0u64,
                    },
                    trace: {
                        number: 0u64,
                        bytes: 0u64,
                    },
                    debug: {
                        number: 1u64,
                        bytes: 88u64,
                    },
                    info: {
                        number: 0u64,
                        bytes: 0u64,
                    },
                    warn: {
                        number: 0u64,
                        bytes: 0u64,
                    },
                    error: {
                        number: 0u64,
                        bytes: 0u64,
                    },
                    fatal: {
                        number: 0u64,
                        bytes: 0u64,
                    },
                }
            }
        );
    }

    #[fuchsia::test]
    async fn socket() {
        let buffer = Arc::new(SharedBuffer::new(
            create_ring_buffer(MAX_MESSAGE_SIZE),
            Box::new(|_| {}),
            Default::default(),
            &fuchsia_inspect::Node::default(),
        ));
        let container_a =
            Arc::new(buffer.new_container_buffer(Arc::new(vec!["a"].into()), test_stats()));
        let msg = make_message("a", None, zx::BootInstant::from_nanos(1));

        let (local, remote) = zx::Socket::create_datagram();
        container_a.add_socket(remote);

        let cursor_a = container_cursor(&buffer, &container_a, StreamMode::Subscribe).unwrap();

        // Use FuturesUnordered so that we can make sure that the cursor is woken when a message is
        // received (FuturesUnordered uses separate wakers for all the futures it manages).
        let mut futures = FuturesUnordered::new();
        futures.push(async move {
            let mut cursor_a = pin!(cursor_a);
            cursor_a.next().await
        });
        let mut next = futures.next();
        assert!(futures::poll!(&mut next).is_pending());

        local.write(msg.bytes()).unwrap();

        let cursor_b = pin!(container_cursor(&buffer, &container_a, StreamMode::Snapshot).unwrap());

        assert_eq!(cursor_b.map(|item| assert_eq!(&*item, msg.bytes())).count().await, 1);

        // If cursor_a wasn't woken, this will hang.
        next.await;
    }

    #[fuchsia::test]
    async fn socket_on_inactive() {
        let on_inactive = Arc::new(AtomicU64::new(0));
        let a_identity = Arc::new(vec!["a"].into());
        let buffer = Arc::new(SharedBuffer::new(
            create_ring_buffer(MAX_MESSAGE_SIZE),
            {
                let on_inactive = Arc::clone(&on_inactive);
                let a_identity = Arc::clone(&a_identity);
                Box::new(move |id| {
                    assert_eq!(id, a_identity);
                    on_inactive.fetch_add(1, Ordering::Relaxed);
                })
            },
            SharedBufferOptions { sleep_time: Duration::ZERO, ..Default::default() },
            &fuchsia_inspect::Node::default(),
        ));
        let container_a = Arc::new(buffer.new_container_buffer(a_identity, test_stats()));
        let msg = make_message("a", None, zx::BootInstant::from_nanos(1));

        let (local, remote) = zx::Socket::create_datagram();
        container_a.add_socket(remote);

        local.write(msg.bytes()).unwrap();

        let cursor = pin!(container_cursor(&buffer, &container_a, StreamMode::Snapshot).unwrap());

        assert_eq!(cursor.map(|item| assert_eq!(&*item, msg.bytes())).count().await, 1);

        // Now roll out a's messages.
        let container_b = buffer.new_container_buffer(Arc::new(vec!["b"].into()), test_stats());
        while container_cursor(&buffer, &container_a, StreamMode::Snapshot).unwrap().count().await
            == 1
        {
            container_b.push_back(msg.bytes());

            // Yield to the executor to allow messages to be rolled out.
            yield_to_executor().await;
        }

        assert_eq!(on_inactive.load(Ordering::Relaxed), 0);

        // Close the socket.
        std::mem::drop(local);

        // We don't know when the socket thread will run so we have to loop.
        while on_inactive.load(Ordering::Relaxed) != 1 {
            fasync::Timer::new(Duration::from_millis(50)).await;
        }
    }

    #[fuchsia::test]
    async fn flush() {
        let a_identity = Arc::new(vec!["a"].into());
        let buffer = Arc::new(SharedBuffer::new(
            create_ring_buffer(1024 * 1024),
            Box::new(|_| {}),
            Default::default(),
            &fuchsia_inspect::Node::default(),
        ));
        let container_a = Arc::new(buffer.new_container_buffer(a_identity, test_stats()));
        let msg = make_message("a", None, zx::BootInstant::from_nanos(1));

        let (local, remote) = zx::Socket::create_datagram();
        container_a.add_socket(remote);

        let cursor = pin!(container_cursor(&buffer, &container_a, StreamMode::Subscribe).unwrap());

        const COUNT: usize = 1000;
        for _ in 0..COUNT {
            local.write(msg.bytes()).unwrap();
        }

        // Race two flush futures.
        let mut flush_futures = FuturesUnordered::from_iter([buffer.flush(), buffer.flush()]);
        flush_futures.next().await;

        let messages: Option<Vec<_>> = cursor.take(COUNT).collect().now_or_never();
        assert!(messages.is_some());

        // Make sure the other one finishes too.
        flush_futures.next().await;

        // Make sure we can still terminate the buffer.
        buffer.terminate().await;
    }

    #[fuchsia::test]
    fn test_slab_next_used() {
        let mut slab = Slab::default();
        let mut ids: Vec<_> = repeat_with(|| slab.insert(|_| ())).take(4).collect();
        ids.sort();
        slab.free(ids[0]);
        slab.free(ids[2]);
        assert_eq!(slab.next_used(0), Some(ids[1]));
        assert_eq!(slab.next_used(ids[1]), Some(ids[1]));
        assert_eq!(slab.next_used(ids[1] + 1), Some(ids[3]));
        assert_eq!(slab.next_used(ids[3] + 1), None);
    }
}
