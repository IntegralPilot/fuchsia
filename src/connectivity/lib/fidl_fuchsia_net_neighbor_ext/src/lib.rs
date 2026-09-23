// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Extensions for types in the `fidl_fuchsia_net_neighbor` crate.

#![deny(missing_docs)]

#[cfg(any(test, feature = "testutils"))]
pub mod testutil;

use async_utils::{fold, stream};
use fidl_fuchsia_net_ext as fnet_ext;
use fidl_table_validation::*;
use flex_client::ProxyHasDomain;
use flex_fuchsia_net as fnet;
use flex_fuchsia_net_neighbor as fnet_neighbor;
use futures::{Stream, TryStreamExt as _};
use std::num::NonZeroU64;
use thiserror::Error;
use zx_types as zx;

#[derive(Debug, Error)]
enum ConversionError {
    #[error("interface ID must be non-zero")]
    ZeroInterface,
}

struct NonZeroU64Converter;

impl Converter for NonZeroU64Converter {
    type Fidl = u64;
    type Validated = NonZeroU64;
    type Error = ConversionError;
    fn try_from_fidl(value: Self::Fidl) -> std::result::Result<Self::Validated, Self::Error> {
        NonZeroU64::new(value).ok_or(ConversionError::ZeroInterface)
    }
    fn from_validated(validated: Self::Validated) -> Self::Fidl {
        validated.get()
    }
}

/// Information on a neighboring device in the local network.
#[derive(Clone, Debug, Eq, Hash, PartialEq, ValidFidlTable)]
#[fidl_table_src(fnet_neighbor::Entry)]
#[fidl_table_strict]
pub struct Entry {
    /// Identifier for the interface used for communicating with the neighbor.
    #[fidl_field_type(converter = NonZeroU64Converter)]
    pub interface: NonZeroU64,
    /// IP address of the neighbor.
    pub neighbor: fnet::IpAddress,
    /// State of the entry within the Neighbor Unreachability Detection (NUD)
    /// state machine.
    pub state: fnet_neighbor::EntryState,
    /// MAC address of the neighboring device's network interface controller.
    #[fidl_field_type(optional)]
    pub mac: Option<fnet::MacAddress>,
    /// Timestamp when this entry has changed `state`.
    // TODO(https://fxbug.dev/42155335): Replace with zx::MonotonicInstant once there is
    // support for custom conversion functions.
    pub updated_at: zx::zx_time_t,
}

/// Returns a &str suitable for display representing the EntryState parameter.
pub fn display_entry_state(state: &fnet_neighbor::EntryState) -> &'static str {
    match state {
        fnet_neighbor::EntryState::Incomplete => "INCOMPLETE",
        fnet_neighbor::EntryState::Reachable => "REACHABLE",
        fnet_neighbor::EntryState::Stale => "STALE",
        fnet_neighbor::EntryState::Delay => "DELAY",
        fnet_neighbor::EntryState::Probe => "PROBE",
        fnet_neighbor::EntryState::Static => "STATIC",
        fnet_neighbor::EntryState::Unreachable => "UNREACHABLE",
    }
}

impl std::fmt::Display for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        let Self { interface, neighbor, mac, state, updated_at: _ } = self;
        write!(f, "Interface {} | IP {} | MAC ", interface, fnet_ext::IpAddress::from(*neighbor))?;
        if let Some(mac) = mac {
            write!(f, "{}", fnet_ext::MacAddress::from(*mac))?;
        } else {
            write!(f, "?")?;
        }
        write!(f, " | {}", display_entry_state(state))
    }
}

/// Neighbor entry events.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A neighbor entry that existed prior to watching.
    Existing(Entry),
    /// A neighbor entry that was added.
    Added(Entry),
    /// A neighbor entry that was changed.
    Changed(Entry),
    /// A neighbor entry that was removed.
    Removed(Entry),
    /// Sentinel value indicating that all existing entries have been sent.
    Idle,
}

impl TryFrom<fnet_neighbor::EntryIteratorItem> for Event {
    type Error = EntryValidationError;

