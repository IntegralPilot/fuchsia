// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::{ContainerId, Inner, InnerGuard, SharedBuffer};
use crate::identity::ComponentIdentity;
use diagnostics_data::{LogError, LogsData};
use diagnostics_log_encoding::parse::ParseError;
use diagnostics_log_encoding::{Header, TRACING_FORMAT_LOG_RECORD_TYPE};
use diagnostics_message::error::MessageError;
use fidl_fuchsia_diagnostics::ComponentSelector;
use fuchsia_async::condition::{ConditionGuard, WakerEntry};
use futures::Stream;
use futures::stream::FusedStream;
use pin_project::pin_project;
use ring_buffer::ring_buffer_record_len;
use selectors::matches_selectors;
use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use zerocopy::FromBytes;

/// FilterCursor is a cursor that returns all logs optionally filtered by component selectors.
#[pin_project]
pub struct FilterCursor {
    buffer: Arc<SharedBuffer>,
    index: u64,
    message_id: u64,
    end: Option<u64>,
    selectors: Vec<ComponentSelector>,
    messages: BinaryHeap<MessageRef>,
    flush_sockets_for_snapshot: bool,
    #[pin]
    waker_entry: WakerEntry<Inner>,
}

impl FilterCursor {
    pub fn new(
        buffer: Arc<SharedBuffer>,
        index: u64,
        message_id: u64,
        snapshot: bool,
        selectors: Vec<ComponentSelector>,
    ) -> Self {
        let waker_entry = buffer.inner.waker_entry();
        Self {
            buffer,
            index,
            message_id,
            end: None,
            selectors,
            messages: BinaryHeap::new(),
            waker_entry,
            flush_sockets_for_snapshot: snapshot,
        }
    }

    /// Polls for the next message.  We can't use the Stream trait because this is a lending
    /// iterator; it returns a reference to a message (and will hold a lock).  Depending on the use,
    /// this can be more efficient than using a Stream which will involve copying the message.
    pub fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Message<'_>>> {
        let this = self.project();

        if *this.flush_sockets_for_snapshot {
            let mut inner = InnerGuard::new(this.buffer);
            *this.end = Some(this.buffer.flush_sockets(&mut inner));
            *this.flush_sockets_for_snapshot = false;
        }

        let mut inner = this.buffer.inner.lock();

        // Comparing monikers can be expensive, so memoize the results.
        struct ContainerIds {
            ids: [ContainerId; 8],
            result: u8,
            pos: usize,
        }

        impl ContainerIds {
            /// Memoizes the result of `predicate` for a given `id`.
            fn memoize(&mut self, id: ContainerId, predicate: impl FnOnce() -> bool) -> bool {
                if let Some(pos) = self.ids.iter().position(|i| i == &id) {
                    self.result & (1 << pos) != 0
                } else {
                    let result = predicate();
                    self.ids[self.pos] = id;
                    if result {
                        self.result |= 1 << self.pos;
                    } else {
                        self.result &= !(1 << self.pos);
                    }
                    self.pos = (self.pos + 1) % self.ids.len();
                    result
                }
            }
        }

        let mut container_ids = ContainerIds { ids: [ContainerId(0xffff); 8], result: 0, pos: 0 };

        // NOTE: If messages are dropped the dropped count won't account for filtering: it will
        // include messages that have been dropped that don't match the filter.  Fixing this is
        // difficult and not worth the effort.  Dropped messages should be rare and the common case
        // is that there is no filtering.
        let mut dropped = 0;
        if *this.index < inner.tail {
            *this.index = inner.tail;
            dropped += inner.tail_message_id - *this.message_id;
            *this.message_id = inner.tail_message_id;
        }
        while *this.index < inner.last_scanned && this.end.is_none_or(|e| *this.index < e) {
            // SAFETY: We've checked index >= inner.tail, so the range must be valid.  `msg`
            // will remain valid whilst we're holding the lock.
            let (component, msg, timestamp) =
                unsafe { inner.parse_message(*this.index..inner.last_scanned) };

            if let Some(timestamp) = timestamp
                && let Some(container) = inner.containers.get(component)
                && (this.selectors.is_empty()
                    || container_ids.memoize(component, || {
                        matches_selectors(&container.identity.moniker, this.selectors)
                    }))
            {
                this.messages.push(MessageRef { index: *this.index, timestamp });
            }

            *this.index += ring_buffer_record_len(msg.len()) as u64;
            *this.message_id += 1;
        }
        let message = loop {
            let Some(message) = this.messages.pop() else { break None };
            if message.index < inner.tail {
                // See note above regarding filtering.
                dropped += 1;
            } else {
                break Some(message);
            }
        };

