// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::diagnostics::BatchIteratorConnectionStats;
use crate::error::AccessorError;
use crate::logs::servers::{ExtendRecordOpts, extend_fxt_record};
use crate::logs::shared_buffer::FilterCursor;
use diagnostics_log_encoding::encode::{Encoder, EncoderOpts, ResizableBuffer};
use diagnostics_log_encoding::{Argument, Header, LOG_CONTROL_BIT, Record};
use fidl_fuchsia_diagnostics_types::Severity;

use fidl_fuchsia_diagnostics::{
    DataType, Format, FormattedContent, MAXIMUM_ENTRIES_PER_BATCH, StreamMode,
};

use fuchsia_async as fasync;
use futures::{Stream, StreamExt};
use log::warn;
use pin_project::pin_project;
use serde::Serialize;
use std::io::Cursor;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use zerocopy::{FromBytes, IntoBytes};
use zx;

static SERIALIZED_DATA_VMO_NAME: zx::Name = zx::Name::new_lossy("archivist-serialized-data");
static PACKET_BUFFER_VMO_NAME: zx::Name = zx::Name::new_lossy("archivist-packet-buffer");

const SNAPSHOT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

pub type FormattedStream =
    Pin<Box<dyn Stream<Item = Vec<Result<FormattedContent, AccessorError>>> + Send>>;

#[pin_project]
pub struct FormattedContentBatcher<C> {
    #[pin]
    items: C,
    stats: Arc<BatchIteratorConnectionStats>,
}

/// Make a new `FormattedContentBatcher` with a chunking strategy depending on stream mode.
///
/// In snapshot mode, batched items will not be flushed to the client until the batch is complete,
/// the underlying stream has terminated, or `SNAPSHOT_TIMEOUT` has passed since the first
/// item was discovered (in order to prevent client starvation).
///
/// In subscribe or snapshot-then-subscribe mode, batched items will be flushed whenever the
/// underlying stream is pending, ensuring clients always receive latest results.
pub fn new_batcher<I, T, E>(
    items: I,
    stats: Arc<BatchIteratorConnectionStats>,
    mode: StreamMode,
) -> FormattedStream
where
    I: Stream<Item = Result<T, E>> + Send + 'static,
    T: Into<FormattedContent> + Send,
    E: Into<AccessorError> + Send,
{
    match mode {
        StreamMode::Subscribe | StreamMode::SnapshotThenSubscribe => {
            Box::pin(FormattedContentBatcher {
                items: items.ready_chunks(MAXIMUM_ENTRIES_PER_BATCH as _),
                stats,
            })
        }
        StreamMode::Snapshot => Box::pin(FormattedContentBatcher {
            items: items.time_limited_chunks(MAXIMUM_ENTRIES_PER_BATCH as usize, SNAPSHOT_TIMEOUT),
            stats,
        }),
    }
}