    fn try_from(value: fnet_neighbor::EntryIteratorItem) -> Result<Self, Self::Error> {
        match value {
            fnet_neighbor::EntryIteratorItem::Existing(e) => {
                Ok(Event::Existing(Entry::try_from(e)?))
            }
            fnet_neighbor::EntryIteratorItem::Added(e) => Ok(Event::Added(Entry::try_from(e)?)),
            fnet_neighbor::EntryIteratorItem::Changed(e) => Ok(Event::Changed(Entry::try_from(e)?)),
            fnet_neighbor::EntryIteratorItem::Removed(e) => Ok(Event::Removed(Entry::try_from(e)?)),
            fnet_neighbor::EntryIteratorItem::Idle(_) => Ok(Event::Idle),
        }
    }
}

/// Options for modifying the behavior of `EntryIterator`.
#[derive(Clone, Debug, Default, Eq, PartialEq, ValidFidlTable)]
#[fidl_table_src(fnet_neighbor::EntryIteratorOptions)]
#[fidl_table_strict]
pub struct EntryIteratorOptions {}

/// Neighbor table entry iterator creation error.
#[derive(Clone, Debug, Error)]
#[error("failed to open neighbor entry iterator: {0}")]
pub struct OpenEntryIteratorError(fidl::Error);

/// Dispatches `OpenEntryIterator` on the view proxy.
pub fn open_entry_iterator(
    view_proxy: &fnet_neighbor::ViewProxy,
    options: EntryIteratorOptions,
) -> Result<fnet_neighbor::EntryIteratorProxy, OpenEntryIteratorError> {
    let (neighbor_iter_proxy, entry_iter_server_end) =
        view_proxy.domain().create_proxy::<fnet_neighbor::EntryIteratorMarker>();

    view_proxy
        .open_entry_iterator(entry_iter_server_end, &options.into())
        .map_err(OpenEntryIteratorError)?;

    Ok(neighbor_iter_proxy)
}

/// Neighbor table entry iterator `GetNext` errors.
#[derive(Debug, Error)]
pub enum EntryIteratorError {
    /// The call to `GetNext` returned a FIDL error.
    #[error("the call to `GetNext()` failed: {0}")]
    Fidl(fidl::Error),
    /// The event returned by `GetNext` encountered a conversion error.
    #[error("failed to convert event returned by `GetNext()`: {0:?}")]
    Conversion(EntryValidationError),
    /// The server returned an empty batch of events.
    #[error("the call to `GetNext()` returned an empty batch of events")]
    EmptyEventBatch,
}

/// [`event_stream_from_view_with_options`] with default [`EntryIteratorOptions`].
pub fn event_stream_from_view(
    neighbors_view: &fnet_neighbor::ViewProxy,
) -> Result<impl Stream<Item = Result<Event, EntryIteratorError>> + 'static, OpenEntryIteratorError>
{
    event_stream_from_view_with_options(neighbors_view, Default::default())
}

/// Connects to the neighbor table entry iterator protocol with
/// [`EntryIteratorOptions`] and converts the Hanging-Get style API into an
/// Event stream.
///
/// Each call to `GetNext` returns a batch of events, which are flattened into a
/// single stream. If an error is encountered while calling `GetNext` or while
/// converting the event, the stream is immediately terminated.
pub fn event_stream_from_view_with_options(
    neighbors_view: &fnet_neighbor::ViewProxy,
    options: EntryIteratorOptions,
) -> Result<impl Stream<Item = Result<Event, EntryIteratorError>> + 'static, OpenEntryIteratorError>
{
    let neighbor_iterator = open_entry_iterator(neighbors_view, options)?;
    Ok(event_stream_from_iterator(neighbor_iterator))
}