        let live_messages = (inner.last_scanned_message_id - inner.tail_message_id) as usize;
        const HEAP_PRUNE_THRESHOLD: usize = 256;

        // Drain all stale entries if the heap has grown too large.
        // Timestamps are attacker-controlled (see parse_message), so a hostile writer
        // can order the heap such that stale entries never surface and the heap grows
        // without bound. This keeps the heap bounded by the live ring-buffer window.
        if this.messages.len() > live_messages + HEAP_PRUNE_THRESHOLD {
            let tail = inner.tail;
            this.messages.retain(|m| m.index >= tail);
        }

        if let Some(message) = message {
            return Poll::Ready(Some(Message { inner, message, dropped }));
        }

        if this.end.is_some_and(|e| *this.index >= e) || inner.terminated {
            *this.index = u64::MAX;
            Poll::Ready(None)
        } else {
            inner.add_waker(this.waker_entry, cx.waker().clone());
            Poll::Pending
        }
    }

    fn is_terminated(&self) -> bool {
        self.index == u64::MAX
    }
}

#[derive(Eq)]
struct MessageRef {
    timestamp: zx::BootInstant,
    index: u64,
}

impl PartialEq for MessageRef {
    fn eq(&self, other: &Self) -> bool {
        self.timestamp == other.timestamp && self.index == other.index
    }
}

impl PartialOrd for MessageRef {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for MessageRef {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap, but we want min, hence this ordering.
        other.timestamp.cmp(&self.timestamp).then_with(|| other.index.cmp(&self.index))
    }
}

pub struct Message<'a> {
    inner: ConditionGuard<'a, Inner>,
    message: MessageRef,
    pub dropped: u64,
}

impl Message<'_> {
    /// Returns the component and FXT bytes.  The FXT record is validated to be the correct length
    /// and type.
    pub fn parse(&self) -> Result<(&Arc<ComponentIdentity>, Header, &[u8]), MessageError> {
        // SAFETY: We hold a lock which prevents the buffer from being drained so
        // it should be safe to read this range.
        let (container, data, _) =
            unsafe { self.inner.parse_message(self.message.index..self.inner.last_scanned) };
        let container_id = container;
        let (mut header, _) = Header::read_from_prefix(data)
            .map_err(|_| MessageError::from(ParseError::InvalidHeader))?;
        let msg_len = header.size_words() as usize * 8;
        if msg_len > data.len() || msg_len < 16 {
            return Err(ParseError::ValueOutOfValidRange.into());
        }
        if header.raw_type() != TRACING_FORMAT_LOG_RECORD_TYPE {
            return Err(ParseError::InvalidRecordType.into());
        }
        header.set_tag(container_id.0);
        let container = self.inner.containers.get(container).unwrap();
        Ok((&container.identity, header, &data[..msg_len]))
    }
}

impl TryFrom<Message<'_>> for LogsData {
    type Error = MessageError;

    fn try_from(value: Message<'_>) -> Result<Self, Self::Error> {
        let (container, _header, data) = value.parse()?;
        let mut data = diagnostics_message::from_structured(container.as_ref().into(), data)?;
        if value.dropped > 0 {
            data.metadata
                .errors
                .get_or_insert(vec![])
                .push(LogError::RolledOutLogs { count: value.dropped });
        }
        Ok(data)
    }
}

/// Like FilterCursor but returns a stream of LogsData.
#[pin_project]
pub struct FilterCursorStream<T> {
    #[pin]
    cursor: FilterCursor,
    phantom: PhantomData<T>,
}

impl<T> Stream for FilterCursorStream<T>
where
    T: for<'a> TryFrom<Message<'a>>,
{
    type Item = T;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        loop {
            match ready!(this.cursor.as_mut().poll_next(cx)) {
                Some(item) => {
                    if let Ok(data) = T::try_from(item) {
                        return Poll::Ready(Some(data));
                    }
                    // The message is bad, just ignore it.
                }
                None => return Poll::Ready(None),
            }
        }
    }
}

impl<T> FusedStream for FilterCursorStream<T>
where
    T: for<'a> TryFrom<Message<'a>>,
{
    fn is_terminated(&self) -> bool {
        self.cursor.is_terminated()
    }
}