impl<I, T, E> Stream for FormattedContentBatcher<I>
where
    I: Stream<Item = Vec<Result<T, E>>>,
    T: Into<FormattedContent>,
    E: Into<AccessorError>,
{
    type Item = Vec<Result<FormattedContent, AccessorError>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.project();
        match this.items.poll_next(cx) {
            Poll::Ready(Some(chunk)) => {
                // loop over chunk instead of into_iter/map because we can't move `this`
                let mut batch = vec![];
                for item in chunk {
                    let result = match item {
                        Ok(i) => Ok(i.into()),
                        Err(e) => {
                            this.stats.add_result_error();
                            Err(e.into())
                        }
                    };
                    batch.push(result);
                }
                Poll::Ready(Some(batch))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimerState {
    Idle,
    Running,
}

pub trait TimeLimitedChunksExt: Stream {
    fn time_limited_chunks(
        self,
        capacity: usize,
        timeout: std::time::Duration,
    ) -> TimeLimitedChunks<Self>
    where
        Self: Sized,
    {
        TimeLimitedChunks::new(self, capacity, timeout)
    }
}

impl<T: Stream> TimeLimitedChunksExt for T {}

/// A stream adapter that yields chunks of items from the underlying stream.
///
/// Chunks are yielded when they reach the specified capacity or when the specified timeout
/// expires since the first item in the chunk was received.
#[pin_project]
pub struct TimeLimitedChunks<S: Stream> {
    #[pin]
    stream: S,
    capacity: usize,
    timeout: std::time::Duration,
    buffer: Vec<S::Item>,
    #[pin]
    timer: fasync::Timer,
    state: TimerState,
}

impl<S: Stream> TimeLimitedChunks<S> {
    pub fn new(stream: S, capacity: usize, timeout: std::time::Duration) -> Self {
        Self {
            stream,
            capacity,
            timeout,
            buffer: Vec::with_capacity(capacity),
            timer: fasync::Timer::new(std::time::Instant::now() + timeout),
            state: TimerState::Idle,
        }
    }
}

impl<S: Stream> Stream for TimeLimitedChunks<S> {
    type Item = Vec<S::Item>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        loop {
            match this.stream.as_mut().poll_next(cx) {
                Poll::Ready(Some(item)) => {
                    if this.buffer.is_empty() {
                        let deadline = fasync::MonotonicInstant::after(
                            zx::MonotonicDuration::from_nanos(this.timeout.as_nanos() as i64),
                        );
                        this.timer.as_mut().reset(deadline);
                        *this.state = TimerState::Running;
                    }
                    this.buffer.push(item);
                    if this.buffer.len() >= *this.capacity {
                        *this.state = TimerState::Idle;
                        return Poll::Ready(Some(std::mem::take(this.buffer)));
                    }
                }
                Poll::Ready(None) => {
                    if !this.buffer.is_empty() {
                        *this.state = TimerState::Idle;
                        return Poll::Ready(Some(std::mem::take(this.buffer)));
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => {
                    if !this.buffer.is_empty() && *this.state == TimerState::Running {
                        use std::future::Future;
                        if this.timer.as_mut().poll(cx).is_ready() {
                            *this.state = TimerState::Idle;
                            return Poll::Ready(Some(std::mem::take(this.buffer)));
                        }
                    }
                    return Poll::Pending;
                }
            }
        }
    }
}

/// Holds a VMO containing valid serialized data as well as the size of that data.
pub struct SerializedVmo {
    pub vmo: zx::Vmo,
    pub size: u64,
    format: Format,
}

impl SerializedVmo {
    pub fn serialize(
        source: &impl Serialize,
        data_type: DataType,
        format: Format,
    ) -> Result<Self, AccessorError> {
        let initial_buffer_capacity = match data_type {
            DataType::Inspect => inspect_format::constants::DEFAULT_VMO_SIZE_BYTES,
            // Logs won't go through this codepath anyway, but in case we ever want to serialize a
            // single log instance it makes sense to start at the page size.
            DataType::Logs => 4096, // page size
        };
        let mut buffer = Vec::with_capacity(initial_buffer_capacity);
        match format {
            Format::Json => {
                serde_json::to_writer(&mut buffer, source).map_err(AccessorError::Serialization)?
            }
            Format::Cbor => ciborium::into_writer(source, &mut buffer)
                .map_err(|err| AccessorError::CborSerialization(err.into()))?,
            Format::Text => unreachable!("We'll never get Text"),
            Format::Fxt | Format::LegacyFxt => unreachable!("We'll never get FXT"),
        }
        let vmo = zx::Vmo::create(buffer.len() as u64).unwrap();
        let _ = vmo.set_name(&SERIALIZED_DATA_VMO_NAME);
        vmo.write(&buffer, 0).unwrap();
        Ok(Self { vmo, size: buffer.len() as u64, format })
    }
}

pub fn encode_rolled_out(count: u64) -> Vec<u8> {
    let mut encoder =
        Encoder::new(Cursor::new(ResizableBuffer::from(Vec::new())), EncoderOpts::default());
    let record = Record {
        timestamp: zx::BootInstant::from_nanos(0),
        severity: Severity::Info.into_primitive(),
        arguments: vec![Argument::new(
            "rolled_out",
            diagnostics_log_encoding::Value::UnsignedInt(count),
        )],
    };
    // This write should not fail as ResizableBuffer will grow as needed.
    encoder.write_record(record).unwrap();

    let mut buffer = encoder.take().into_inner().into_inner();
    if buffer.len() >= 8 {
        let mut header = Header::read_from_bytes(&buffer[0..8]).unwrap();
        header.set_tag(LOG_CONTROL_BIT);
        buffer[0..8].copy_from_slice(header.as_bytes());
    }
    buffer
}

impl From<SerializedVmo> for FormattedContent {
    fn from(content: SerializedVmo) -> FormattedContent {
        match content.format {
            Format::Json => {
                // set_content_size() is redundant, but consumers may expect the size there.
                content
                    .vmo
                    .set_content_size(&content.size)
                    .expect("set_content_size always returns Ok");
                FormattedContent::Json(fidl_fuchsia_mem::Buffer {
                    vmo: content.vmo,
                    size: content.size,
                })
            }
            Format::Cbor => {
                content
                    .vmo
                    .set_content_size(&content.size)
                    .expect("set_content_size always returns Ok");
                FormattedContent::Cbor(content.vmo)
            }
            Format::Fxt | Format::LegacyFxt => {
                content
                    .vmo
                    .set_content_size(&content.size)
                    .expect("set_content_size always returns Ok");
                FormattedContent::Fxt(content.vmo)
            }
            Format::Text => unreachable!("We'll never get Text"),
        }
    }
}

trait PacketFormat {
    const FORMAT: Format;
    const HEADER: &[u8] = &[];
    const FOOTER: &[u8] = &[];

    /// Writes an item in the required format.  Returns `Poll::Ready(Some(<separator length>))` if
    /// an item was written, and `Poll::Ready(None)` if there are no more items.  If `first` is true,
    /// this is the first item in this batch.
    fn write_item(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        first: bool,
        buffer: &mut Vec<u8>,
    ) -> Poll<Option<usize>>;
}

#[pin_project]
pub struct PacketSerializer<T> {
    stats: Option<Arc<BatchIteratorConnectionStats>>,
    max_packet_size: u64,
    #[pin]
    format: T,
    overflow: Vec<u8>,
    finished: bool,
}

impl<T> PacketSerializer<T> {
    fn with_format(
        stats: Option<Arc<BatchIteratorConnectionStats>>,
        max_packet_size: u64,
        format: T,
    ) -> Self {
        Self { stats, max_packet_size, format, overflow: Vec::new(), finished: false }
    }
}

impl<T: PacketFormat> Stream for PacketSerializer<T> {
    type Item = Result<SerializedVmo, AccessorError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.finished {
            return Poll::Ready(None);
        }

        // Limit packet size to prevent unbounded memory use.
        const MAX_PACKET_SIZE_LIMIT: u64 = 1 << 20; // 1 MiB
        let max_packet_size = std::cmp::min(self.max_packet_size, MAX_PACKET_SIZE_LIMIT);
        let mut this = self.project();

        let mut buffer = Vec::with_capacity(256 * 1024);
        buffer.extend_from_slice(T::HEADER);

        let mut results = 0;

        if !this.overflow.is_empty() {
            buffer.append(this.overflow);
            results += 1;
        }

        let mut vmo = None;
        let mut vmo_len = 0;

        loop {
            // Copy to the VMO if the room in the buffer drops below a threshold.
            if buffer.capacity() - buffer.len() < 512 {
                vmo.get_or_insert_with(|| {
                    let v = zx::Vmo::create(max_packet_size).unwrap();
                    let _ = v.set_name(&PACKET_BUFFER_VMO_NAME);
                    v
                })
                .write(&buffer, vmo_len as u64)
                .unwrap();
                vmo_len += buffer.len();
                buffer.clear();
            }

            let last_len = buffer.len();

            let separator_len = match this.format.as_mut().write_item(cx, results == 0, &mut buffer)
            {
                Poll::Ready(Some(separator_len)) => separator_len,
                Poll::Ready(None) => {
                    *this.finished = true;
                    if results == 0 {
                        return Poll::Ready(None);
                    } else {
                        break;
                    }
                }
                Poll::Pending => {
                    if results == 0 {
                        return Poll::Pending;
                    } else {
                        break;
                    }
                }
            };

            let item_len = buffer.len() - last_len - separator_len;

            if (item_len + T::HEADER.len() + T::FOOTER.len()) as u64 >= max_packet_size {
                warn!("dropping oversize item (limit={max_packet_size} len={item_len})");
                buffer.truncate(last_len);
            } else {
                if (vmo_len + buffer.len() + T::FOOTER.len()) as u64 > max_packet_size {
                    // Last item put us over the maximum packet size, keep it for the next batch.
                    // We should have at least one item because otherwise we should have gone
                    // through the branch above.
                    assert!(results > 0);
                    this.overflow.extend_from_slice(&buffer[last_len + separator_len..]);
                    buffer.truncate(last_len);
                    break;
                }

                results += 1;
            }
        }

        if let Some(stats) = &this.stats {
            stats.add_result(results);
        }

        buffer.extend_from_slice(T::FOOTER);

        let vmo = match vmo {
            Some(vmo) => {
                vmo.set_stream_size((vmo_len + buffer.len()) as u64).unwrap();
                vmo
            }
            None => {
                let v = zx::Vmo::create(buffer.len() as u64).unwrap();
                let _ = v.set_name(&PACKET_BUFFER_VMO_NAME);
                v
            }
        };
        vmo.write(&buffer, vmo_len as u64).unwrap();
        vmo_len += buffer.len();
        Poll::Ready(Some(Ok(SerializedVmo { vmo, size: vmo_len as u64, format: T::FORMAT })))
    }
}

#[pin_project]
pub struct FxtPacketFormat {
    #[pin]
    pub cursor: FilterCursor,
    pub subscribe_to_manifest: bool,
    pub sent_tags: std::collections::HashMap<u64, Arc<crate::identity::ComponentIdentity>>,
}

impl PacketFormat for FxtPacketFormat {
    const FORMAT: Format = Format::LegacyFxt;

    fn write_item(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        _first: bool,
        buffer: &mut Vec<u8>,
    ) -> Poll<Option<usize>> {
        let mut this = self.project();
        loop {
            let Some(message) = ready!(this.cursor.as_mut().poll_next(cx)) else {
                return Poll::Ready(None);
            };

            let (identity, header, data) = match message.parse() {
                Ok(res) => res,
                Err(_) => {
                    // If we fail to parse the message, just ignore it and move on to the next
                    // message.
                    continue;
                }
            };
            let tag = header.tag() as u64;

            if *this.subscribe_to_manifest {
                let send_manifest = match this.sent_tags.entry(tag) {
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(Arc::clone(identity));
                        true
                    }
                    std::collections::hash_map::Entry::Occupied(mut e) => {
                        if !Arc::ptr_eq(e.get(), identity) && **e.get() != **identity {
                            e.insert(Arc::clone(identity));
                            true
                        } else {
                            false
                        }
                    }
                };

                if message.dropped > 0 {
                    let rolled_out_buffer = encode_rolled_out(message.dropped);
                    buffer.extend_from_slice(&rolled_out_buffer);
                }

                if send_manifest {
                    use diagnostics_log_encoding::encode::{Encoder, EncoderOpts, ResizableBuffer};
                    use diagnostics_log_encoding::{Argument, LOG_CONTROL_BIT, Record};
                    use fidl_fuchsia_diagnostics_types::Severity;
                    use std::io::Cursor;

                    let mut encoder = Encoder::new(
                        Cursor::new(ResizableBuffer::from(Vec::new())),
                        EncoderOpts::default(),
                    );
                    let record = Record {
                        timestamp: zx::BootInstant::from_nanos(0),
                        severity: Severity::Info.into_primitive(),
                        arguments: vec![
                            Argument::other("moniker", identity.moniker.to_string()),
                            Argument::other("url", identity.url.as_str()),
                        ],
                    };
                    encoder.write_record(record).unwrap();
                    let mut manifest_buffer = encoder.take().into_inner().into_inner();
                    if manifest_buffer.len() >= 8 {
                        let mut header_manifest =
                            Header::read_from_bytes(&manifest_buffer[0..8]).unwrap();
                        header_manifest.set_tag((tag as u32) | LOG_CONTROL_BIT);
                        manifest_buffer[0..8].copy_from_slice(header_manifest.as_bytes());
                    }
                    buffer.extend_from_slice(&manifest_buffer);
                }
            }

            buffer.extend_from_slice(header.as_bytes());
            buffer.extend_from_slice(&data[8..]);

            extend_fxt_record(
                identity,
                message.dropped,
                &ExtendRecordOpts {
                    component_url: !*this.subscribe_to_manifest,
                    moniker: !*this.subscribe_to_manifest,
                    rolled_out: !*this.subscribe_to_manifest,
                    subscribe_to_manifest: false,
                },
                buffer,
            );
            return Poll::Ready(Some(0));
        }
    }
}

pub type FxtPacketSerializer = PacketSerializer<FxtPacketFormat>;

impl FxtPacketSerializer {
    pub fn new(
        stats: Arc<BatchIteratorConnectionStats>,
        max_packet_size: u64,
        cursor: FilterCursor,
        subscribe_to_manifest: bool,
    ) -> Self {
        Self::with_format(
            Some(stats),
            max_packet_size,
            FxtPacketFormat {
                cursor,
                subscribe_to_manifest,
                sent_tags: std::collections::HashMap::new(),
            },
        )
    }
}

#[pin_project]
pub struct JsonPacketFormat<I>(#[pin] I);

impl<I: Stream<Item = impl Serialize>> PacketFormat for JsonPacketFormat<I> {
    const FORMAT: Format = Format::Json;
    const HEADER: &[u8] = b"[";
    const FOOTER: &[u8] = b"]";

    fn write_item(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        first: bool,
        buffer: &mut Vec<u8>,
    ) -> Poll<Option<usize>> {
        const SEPARATOR: &[u8] = b",\n";

        if let Some(item) = ready!(self.project().0.poll_next(cx)) {
            let separator_len = if !first {
                buffer.extend_from_slice(SEPARATOR);
                SEPARATOR.len()
            } else {
                0
            };
            // We don't expect serialization to fail because we should always be able to write to
            // `buffer` and `item` is a type we control which we know should always be serializable.
            serde_json::to_writer(buffer, &item).expect("failed to serialize item");
            Poll::Ready(Some(separator_len))
        } else {
            Poll::Ready(None)
        }
    }
}

pub type JsonPacketSerializer<I> = PacketSerializer<JsonPacketFormat<I>>;

impl<I> JsonPacketSerializer<I> {
    pub fn new(stats: Arc<BatchIteratorConnectionStats>, max_packet_size: u64, items: I) -> Self {
        Self::with_format(Some(stats), max_packet_size, JsonPacketFormat(items))
    }

    pub fn new_without_stats(max_packet_size: u64, items: I) -> Self {
        Self::with_format(None, max_packet_size, JsonPacketFormat(items))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::AccessorStats;
    use futures::stream::iter;

    #[fuchsia::test]
    async fn time_limited_chunks_yields_on_capacity() {
        let stream = futures::stream::iter(vec![1, 2, 3, 4]);
        let mut chunks =
            Box::pin(TimeLimitedChunks::new(stream, 2, std::time::Duration::from_secs(30)));

        assert_eq!(chunks.next().await, Some(vec![1, 2]));
        assert_eq!(chunks.next().await, Some(vec![3, 4]));
        assert_eq!(chunks.next().await, None);
    }

    #[fuchsia::test]
    async fn time_limited_chunks_yields_on_stream_end() {
        let stream = futures::stream::iter(vec![1, 2, 3]);
        let mut chunks =
            Box::pin(TimeLimitedChunks::new(stream, 2, std::time::Duration::from_secs(30)));

        assert_eq!(chunks.next().await, Some(vec![1, 2]));
        assert_eq!(chunks.next().await, Some(vec![3]));
        assert_eq!(chunks.next().await, None);
    }

    #[fuchsia::test]
    async fn time_limited_chunks_yields_on_timeout() {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let mut chunks =
            Box::pin(TimeLimitedChunks::new(rx, 2, std::time::Duration::from_millis(10)));

        tx.unbounded_send(1).unwrap();

        let start = std::time::Instant::now();
        assert_eq!(chunks.next().await, Some(vec![1]));
        assert!(start.elapsed() >= std::time::Duration::from_millis(10));

        tx.unbounded_send(2).unwrap();
        tx.unbounded_send(3).unwrap();
        assert_eq!(chunks.next().await, Some(vec![2, 3]));

        drop(tx);
        assert_eq!(chunks.next().await, None);
    }

    #[fuchsia::test]
    async fn two_items_joined_and_split() {
        let inputs = &[&"FFFFFFFFFF", &"GGGGGGGGGG"];
        let joined = &["[\"FFFFFFFFFF\",\n\"GGGGGGGGGG\"]"];
        let split = &[r#"["FFFFFFFFFF"]"#, r#"["GGGGGGGGGG"]"#];
        let smallest_possible_joined_len = joined[0].len() as u64;

        let make_packets = |max| async move {
            let node = fuchsia_inspect::Node::default();
            let accessor_stats = Arc::new(AccessorStats::new(node));
            let test_stats = Arc::new(accessor_stats.new_logs_batch_iterator());
            JsonPacketSerializer::new(test_stats, max, iter(inputs.iter()))
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .map(|r| {
                    let result = r.unwrap();
                    let mut buf = vec![0; result.size as usize];
                    result.vmo.read(&mut buf, 0).expect("reading vmo");
                    std::str::from_utf8(&buf).unwrap().to_string()
                })
                .collect::<Vec<_>>()
        };

        let actual_joined = make_packets(smallest_possible_joined_len).await;
        assert_eq!(&actual_joined[..], joined);

        let actual_split = make_packets(smallest_possible_joined_len - 1).await;
        assert_eq!(&actual_split[..], split);
    }

    #[fuchsia::test]
    async fn overflow_separator_added() {
        let inputs = &[&"A", &"B", &"C"];
        // "[" + "A" + "]" = 4 bytes.
        // "[" + "A" + ",\n" + "B" + "]" = 4 + 2 + 3 = 9 bytes.
        // If max is 8, "B" will overflow.
        // Second packet starts with "B" from overflow.
        // "[" + "B" + ",\n" + "C" + "]" = 4 + 2 + 3 = 9 bytes.
        // If max is 8, "C" will overflow.
        // Third packet starts with "C" from overflow.

        let make_packets = |max| async move {
            let node = fuchsia_inspect::Node::default();
            let accessor_stats = Arc::new(AccessorStats::new(node));
            let test_stats = Arc::new(accessor_stats.new_logs_batch_iterator());
            JsonPacketSerializer::new(test_stats, max, iter(inputs.iter()))
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .map(|r| {
                    let result = r.unwrap();
                    let mut buf = vec![0; result.size as usize];
                    result.vmo.read(&mut buf, 0).expect("reading vmo");
                    std::str::from_utf8(&buf).unwrap().to_string()
                })
                .collect::<Vec<_>>()
        };

        let packets = make_packets(8).await;
        assert_eq!(packets.len(), 3);
        assert_eq!(packets[0], r#"["A"]"#);
        assert_eq!(packets[1], r#"["B"]"#);
        assert_eq!(packets[2], r#"["C"]"#);

        let packets = make_packets(10).await;
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0], "[\"A\",\n\"B\"]");
        assert_eq!(packets[1], r#"["C"]"#);
    }

    #[fuchsia::test]
    async fn oversize_item_not_dropped_incorrectly() {
        let inputs = &[&"A", &"BCDEF"];
        // Packet 1: ["A"] (4 bytes)
        // Item 2: "BCDEF" (7 bytes)
        // "[" + "A" + ",\n" + "BCDEF" + "]" = 1 + 3 + 2 + 7 + 1 = 14 bytes.
        // If max is 11:
        // "A" fits (4 bytes).
        // "BCDEF" overflows.
        // NEXT packet:
        // "[" + "BCDEF" + "]" = 9 bytes.
        // 9 fits in 11.

        let make_packets = |max| async move {
            let node = fuchsia_inspect::Node::default();
            let accessor_stats = Arc::new(AccessorStats::new(node));
            let test_stats = Arc::new(accessor_stats.new_logs_batch_iterator());
            JsonPacketSerializer::new(test_stats, max, iter(inputs.iter()))
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .map(|r| {
                    let result = r.unwrap();
                    let mut buf = vec![0; result.size as usize];
                    result.vmo.read(&mut buf, 0).expect("reading vmo");
                    std::str::from_utf8(&buf).unwrap().to_string()
                })
                .collect::<Vec<_>>()
        };

        let packets = make_packets(11).await;
        assert_eq!(packets.len(), 2);
        assert_eq!(packets[0], r#"["A"]"#);
        assert_eq!(packets[1], r#"["BCDEF"]"#);
    }

    #[fuchsia::test]
    async fn item_too_big_for_packet_is_dropped() {
        let inputs = &[&"ABCDE"]; // 7 bytes
        // "[" + 7 + "]" = 9 bytes.
        // If max is 8, it should be dropped.

        let make_packets = |max| async move {
            let node = fuchsia_inspect::Node::default();
            let accessor_stats = Arc::new(AccessorStats::new(node));
            let test_stats = Arc::new(accessor_stats.new_logs_batch_iterator());
            JsonPacketSerializer::new(test_stats, max, iter(inputs.iter()))
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .map(|r| {
                    let result = r.unwrap();
                    let mut buf = vec![0; result.size as usize];
                    result.vmo.read(&mut buf, 0).expect("reading vmo");
                    std::str::from_utf8(&buf).unwrap().to_string()
                })
                .collect::<Vec<_>>()
        };

        let packets = make_packets(8).await;
        // Item should be dropped, so we get no packets.
        assert_eq!(packets.len(), 0);
    }

    #[fuchsia::test]
    async fn fxt_packet_serializer_subscribe_to_manifest() {
        use crate::identity::ComponentIdentity;
        use crate::logs::shared_buffer::{SharedBuffer, create_ring_buffer};
        use crate::logs::stats::LogStreamStats;
        use diagnostics_log_encoding::encode::{Encoder, EncoderOpts};
        use diagnostics_log_encoding::{Argument, Header, LOG_CONTROL_BIT, Record};
        use fidl_fuchsia_diagnostics::StreamMode;
        use fidl_fuchsia_diagnostics_types::Severity;
        use fuchsia_inspect::Node;
        use std::io::Cursor;
        use zerocopy::{FromBytes, IntoBytes};

        let identity1 = Arc::new(ComponentIdentity::unknown());
        let mut identity2_inner = ComponentIdentity::unknown();
        identity2_inner.url =
            flyweights::FlyStr::new("fuchsia-pkg://fuchsia.com/test#meta/test.cm");
        let identity2 = Arc::new(identity2_inner);

        let buffer = Arc::new(SharedBuffer::new(
            create_ring_buffer(65536),
            Box::new(|_| {}),
            Default::default(),
            &Node::default(),
        ));
        let stats1 = Arc::new(LogStreamStats::new(&Node::default(), &identity1));
        let container1 = buffer.new_container_buffer(Arc::clone(&identity1), stats1);
        let stats2 = Arc::new(LogStreamStats::new(&Node::default(), &identity2));
        let container2 = buffer.new_container_buffer(Arc::clone(&identity2), stats2);

        // create message 1
        let mut buffer_out = Cursor::new(vec![0u8; 128]);
        let mut encoder = Encoder::new(&mut buffer_out, EncoderOpts::default());
        encoder
            .write_record(Record {
                timestamp: zx::BootInstant::from_nanos(100),
                severity: Severity::Info.into_primitive(),
                arguments: vec![Argument::other("msg", "hello")],
            })
            .unwrap();
        let end = encoder.inner().position() as usize;
        let mut msg1_bytes = encoder.inner().get_ref()[..end].to_vec();
        let mut header = Header::read_from_bytes(&msg1_bytes[0..8]).unwrap();
        header.set_tag(1);
        msg1_bytes[0..8].copy_from_slice(header.as_bytes());
        container1.push_back(&msg1_bytes);

        // create message 2
        let mut buffer_out = Cursor::new(vec![0u8; 128]);
        let mut encoder = Encoder::new(&mut buffer_out, EncoderOpts::default());
        encoder
            .write_record(Record {
                timestamp: zx::BootInstant::from_nanos(200),
                severity: Severity::Info.into_primitive(),
                arguments: vec![Argument::other("msg", "world")],
            })
            .unwrap();
        let end = encoder.inner().position() as usize;
        let mut msg2_bytes = encoder.inner().get_ref()[..end].to_vec();
        let mut header = Header::read_from_bytes(&msg2_bytes[0..8]).unwrap();
        header.set_tag(2);
        msg2_bytes[0..8].copy_from_slice(header.as_bytes());
        container2.push_back(&msg2_bytes);

        // create message 3 (same tag/identity as message 2)
        let mut buffer_out = Cursor::new(vec![0u8; 128]);
        let mut encoder = Encoder::new(&mut buffer_out, EncoderOpts::default());
        encoder
            .write_record(Record {
                timestamp: zx::BootInstant::from_nanos(300),
                severity: Severity::Info.into_primitive(),
                arguments: vec![Argument::other("msg", "again")],
            })
            .unwrap();
        let end = encoder.inner().position() as usize;
        let mut msg3_bytes = encoder.inner().get_ref()[..end].to_vec();
        let mut header = Header::read_from_bytes(&msg3_bytes[0..8]).unwrap();
        header.set_tag(2);
        msg3_bytes[0..8].copy_from_slice(header.as_bytes());
        container2.push_back(&msg3_bytes);

        let cursor = buffer.cursor(StreamMode::Snapshot, vec![]);

        let node = Node::default();
        let accessor_stats = Arc::new(AccessorStats::new(node));
        let test_stats = Arc::new(accessor_stats.new_logs_batch_iterator());

        // Test with subscribe_to_manifest = true
        let packets: Vec<_> =
            FxtPacketSerializer::new(Arc::clone(&test_stats), 1024 * 1024, cursor, true)
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .map(|r| {
                    let result = r.unwrap();
                    let mut buf = vec![0; result.size as usize];
                    result.vmo.read(&mut buf, 0).expect("reading vmo");
                    buf
                })
                .collect();

        assert_eq!(packets.len(), 1);
        let output = &packets[0];

        let mut current_slice = output.as_slice();
        let mut records = vec![];

        while !current_slice.is_empty() {
            let (record, remaining) =
                diagnostics_log_encoding::parse::parse_record(current_slice).unwrap();
            let header =
                diagnostics_log_encoding::Header::read_from_bytes(&current_slice[..8]).unwrap();
            records.push((header, record));
            current_slice = remaining;
        }

        assert_eq!(records.len(), 5);

        // Record 0: Manifest for tag 0
        assert_ne!(records[0].0.tag() & LOG_CONTROL_BIT, 0);
        assert_eq!(records[0].0.tag() & !LOG_CONTROL_BIT, 0);

        // Record 1: Msg 1
        assert_eq!(records[1].0.tag(), 0);
        assert_eq!(records[1].1.arguments.len(), 1); // no moniker/url injected

        // Record 2: Manifest for tag 1
        assert_ne!(records[2].0.tag() & LOG_CONTROL_BIT, 0);
        assert_eq!(records[2].0.tag() & !LOG_CONTROL_BIT, 1);

        // Record 3: Msg 2
        assert_eq!(records[3].0.tag(), 1);
        assert_eq!(records[3].1.arguments.len(), 1); // no moniker/url injected

        // Record 4: Msg 3
        assert_eq!(records[4].0.tag(), 1);
        assert_eq!(records[4].1.arguments.len(), 1); // no moniker/url injected
    }
}