/// Turns the provided neighbor table entry iterator into an [`Event`] stream by applying
/// Hanging-Get watch.
///
/// Each call to `GetNext` returns a batch of events, which are flattened into a
/// single stream. If an error is encountered while calling `GetNext` or while
/// converting the event, the stream is immediately terminated.
pub fn event_stream_from_iterator(
    neighbor_iterator: fnet_neighbor::EntryIteratorProxy,
) -> impl Stream<Item = Result<Event, EntryIteratorError>> {
    stream::ShortCircuit::new(
        futures::stream::try_unfold(neighbor_iterator, |iter| async {
            let events_batch = iter.get_next().await.map_err(EntryIteratorError::Fidl)?;
            if events_batch.is_empty() {
                return Err(EntryIteratorError::EmptyEventBatch);
            }
            let events_batch = events_batch
                .into_iter()
                .map(|event| event.try_into().map_err(EntryIteratorError::Conversion));
            let event_stream = futures::stream::iter(events_batch);
            Ok(Some((event_stream, iter)))
        })
        // Flatten the stream of event streams into a single event stream.
        .try_flatten(),
    )
}

/// Errors returned by [`collect_neighbors_until_idle`].
#[derive(Debug, Error)]
pub enum CollectNeighborsUntilIdleError {
    /// There was an error in the event stream.
    #[error("there was an error in the event stream: {0}")]
    ErrorInStream(EntryIteratorError),
    /// There was an unexpected event in the event stream. Only `existing` or
    /// `idle` events are expected.
    #[error("there was an unexpected event in the event stream: {0:?}")]
    UnexpectedEvent(Event),
    /// The event stream unexpectedly ended.
    #[error("the event stream unexpectedly ended")]
    StreamEnded,
}