impl<T> From<FilterCursor> for FilterCursorStream<T> {
    fn from(cursor: FilterCursor) -> Self {
        Self { cursor, phantom: PhantomData }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logs::shared_buffer::{
        InnerGuard, SharedBuffer, SharedBufferOptions, create_ring_buffer,
    };
    use crate::logs::stats::LogStreamStats;
    use crate::logs::testing::make_message;
    use assert_matches::assert_matches;
    use fidl_fuchsia_diagnostics::StreamMode;
    use futures::StreamExt;
    use selectors::{FastError, parse_component_selector};
    use std::future::poll_fn;
    use std::pin::pin;
    use std::time::Duration;

    fn test_stats() -> Arc<LogStreamStats> {
        Arc::new(LogStreamStats::new(
            &fuchsia_inspect::Node::default(),
            &ComponentIdentity::unknown(),
        ))
    }

    #[fuchsia::test]
    async fn cursor_basic() {
        let buffer = SharedBuffer::new(
            create_ring_buffer(65536),
            Box::new(|_| {}),
            Default::default(),
            &fuchsia_inspect::Node::default(),
        );
        let container = buffer.new_container_buffer(Arc::new(vec!["a"].into()), test_stats());
        let msg = make_message("a", None, zx::BootInstant::from_nanos(1));
        container.push_back(msg.bytes());

        let cursor = buffer.cursor(StreamMode::Snapshot, vec![]);
        let mut stream = pin!(FilterCursorStream::<LogsData>::from(cursor));

        let item = stream.next().await.unwrap();
        assert_eq!(item.msg().unwrap(), "a");
        assert!(stream.next().await.is_none());
    }

    #[fuchsia::test]
    async fn cursor_filter() {
        let buffer = SharedBuffer::new(
            create_ring_buffer(65536),
            Box::new(|_| {}),
            Default::default(),
            &fuchsia_inspect::Node::default(),
        );
        let container_a = buffer.new_container_buffer(Arc::new(vec!["a"].into()), test_stats());
        let container_b = buffer.new_container_buffer(Arc::new(vec!["b"].into()), test_stats());

        container_a.push_back(make_message("msg_a", None, zx::BootInstant::from_nanos(1)).bytes());
        container_b.push_back(make_message("msg_b", None, zx::BootInstant::from_nanos(2)).bytes());

        let selector = parse_component_selector::<FastError>("a").unwrap();
        let cursor = buffer.cursor(StreamMode::Snapshot, vec![selector]);
        let mut stream = pin!(FilterCursorStream::<LogsData>::from(cursor));

        let item = stream.next().await.unwrap();
        assert_eq!(item.msg().unwrap(), "msg_a");
        assert!(stream.next().await.is_none());
    }

    #[fuchsia::test]
    async fn cursor_subscribe() {
        let buffer = SharedBuffer::new(
            create_ring_buffer(65536),
            Box::new(|_| {}),
            Default::default(),
            &fuchsia_inspect::Node::default(),
        );
        let container = buffer.new_container_buffer(Arc::new(vec!["a"].into()), test_stats());

        let cursor = buffer.cursor(StreamMode::Subscribe, vec![]);
        let mut stream = pin!(FilterCursorStream::<LogsData>::from(cursor));

        assert!(futures::poll!(stream.next()).is_pending());

        container.push_back(make_message("msg", None, zx::BootInstant::from_nanos(1)).bytes());

        let item = stream.next().await.unwrap();
        assert_eq!(item.msg().unwrap(), "msg");
    }

    #[fuchsia::test]
    async fn cursor_snapshot_then_subscribe() {
        let buffer = SharedBuffer::new(
            create_ring_buffer(65536),
            Box::new(|_| {}),
            Default::default(),
            &fuchsia_inspect::Node::default(),
        );
        let container = buffer.new_container_buffer(Arc::new(vec!["a"].into()), test_stats());

        container.push_back(make_message("msg1", None, zx::BootInstant::from_nanos(1)).bytes());

        let cursor = buffer.cursor(StreamMode::SnapshotThenSubscribe, vec![]);
        let mut stream = pin!(FilterCursorStream::<LogsData>::from(cursor));

        let item = stream.next().await.unwrap();
        assert_eq!(item.msg().unwrap(), "msg1");

        assert!(futures::poll!(stream.next()).is_pending());

        container.push_back(make_message("msg2", None, zx::BootInstant::from_nanos(2)).bytes());

        let item = stream.next().await.unwrap();
        assert_eq!(item.msg().unwrap(), "msg2");
    }

    #[fuchsia::test]
    async fn cursor_dropped_logs() {
        let buffer = SharedBuffer::new(
            create_ring_buffer(65536),
            Box::new(|_| {}),
            SharedBufferOptions { sleep_time: Duration::ZERO, ..Default::default() },
            &fuchsia_inspect::Node::default(),
        );
        let container_a = buffer.new_container_buffer(Arc::new(vec!["a"].into()), test_stats());
        let container_b = buffer.new_container_buffer(Arc::new(vec!["b"].into()), test_stats());

        let msg = make_message("msg", None, zx::BootInstant::from_nanos(1));

        container_a.push_back(msg.bytes()); // 1
        container_a.push_back(msg.bytes()); // 2
        container_a.push_back(msg.bytes()); // 3

        let cursor = buffer.cursor(StreamMode::SnapshotThenSubscribe, vec![]);
        let mut stream = pin!(FilterCursorStream::<LogsData>::from(cursor));

        // Consume one
        let item = stream.next().await.unwrap();
        assert_eq!(item.msg().unwrap(), "msg");

        let has_messages = async |component| {
            let mut cursor = pin!(buffer.cursor(
                StreamMode::Snapshot,
                vec![parse_component_selector::<FastError>(component).unwrap()],
            ));
            poll_fn(|cx| cursor.as_mut().poll_next(cx).map(|i| i.is_some())).await
        };

        // Force roll out of remaining A messages
        while has_messages("a").await {
            container_b.push_back(msg.bytes());
            // Force the buffer to pop old messages.
            {
                let mut inner = InnerGuard::new(&buffer);
                inner.check_space(inner.ring_buffer.head());
            }
        }

        // Next item should indicate the two dropped messages for container A.
        let item = stream.next().await.unwrap();
        assert_eq!(item.metadata.errors.as_ref().unwrap().len(), 1);
        assert_matches!(
            item.metadata.errors.as_ref().unwrap()[0],
            LogError::RolledOutLogs { count } if count == 2
        );
    }

    #[fuchsia::test]
    async fn terminate_terminates_cursor() {
        let buffer = SharedBuffer::new(
            create_ring_buffer(65536),
            Box::new(|_| {}),
            SharedBufferOptions { sleep_time: Duration::from_secs(10), ..Default::default() },
            &fuchsia_inspect::Node::default(),
        );
        let cursor = pin!(FilterCursorStream::<LogsData>::from(
            buffer.cursor(StreamMode::SnapshotThenSubscribe, vec![])
        ));
        let container = buffer.new_container_buffer(Arc::new(vec!["a"].into()), test_stats());
        container.push_back(make_message("msg", None, zx::BootInstant::from_nanos(1)).bytes());
        drop(buffer.terminate());
        assert_eq!(cursor.count().await, 1);
    }

    #[fuchsia::test]
    async fn cursor_descending_timestamps_bounded_heap() {
        let buffer = SharedBuffer::new(
            create_ring_buffer(65536),
            Box::new(|_| {}),
            SharedBufferOptions { sleep_time: Duration::ZERO, ..Default::default() },
            &fuchsia_inspect::Node::default(),
        );
        let identity = Arc::new(vec!["a"].into());
        let container = buffer.new_container_buffer(identity, test_stats());
        let cursor = buffer.cursor(StreamMode::SnapshotThenSubscribe, vec![]);
        let mut stream = pin!(FilterCursorStream::<LogsData>::from(cursor));

        // Write enough messages to roll out old messages and exceed the HEAP_PRUNE_THRESHOLD.
        // We interleave large and small timestamps so the large ones remain in the heap.
        for i in 0..5000 {
            let msg_large = make_message(
                &format!("msg_large_{}", i),
                None,
                zx::BootInstant::from_nanos(2000000 + i as i64),
            );
            container.push_back(msg_large.bytes());
            let msg_small = make_message(
                &format!("msg_small_{}", i),
                None,
                zx::BootInstant::from_nanos(1000000 - i as i64),
            );
            container.push_back(msg_small.bytes());

            {
                let mut inner = InnerGuard::new(&buffer);
                inner.check_space(inner.ring_buffer.head());
            }

            let item = stream.next().await.unwrap();
            assert_eq!(item.msg().unwrap(), &format!("msg_small_{}", i));
        }

        // Without the prune logic, the large messages would stay in the heap forever,
        // making the heap size >= 5000.
        // With the prune logic, they are pruned once they exceed live_messages + PRUNE_THRESHOLD.
        let heap_len = stream.as_mut().project().cursor.project().messages.len();
        assert!(heap_len <= 3000, "Heap size: {}", heap_len);
    }
}