/// Collects all `existing` events from the stream, stopping once the `idle`
/// event is observed.
pub async fn collect_neighbors_until_idle<C: Extend<Entry> + Default>(
    event_stream: impl futures::Stream<Item = Result<Event, EntryIteratorError>> + Unpin,
) -> Result<C, CollectNeighborsUntilIdleError> {
    fold::fold_while(
        event_stream,
        Ok(C::default()),
        |existing_neighbors: Result<C, CollectNeighborsUntilIdleError>, event| {
            futures::future::ready(match existing_neighbors {
                Err(_) => {
                    unreachable!(
                        "`existing_neighbors` must be `Ok`, because we stop folding on err"
                    )
                }
                Ok(mut existing_neighbors) => match event {
                    Err(e) => {
                        fold::FoldWhile::Done(Err(CollectNeighborsUntilIdleError::ErrorInStream(e)))
                    }
                    Ok(e) => match e {
                        Event::Existing(e) => {
                            existing_neighbors.extend([e]);
                            fold::FoldWhile::Continue(Ok(existing_neighbors))
                        }
                        Event::Idle => fold::FoldWhile::Done(Ok(existing_neighbors)),
                        e @ Event::Added(_) | e @ Event::Changed(_) | e @ Event::Removed(_) => {
                            fold::FoldWhile::Done(Err(
                                CollectNeighborsUntilIdleError::UnexpectedEvent(e),
                            ))
                        }
                    },
                },
            })
        },
    )
    .await
    .short_circuited()
    .map_err(|_accumulated_thus_far: Result<C, CollectNeighborsUntilIdleError>| {
        CollectNeighborsUntilIdleError::StreamEnded
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    use assert_matches::assert_matches;
    use futures::{FutureExt, StreamExt as _};
    use test_case::test_case;

    fn valid_fidl_entry(interface: NonZeroU64) -> fnet_neighbor::Entry {
        fnet_neighbor::Entry {
            interface: Some(interface.get()),
            neighbor: Some(fnet::IpAddress::Ipv4(fnet::Ipv4Address { addr: [192, 168, 0, 1] })),
            state: Some(fnet_neighbor::EntryState::Reachable),
            mac: Some(fnet::MacAddress { octets: [0, 1, 2, 3, 4, 5] }),
            updated_at: Some(123456),
            ..Default::default()
        }
    }

    #[test]
    fn event_try_from_success() {
        let fidl_entry = valid_fidl_entry(NonZeroU64::new(1).unwrap());
        let local_entry: Entry = fidl_entry.clone().try_into().unwrap();
        assert_matches!(
            fnet_neighbor::EntryIteratorItem::Existing(fidl_entry.clone()).try_into(),
            Ok(Event::Existing(entry)) if entry == local_entry
        );
        assert_matches!(
            fnet_neighbor::EntryIteratorItem::Added(fidl_entry.clone()).try_into(),
            Ok(Event::Added(entry)) if entry == local_entry
        );
        assert_matches!(
            fnet_neighbor::EntryIteratorItem::Changed(fidl_entry.clone()).try_into(),
            Ok(Event::Changed(entry)) if entry == local_entry
        );
        assert_matches!(
            fnet_neighbor::EntryIteratorItem::Removed(fidl_entry.clone()).try_into(),
            Ok(Event::Removed(entry)) if entry == local_entry
        );
        assert_matches!(
            fnet_neighbor::EntryIteratorItem::Idle(fnet_neighbor::IdleEvent).try_into(),
            Ok(Event::Idle)
        );
    }

    #[test]
    fn event_try_from_missing_field() {
        let fidl_entry = fnet_neighbor::Entry {
            interface: None, // Required field omitted.
            ..valid_fidl_entry(NonZeroU64::new(1).unwrap())
        };
        assert_matches!(
            Event::try_from(fnet_neighbor::EntryIteratorItem::Existing(fidl_entry.clone())),
            Err(EntryValidationError::MissingField(_))
        );
        assert_matches!(
            Event::try_from(fnet_neighbor::EntryIteratorItem::Added(fidl_entry.clone())),
            Err(EntryValidationError::MissingField(_))
        );
        assert_matches!(
            Event::try_from(fnet_neighbor::EntryIteratorItem::Changed(fidl_entry.clone())),
            Err(EntryValidationError::MissingField(_))
        );
        assert_matches!(
            Event::try_from(fnet_neighbor::EntryIteratorItem::Removed(fidl_entry.clone())),
            Err(EntryValidationError::MissingField(_))
        );
    }

    #[test]
    fn entry_try_from_zero_interface() {
        let fidl_entry = fnet_neighbor::Entry {
            interface: Some(0),
            ..valid_fidl_entry(NonZeroU64::new(1).unwrap())
        };
        assert_matches!(Entry::try_from(fidl_entry), Err(EntryValidationError::InvalidField(_)));
    }

    // Tests `event_stream_from_view` with various "shapes". The test
    // parameter is a vec of ranges, where each range corresponds to the batch
    // of events that will be sent in response to a single call to `GetNext().
    #[test_case(Vec::new(); "no events")]
    #[test_case(vec![0..1]; "single_batch_single_event")]
    #[test_case(vec![0..10]; "single_batch_many_events")]
    #[test_case(vec![0..10, 10..20, 20..30]; "many_batches_many_events")]
    #[fuchsia::test]
    async fn event_stream_from_view_against_shape(test_shape: Vec<std::ops::Range<u8>>) {
        // Build the event stream based on the `test_shape`. Use a channel
        // so that the stream stays open until `close_channel` is called later.
        let (batches_sender, batches_receiver) =
            futures::channel::mpsc::unbounded::<Vec<fnet_neighbor::EntryIteratorItem>>();
        for batch_shape in &test_shape {
            batches_sender
                .unbounded_send(testutil::generate_events_in_range(batch_shape.clone()))
                .expect("failed to send event batch");
        }

        let (view, entry_iter_fut) = testutil::create_fake_view(batches_receiver);

        let event_stream =
            event_stream_from_view(&view).expect("failed to open entry iterator").fuse();

        futures::pin_mut!(entry_iter_fut, event_stream);

        for batch_shape in test_shape {
            for event_idx in batch_shape.into_iter() {
                futures::select! {
                    () = entry_iter_fut => panic!(
                        "fake entry iterator implementation unexpectedly finished"
                    ),
                    event = event_stream.next() => {
                        let actual_event = event
                            .expect("event stream unexpectedly empty")
                            .expect("error processing event");
                        let expected_event = testutil::generate_event(event_idx)
                                .try_into()
                                .expect("test event is unexpectedly invalid");
                        assert_eq!(actual_event, expected_event);
                    }
                };
            }
        }

        // Close `batches_sender` and observe that the `event_stream` ends.
        batches_sender.close_channel();
        let ((), mut events) = futures::join!(entry_iter_fut, event_stream.collect::<Vec<_>>());
        assert_matches!(
            events.pop(),
            Some(Err(EntryIteratorError::Fidl(fidl::Error::ClientChannelClosed {
                epitaph: fidl::Epitaph::PeerClosed,
                ..
            })))
        );
        assert_matches!(events[..], []);
    }

    // Verify that calling `event_stream_from_view` multiple times with the
    // same `View` proxy, results in independent `EntryIterator` clients.
    #[fuchsia::test]
    async fn event_stream_from_view_multiple_iterators() {
        // Events for 3 iterators. Each receives one batch containing 10 events.
        let test_data = vec![
            vec![testutil::generate_events_in_range(0..10)],
            vec![testutil::generate_events_in_range(10..20)],
            vec![testutil::generate_events_in_range(20..30)],
        ];

        // Instantiate the fake EntryIterator implementations.
        let (view, view_server_end) = fidl::endpoints::create_proxy::<fnet_neighbor::ViewMarker>();
        let view_request_stream = view_server_end.into_stream();
        let entry_iters_fut = view_request_stream
            .zip(futures::stream::iter(test_data.clone()))
            .for_each_concurrent(usize::MAX, |(request, event_data)| {
                testutil::serve_view_request(
                    request.expect("failed to receive `OpenEntryIterator` request"),
                    futures::stream::iter(event_data),
                )
            });

        let validate_event_streams_fut =
            futures::future::join_all(test_data.into_iter().map(|event_data| {
                let events_fut = event_stream_from_view(&view)
                    .expect("failed to create entry iterator")
                    .collect::<std::collections::VecDeque<_>>();
                events_fut.then(|mut events| {
                    for expected_event in event_data.into_iter().flatten() {
                        assert_eq!(
                            events
                                .pop_front()
                                .expect("event_stream unexpectedly empty")
                                .expect("error processing event"),
                            expected_event.try_into().expect("test event is unexpectedly invalid"),
                        );
                    }
                    assert_matches!(
                        events.pop_front(),
                        Some(Err(EntryIteratorError::Fidl(fidl::Error::ClientChannelClosed {
                            epitaph: fidl::Epitaph::PeerClosed,
                            ..
                        })))
                    );
                    assert_matches!(events.make_contiguous(), []);
                    futures::future::ready(())
                })
            }));

        futures::join!(entry_iters_fut, validate_event_streams_fut);
    }

    // Verify that failing to convert an event results in an error and closes
    // the event stream. `trailing_event` and `trailing_batch` control whether
    // a good event is sent after the bad event, either as part of the same
    // batch or in a subsequent batch. The test expects this data to be
    // truncated from the resulting event_stream.
    #[test_case(false, false; "no_trailing")]
    #[test_case(true, false; "trailing_event")]
    #[test_case(false, true; "trailing_batch")]
    #[test_case(true, true; "trailing_event_and_batch")]
    #[fuchsia::test]
    async fn event_stream_from_view_conversion_error(trailing_event: bool, trailing_batch: bool) {
        let bad_event = fnet_neighbor::EntryIteratorItem::Added(fnet_neighbor::Entry {
            interface: None, // Required field omitted.
            neighbor: Some(fnet::IpAddress::Ipv4(fnet::Ipv4Address { addr: [192, 168, 0, 1] })),
            state: Some(fnet_neighbor::EntryState::Reachable),
            mac: Some(fnet::MacAddress { octets: [0, 1, 2, 3, 4, 5] }),
            updated_at: Some(123456),
            ..Default::default()
        });

        let batch = std::iter::once(bad_event)
            // Optionally append a known good event to the batch.
            .chain(trailing_event.then(|| testutil::generate_event(0)).into_iter())
            .collect::<Vec<_>>();
        let batches = std::iter::once(batch)
            // Optionally append a known good batch to the sequence of batches.
            .chain(trailing_batch.then(|| vec![testutil::generate_event(1)]))
            .collect::<Vec<_>>();

        // Instantiate the fake entry iterator implementation.
        let (view, entry_iter_fut) = testutil::create_fake_view(futures::stream::iter(batches));

        let event_stream = event_stream_from_view(&view).expect("failed to connect to view").fuse();

        futures::pin_mut!(entry_iter_fut, event_stream);
        let ((), events) = futures::join!(entry_iter_fut, event_stream.collect::<Vec<_>>());
        assert_matches!(&events[..], &[Err(EntryIteratorError::Conversion(_))]);
    }

    // Verify that iterator returning an empty batch results in an error and
    // closes the event stream. When `trailing_batch` is true, an additional
    // "good" batch will be sent after the empty batch; the test expects this
    // data to be truncated from the resulting event_stream.
    #[test_case(false; "no_trailing_batch")]
    #[test_case(true; "trailing_batch")]
    #[fuchsia::test]
    async fn event_stream_from_view_empty_batch_error(trailing_batch: bool) {
        let batches = std::iter::once(Vec::new())
            // Optionally append a known good batch to the sequence of batches.
            .chain(trailing_batch.then(|| vec![testutil::generate_event(0)]))
            .collect::<Vec<_>>();

        // Instantiate the fake EntryIterator implementation.
        let (view, entry_iter_fut) = testutil::create_fake_view(futures::stream::iter(batches));

        let event_stream =
            event_stream_from_view(&view).expect("failed to create entry iterator").fuse();

        futures::pin_mut!(entry_iter_fut, event_stream);
        let ((), events) = futures::join!(entry_iter_fut, event_stream.collect::<Vec<_>>());
        assert_matches!(&events[..], &[Err(EntryIteratorError::EmptyEventBatch)]);
    }

    #[fuchsia::test]
    async fn collect_neighbors_until_idle_error_error_in_stream() {
        let event = Err(EntryIteratorError::EmptyEventBatch);
        let event_stream = futures::stream::once(futures::future::ready(event));
        assert_matches!(
            collect_neighbors_until_idle::<Vec<_>>(event_stream).await,
            Err(CollectNeighborsUntilIdleError::ErrorInStream(_))
        );
    }

    #[fuchsia::test]
    async fn collect_neighbors_until_idle_error_unexpected_event() {
        let event =
            Ok(Event::Added(valid_fidl_entry(NonZeroU64::new(1).unwrap()).try_into().unwrap()));
        let event_stream = futures::stream::once(futures::future::ready(event));
        assert_matches!(
            collect_neighbors_until_idle::<Vec<_>>(event_stream).await,
            Err(CollectNeighborsUntilIdleError::UnexpectedEvent(_))
        );
    }

    #[fuchsia::test]
    async fn collect_neighbors_until_idle_error_stream_ended() {
        let event =
            Ok(Event::Existing(valid_fidl_entry(NonZeroU64::new(1).unwrap()).try_into().unwrap()));
        let event_stream = futures::stream::once(futures::future::ready(event));
        assert_matches!(
            collect_neighbors_until_idle::<Vec<_>>(event_stream).await,
            Err(CollectNeighborsUntilIdleError::StreamEnded)
        );
    }

    #[fuchsia::test]
    async fn collect_neighbors_until_idle_success() {
        let entry: Entry = valid_fidl_entry(NonZeroU64::new(1).unwrap()).try_into().unwrap();
        let mut event_stream = futures::stream::iter([
            Ok(Event::Existing(entry.clone())),
            Ok(Event::Idle),
            Ok(Event::Added(entry.clone())),
        ]);

        let existing: Vec<_> = collect_neighbors_until_idle(&mut event_stream)
            .await
            .expect("failed to collect existing neighbors");
        assert_eq!(existing, &[entry.clone()]);

        let trailing_events: Vec<_> = event_stream.collect().await;
        assert_matches!(
            &trailing_events[..],
            [Ok(Event::Added(found_entry))] if *found_entry == entry
        );
    }
}
