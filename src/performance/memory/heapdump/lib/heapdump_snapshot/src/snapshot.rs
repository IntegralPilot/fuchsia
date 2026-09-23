// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::MonotonicInstant;
use flex_fuchsia_memory_heapdump_client as fheapdump_client;
use futures::StreamExt;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::Error;

/// Contains a snapshot along with metadata from its header.
#[derive(Debug)]
pub struct SnapshotWithHeader {
    pub process_name: String,
    pub process_koid: u64,
    pub snapshot: Snapshot,
}

/// Contains all the data received over a `SnapshotReceiver` channel.
#[derive(Debug)]
pub struct Snapshot {
    /// All the live allocations in the analyzed process, indexed by memory address.
    pub allocations: Vec<Allocation>,

    /// All the executable memory regions in the analyzed process, indexed by start address.
    pub executable_regions: HashMap<u64, ExecutableRegion>,
}

/// Information about one or more allocated memory blocks.
#[derive(Debug)]
pub struct Allocation {
    pub address: Option<u64>,

    /// Number of allocations that have been aggregated into this `Allocation` instance.
    pub count: u64,

    /// Total size, in bytes.
    pub size: u64,

    /// Allocating thread.
    pub thread_info: Option<Rc<ThreadInfo>>,

    /// Stack trace of the allocation site.
    pub stack_trace: Rc<StackTrace>,

    /// Allocation timestamp, in nanoseconds.
    pub timestamp: Option<MonotonicInstant>,

    /// Memory dump of this block's contents.
    pub contents: Option<Vec<u8>>,
}

/// A stack trace.
#[derive(Debug)]
pub struct StackTrace {
    /// Code addresses at each call frame. The first entry corresponds to the leaf call.
    pub program_addresses: Vec<u64>,
}

/// A memory region containing code loaded from an ELF file.
#[derive(Debug)]
pub struct ExecutableRegion {
    /// Region name for human consumption (usually either the ELF soname or the VMO name), if known.
    pub name: String,

    /// Region size, in bytes.
    pub size: u64,

    /// The corresponding offset in the ELF file.
    pub file_offset: u64,

    /// The corresponding relative address in the ELF file.
    pub vaddr: u64,

    /// The Build ID of the ELF file.
    pub build_id: Vec<u8>,
}

/// Information identifying a specific thread.
#[derive(Debug, PartialEq)]
pub struct ThreadInfo {
    /// The thread's koid.
    pub koid: zx_types::zx_koid_t,

    /// The thread's name.
    pub name: String,
}

/// Gets the value of a field in a FIDL table as a `Result<T, Error>`.
///
/// An `Err(Error::MissingField { .. })` is returned if the field's value is `None`.
///
/// Usage: `read_field!(container_expression => ContainerType, field_name)`
///
/// # Example
///
/// ```
/// struct MyFidlTable { field: Option<u32>, .. }
/// let table = MyFidlTable { field: Some(44), .. };
///
/// let val = read_field!(table => MyFidlTable, field)?;
/// ```
macro_rules! read_field {
    ($e:expr => $c:ident, $f:ident) => {
        $e.$f.ok_or(Error::MissingField {
            container: std::stringify!($c),
            field: std::stringify!($f),
        })
    };
}

impl Snapshot {
    /// Receives a snapshot over a `SnapshotReceiver` channel and reassembles it.
    pub async fn receive_single_from(
        mut stream: fheapdump_client::SnapshotReceiverRequestStream,
    ) -> Result<Snapshot, Error> {
        Snapshot::receive_inner(&mut stream).await
    }

    /// Receives multiple header-prefixed snapshots over a `SnapshotReceiver` channel and
    /// reassemble them.
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    pub async fn receive_multi_from(
        mut stream: fheapdump_client::SnapshotReceiverRequestStream,
    ) -> Result<Vec<SnapshotWithHeader>, Error> {
        let mut snapshots = vec![];
        loop {
            // Wait for a batch of elements containing either just a header element or an empty
            // batch (to signal the end of the stream).
            match stream.next().await.transpose()? {
                Some(fheapdump_client::SnapshotReceiverRequest::Batch { batch, responder }) => {
                    match &batch[..] {
                        [fheapdump_client::SnapshotElement::SnapshotHeader(header)] => {
                            responder.send()?;

                            // Receive the actual snapshot.
                            let snapshot = Snapshot::receive_inner(&mut stream).await?;

                            let header = header.clone();
                            snapshots.push(SnapshotWithHeader {
                                process_name: read_field!(header => SnapshotHeader, process_name)?,
                                process_koid: read_field!(header => SnapshotHeader, process_koid)?,
                                snapshot,
                            });
                        }
                        [] => {
                            responder.send()?;
                            return Ok(snapshots);
                        }
                        _ => return Err(Error::HeaderExpected),
                    }
                }
                Some(fheapdump_client::SnapshotReceiverRequest::ReportError {
                    error,
                    responder,
                }) => {
                    let _ = responder.send(); // Ignore the result of the acknowledgment.
                    return Err(Error::CollectorError(error));
                }
                None => return Err(Error::UnexpectedEndOfStream),
            };
        }
    }

    async fn receive_inner(
        stream: &mut fheapdump_client::SnapshotReceiverRequestStream,
    ) -> Result<Snapshot, Error> {
        struct AllocationValue {
            address: Option<u64>,
            count: u64,
            size: u64,
            thread_info_key: Option<u64>,
            stack_trace_key: u64,
            timestamp: Option<MonotonicInstant>,
        }
        let mut allocation_addresses: HashSet<u64> = HashSet::new();
        let mut allocations: Vec<AllocationValue> = vec![];
        let mut thread_infos: HashMap<u64, Rc<ThreadInfo>> = HashMap::new();
        let mut stack_traces: HashMap<u64, Vec<u64>> = HashMap::new();
        let mut executable_regions: HashMap<u64, ExecutableRegion> = HashMap::new();
        let mut contents: HashMap<u64, Vec<u8>> = HashMap::new();

        loop {
            // Wait for the next batch of elements.
            let batch = match stream.next().await.transpose()? {
                Some(fheapdump_client::SnapshotReceiverRequest::Batch { batch, responder }) => {
                    // Send acknowledgment as quickly as possible, then keep processing the received batch.
                    responder.send()?;
                    batch
                }
                Some(fheapdump_client::SnapshotReceiverRequest::ReportError {
                    error,
                    responder,
                }) => {
                    let _ = responder.send(); // Ignore the result of the acknowledgment.
                    return Err(Error::CollectorError(error));
                }
                None => return Err(Error::UnexpectedEndOfStream),
            };

            // Process data. An empty batch signals the end of the stream.
            if !batch.is_empty() {
                for element in batch {
                    match element {
                        fheapdump_client::SnapshotElement::Allocation(allocation) => {
                            if let Some(address) = allocation.address {
                                if !allocation_addresses.insert(address) {
                                    return Err(Error::ConflictingElement {
                                        element_type: "Allocation",
                                    });
                                }
                            }

                            #[cfg(not(fuchsia_api_level_at_least = "29"))]
                            let count = 1;
                            #[cfg(fuchsia_api_level_at_least = "29")]
                            let count = allocation.count.unwrap_or(1);

                            let size = read_field!(allocation => Allocation, size)?;
                            let stack_trace_key =
                                read_field!(allocation => Allocation, stack_trace_key)?;
                            allocations.push(AllocationValue {
                                address: allocation.address,
                                count,
                                size,
                                thread_info_key: allocation.thread_info_key,
                                stack_trace_key,
                                timestamp: allocation.timestamp,
                            });
                        }
                        fheapdump_client::SnapshotElement::StackTrace(stack_trace) => {
                            let stack_trace_key =
                                read_field!(stack_trace => StackTrace, stack_trace_key)?;
                            let mut program_addresses =
                                read_field!(stack_trace => StackTrace, program_addresses)?;
                            stack_traces
                                .entry(stack_trace_key)
                                .or_default()
                                .append(&mut program_addresses);
                        }
                        fheapdump_client::SnapshotElement::ThreadInfo(thread_info) => {
                            let thread_info_key =
                                read_field!(thread_info => ThreadInfo, thread_info_key)?;
                            let koid = read_field!(thread_info => ThreadInfo, koid)?;
                            let name = read_field!(thread_info => ThreadInfo, name)?;
                            if thread_infos
                                .insert(thread_info_key, Rc::new(ThreadInfo { koid, name }))
                                .is_some()
                            {
                                return Err(Error::ConflictingElement {
                                    element_type: "ThreadInfo",
                                });
                            }
                        }
                        fheapdump_client::SnapshotElement::ExecutableRegion(region) => {
                            let address = read_field!(region => ExecutableRegion, address)?;
                            let name = region.name.unwrap_or_else(|| String::new());
                            let size = read_field!(region => ExecutableRegion, size)?;
                            let file_offset = read_field!(region => ExecutableRegion, file_offset)?;
                            let vaddr = read_field!(region => ExecutableRegion, vaddr)?;
                            let build_id = read_field!(region => ExecutableRegion, build_id)?.value;
                            let region =
                                ExecutableRegion { name, size, file_offset, vaddr, build_id };
                            if executable_regions.insert(address, region).is_some() {
                                return Err(Error::ConflictingElement {
                                    element_type: "ExecutableRegion",
                                });
                            }
                        }
                        fheapdump_client::SnapshotElement::BlockContents(block_contents) => {
                            let address = read_field!(block_contents => BlockContents, address)?;
                            let mut chunk = read_field!(block_contents => BlockContents, contents)?;
                            contents.entry(address).or_default().append(&mut chunk);
                        }
                        _ => return Err(Error::UnexpectedElementType),
                    }
                }
            } else {
                // We are at the end of the stream. Convert to the final types and resolve
                // cross-references.
                let final_stack_traces: HashMap<u64, Rc<StackTrace>> = stack_traces
                    .into_iter()
                    .map(|(key, program_addresses)| {
                        (key, Rc::new(StackTrace { program_addresses }))
                    })
                    .collect();
                let mut final_allocations = vec![];
                for AllocationValue {
                    address,
                    count,
                    size,
                    thread_info_key,
                    stack_trace_key,
                    timestamp,
                } in allocations
                {
                    let thread_info = match thread_info_key {
                        Some(key) => Some(
                            thread_infos
                                .get(&key)
                                .ok_or(Error::InvalidCrossReference { element_type: "ThreadInfo" })?
                                .clone(),
                        ),
                        None => None,
                    };
                    let stack_trace = final_stack_traces
                        .get(&stack_trace_key)
                        .ok_or(Error::InvalidCrossReference { element_type: "StackTrace" })?
                        .clone();
                    let contents = address.and_then(|address| contents.remove(&address));
                    if let Some(data) = &contents {
                        if data.len() as u64 != size {
                            return Err(Error::ConflictingElement {
                                element_type: "BlockContents",
                            });
                        }
                    }
                    final_allocations.push(Allocation {
                        address,
                        count,
                        size,
                        thread_info,
                        stack_trace,
                        timestamp,
                        contents,
                    });
                }

                return Ok(Snapshot { allocations: final_allocations, executable_regions });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::create_client;
    use assert_matches::assert_matches;
    use fuchsia_async as fasync;
    use test_case::test_case;

    // Constants used by some of the tests below:
    const FAKE_ALLOCATION_1_ADDRESS: u64 = 1234;
    const FAKE_ALLOCATION_1_SIZE: u64 = 8;
    const FAKE_ALLOCATION_1_TIMESTAMP: MonotonicInstant = MonotonicInstant::from_nanos(888888888);
    const FAKE_ALLOCATION_1_CONTENTS: [u8; FAKE_ALLOCATION_1_SIZE as usize] = *b"12345678";
    const FAKE_ALLOCATION_2_ADDRESS: u64 = 5678;
    const FAKE_ALLOCATION_2_SIZE: u64 = 4;
    const FAKE_ALLOCATION_2_TIMESTAMP: MonotonicInstant = MonotonicInstant::from_nanos(-777777777); // test negative value too
    const FAKE_THREAD_1_KOID: u64 = 1212;
    const FAKE_THREAD_1_NAME: &str = "fake-thread-1-name";
    const FAKE_THREAD_1_KEY: u64 = 4567;
    const FAKE_THREAD_2_KOID: u64 = 1213;
    const FAKE_THREAD_2_NAME: &str = "fake-thread-2-name";
    const FAKE_THREAD_2_KEY: u64 = 7654;
    const FAKE_STACK_TRACE_1_ADDRESSES: [u64; 6] = [11111, 22222, 33333, 22222, 44444, 55555];
    const FAKE_STACK_TRACE_1_KEY: u64 = 9876;
    const FAKE_STACK_TRACE_2_ADDRESSES: [u64; 4] = [11111, 22222, 11111, 66666];
    const FAKE_STACK_TRACE_2_KEY: u64 = 6789;
    const FAKE_REGION_1_ADDRESS: u64 = 0x10000000;
    const FAKE_REGION_1_NAME: &str = "region-1";
    const FAKE_REGION_1_SIZE: u64 = 0x80000;
    const FAKE_REGION_1_FILE_OFFSET: u64 = 0x1000;
    const FAKE_REGION_1_VADDR: u64 = 0x3000;
    const FAKE_REGION_1_BUILD_ID: &[u8] = &[0xaa; 20];
    const FAKE_REGION_2_ADDRESS: u64 = 0x7654300000;
    const FAKE_REGION_2_SIZE: u64 = 0x200000;
    const FAKE_REGION_2_FILE_OFFSET: u64 = 0x2000;
    const FAKE_REGION_2_BUILD_ID: &[u8] = &[0x55; 32];
    const FAKE_REGION_2_VADDR: u64 = 0x7000;

    #[fuchsia::test]
    async fn test_empty() {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fheapdump_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_single_from(receiver_stream));

        // Send the end of stream marker.
        let fut = receiver_proxy.batch(&[]);
        fut.await.unwrap();

        // Receive the snapshot we just transmitted and verify that it is empty.
        let received_snapshot = receive_worker.await.unwrap();
        assert!(received_snapshot.allocations.is_empty());
        assert!(received_snapshot.executable_regions.is_empty());
    }

    #[fuchsia::test]
    async fn test_one_batch() {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fheapdump_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_single_from(receiver_stream));

        // Send a batch containing two allocations - whose threads, stack traces and contents can be
        // listed before or after the allocation(s) that reference them - and two executable
        // regions.
        let fut = receiver_proxy.batch(&[
            fheapdump_client::SnapshotElement::BlockContents(fheapdump_client::BlockContents {
                address: Some(FAKE_ALLOCATION_1_ADDRESS),
                contents: Some(FAKE_ALLOCATION_1_CONTENTS.to_vec()),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::ExecutableRegion(
                fheapdump_client::ExecutableRegion {
                    address: Some(FAKE_REGION_1_ADDRESS),
                    name: Some(FAKE_REGION_1_NAME.to_string()),
                    size: Some(FAKE_REGION_1_SIZE),
                    file_offset: Some(FAKE_REGION_1_FILE_OFFSET),
                    vaddr: Some(FAKE_REGION_1_VADDR),
                    build_id: Some(fheapdump_client::BuildId {
                        value: FAKE_REGION_1_BUILD_ID.to_vec(),
                    }),
                    ..Default::default()
                },
            ),
            fheapdump_client::SnapshotElement::StackTrace(fheapdump_client::StackTrace {
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                program_addresses: Some(FAKE_STACK_TRACE_1_ADDRESSES.to_vec()),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::Allocation(fheapdump_client::Allocation {
                address: Some(FAKE_ALLOCATION_1_ADDRESS),
                size: Some(FAKE_ALLOCATION_1_SIZE),
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                stack_trace_key: Some(FAKE_STACK_TRACE_2_KEY),
                timestamp: Some(FAKE_ALLOCATION_1_TIMESTAMP),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::ThreadInfo(fheapdump_client::ThreadInfo {
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                koid: Some(FAKE_THREAD_1_KOID),
                name: Some(FAKE_THREAD_1_NAME.to_string()),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::ExecutableRegion(
                fheapdump_client::ExecutableRegion {
                    address: Some(FAKE_REGION_2_ADDRESS),
                    size: Some(FAKE_REGION_2_SIZE),
                    file_offset: Some(FAKE_REGION_2_FILE_OFFSET),
                    build_id: Some(fheapdump_client::BuildId {
                        value: FAKE_REGION_2_BUILD_ID.to_vec(),
                    }),
                    vaddr: Some(FAKE_REGION_2_VADDR),
                    ..Default::default()
                },
            ),
            fheapdump_client::SnapshotElement::ThreadInfo(fheapdump_client::ThreadInfo {
                thread_info_key: Some(FAKE_THREAD_2_KEY),
                koid: Some(FAKE_THREAD_2_KOID),
                name: Some(FAKE_THREAD_2_NAME.to_string()),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::Allocation(fheapdump_client::Allocation {
                address: Some(FAKE_ALLOCATION_2_ADDRESS),
                size: Some(FAKE_ALLOCATION_2_SIZE),
                thread_info_key: Some(FAKE_THREAD_2_KEY),
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                timestamp: Some(FAKE_ALLOCATION_2_TIMESTAMP),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::StackTrace(fheapdump_client::StackTrace {
                stack_trace_key: Some(FAKE_STACK_TRACE_2_KEY),
                program_addresses: Some(FAKE_STACK_TRACE_2_ADDRESSES.to_vec()),
                ..Default::default()
            }),
        ]);
        fut.await.unwrap();

        // Send the end of stream marker.
        let fut = receiver_proxy.batch(&[]);
        fut.await.unwrap();

        // Receive the snapshot we just transmitted and verify its contents.
        let mut received_snapshot = receive_worker.await.unwrap();
        let allocation1 = received_snapshot.allocations.swap_remove(
            received_snapshot
                .allocations
                .iter()
                .position(|alloc| alloc.address == Some(FAKE_ALLOCATION_1_ADDRESS))
                .unwrap(),
        );
        assert_eq!(allocation1.size, FAKE_ALLOCATION_1_SIZE);
        assert_eq!(
            allocation1.thread_info,
            Some(Rc::new(ThreadInfo {
                koid: FAKE_THREAD_1_KOID,
                name: FAKE_THREAD_1_NAME.to_owned()
            }))
        );
        assert_eq!(allocation1.stack_trace.program_addresses, FAKE_STACK_TRACE_2_ADDRESSES);
        assert_eq!(allocation1.timestamp, Some(FAKE_ALLOCATION_1_TIMESTAMP));
        assert_eq!(
            allocation1.contents.as_ref().expect("contents must be set"),
            &FAKE_ALLOCATION_1_CONTENTS.to_vec()
        );
        let allocation2 = received_snapshot.allocations.swap_remove(
            received_snapshot
                .allocations
                .iter()
                .position(|alloc| alloc.address == Some(FAKE_ALLOCATION_2_ADDRESS))
                .unwrap(),
        );
        assert_eq!(allocation2.size, FAKE_ALLOCATION_2_SIZE);
        assert_eq!(
            allocation2.thread_info,
            Some(Rc::new(ThreadInfo {
                koid: FAKE_THREAD_2_KOID,
                name: FAKE_THREAD_2_NAME.to_owned()
            }))
        );
        assert_eq!(allocation2.stack_trace.program_addresses, FAKE_STACK_TRACE_1_ADDRESSES);
        assert_eq!(allocation2.timestamp, Some(FAKE_ALLOCATION_2_TIMESTAMP));
        assert_matches!(allocation2.contents, None, "no contents are sent for this allocation");
        assert!(received_snapshot.allocations.is_empty(), "all the entries have been removed");
        let region1 = received_snapshot.executable_regions.remove(&FAKE_REGION_1_ADDRESS).unwrap();
        assert_eq!(region1.name, FAKE_REGION_1_NAME);
        assert_eq!(region1.size, FAKE_REGION_1_SIZE);
        assert_eq!(region1.file_offset, FAKE_REGION_1_FILE_OFFSET);
        assert_eq!(region1.vaddr, FAKE_REGION_1_VADDR);
        assert_eq!(region1.build_id, FAKE_REGION_1_BUILD_ID);
        let region2 = received_snapshot.executable_regions.remove(&FAKE_REGION_2_ADDRESS).unwrap();
        assert_eq!(region2.size, FAKE_REGION_2_SIZE);
        assert_eq!(region2.file_offset, FAKE_REGION_2_FILE_OFFSET);
        assert_eq!(region2.build_id, FAKE_REGION_2_BUILD_ID);
        assert!(received_snapshot.executable_regions.is_empty(), "all entries have been removed");
    }

    #[fuchsia::test]
    async fn test_two_batches() {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fheapdump_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_single_from(receiver_stream));

        // Send a first batch.
        let fut = receiver_proxy.batch(&[
            fheapdump_client::SnapshotElement::ExecutableRegion(
                fheapdump_client::ExecutableRegion {
                    address: Some(FAKE_REGION_2_ADDRESS),
                    size: Some(FAKE_REGION_2_SIZE),
                    file_offset: Some(FAKE_REGION_2_FILE_OFFSET),
                    build_id: Some(fheapdump_client::BuildId {
                        value: FAKE_REGION_2_BUILD_ID.to_vec(),
                    }),
                    vaddr: Some(FAKE_REGION_2_VADDR),
                    ..Default::default()
                },
            ),
            fheapdump_client::SnapshotElement::Allocation(fheapdump_client::Allocation {
                address: Some(FAKE_ALLOCATION_1_ADDRESS),
                size: Some(FAKE_ALLOCATION_1_SIZE),
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                stack_trace_key: Some(FAKE_STACK_TRACE_2_KEY),
                timestamp: Some(FAKE_ALLOCATION_1_TIMESTAMP),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::StackTrace(fheapdump_client::StackTrace {
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                program_addresses: Some(FAKE_STACK_TRACE_1_ADDRESSES.to_vec()),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::ThreadInfo(fheapdump_client::ThreadInfo {
                thread_info_key: Some(FAKE_THREAD_2_KEY),
                koid: Some(FAKE_THREAD_2_KOID),
                name: Some(FAKE_THREAD_2_NAME.to_string()),
                ..Default::default()
            }),
        ]);
        fut.await.unwrap();

        // Send another batch.
        let fut = receiver_proxy.batch(&[
            fheapdump_client::SnapshotElement::ThreadInfo(fheapdump_client::ThreadInfo {
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                koid: Some(FAKE_THREAD_1_KOID),
                name: Some(FAKE_THREAD_1_NAME.to_string()),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::Allocation(fheapdump_client::Allocation {
                address: Some(FAKE_ALLOCATION_2_ADDRESS),
                size: Some(FAKE_ALLOCATION_2_SIZE),
                thread_info_key: Some(FAKE_THREAD_2_KEY),
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                timestamp: Some(FAKE_ALLOCATION_2_TIMESTAMP),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::ExecutableRegion(
                fheapdump_client::ExecutableRegion {
                    address: Some(FAKE_REGION_1_ADDRESS),
                    name: Some(FAKE_REGION_1_NAME.to_string()),
                    size: Some(FAKE_REGION_1_SIZE),
                    file_offset: Some(FAKE_REGION_1_FILE_OFFSET),
                    vaddr: Some(FAKE_REGION_1_VADDR),
                    build_id: Some(fheapdump_client::BuildId {
                        value: FAKE_REGION_1_BUILD_ID.to_vec(),
                    }),
                    ..Default::default()
                },
            ),
            fheapdump_client::SnapshotElement::StackTrace(fheapdump_client::StackTrace {
                stack_trace_key: Some(FAKE_STACK_TRACE_2_KEY),
                program_addresses: Some(FAKE_STACK_TRACE_2_ADDRESSES.to_vec()),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::BlockContents(fheapdump_client::BlockContents {
                address: Some(FAKE_ALLOCATION_1_ADDRESS),
                contents: Some(FAKE_ALLOCATION_1_CONTENTS.to_vec()),
                ..Default::default()
            }),
        ]);
        fut.await.unwrap();

        // Send the end of stream marker.
        let fut = receiver_proxy.batch(&[]);
        fut.await.unwrap();

        // Receive the snapshot we just transmitted and verify its contents.
        let mut received_snapshot = receive_worker.await.unwrap();
        let allocation1 = received_snapshot.allocations.swap_remove(
            received_snapshot
                .allocations
                .iter()
                .position(|alloc| alloc.address == Some(FAKE_ALLOCATION_1_ADDRESS))
                .unwrap(),
        );
        assert_eq!(allocation1.size, FAKE_ALLOCATION_1_SIZE);
        assert_eq!(
            allocation1.thread_info,
            Some(Rc::new(ThreadInfo {
                koid: FAKE_THREAD_1_KOID,
                name: FAKE_THREAD_1_NAME.to_owned()
            }))
        );
        assert_eq!(allocation1.stack_trace.program_addresses, FAKE_STACK_TRACE_2_ADDRESSES);
        assert_eq!(allocation1.timestamp, Some(FAKE_ALLOCATION_1_TIMESTAMP));
        assert_eq!(
            allocation1.contents.as_ref().expect("contents must be set"),
            &FAKE_ALLOCATION_1_CONTENTS.to_vec()
        );
        let allocation2 = received_snapshot.allocations.swap_remove(
            received_snapshot
                .allocations
                .iter()
                .position(|alloc| alloc.address == Some(FAKE_ALLOCATION_2_ADDRESS))
                .unwrap(),
        );
        assert_eq!(allocation2.size, FAKE_ALLOCATION_2_SIZE);
        assert_eq!(
            allocation2.thread_info,
            Some(Rc::new(ThreadInfo {
                koid: FAKE_THREAD_2_KOID,
                name: FAKE_THREAD_2_NAME.to_owned()
            }))
        );
        assert_eq!(allocation2.stack_trace.program_addresses, FAKE_STACK_TRACE_1_ADDRESSES);
        assert_eq!(allocation2.timestamp, Some(FAKE_ALLOCATION_2_TIMESTAMP));
        assert_matches!(allocation2.contents, None, "no contents are sent for this allocation");
        assert!(received_snapshot.allocations.is_empty(), "all the entries have been removed");
        let region1 = received_snapshot.executable_regions.remove(&FAKE_REGION_1_ADDRESS).unwrap();
        assert_eq!(region1.name, FAKE_REGION_1_NAME);
        assert_eq!(region1.size, FAKE_REGION_1_SIZE);
        assert_eq!(region1.file_offset, FAKE_REGION_1_FILE_OFFSET);
        assert_eq!(region1.vaddr, FAKE_REGION_1_VADDR);
        assert_eq!(region1.build_id, FAKE_REGION_1_BUILD_ID);
        let region2 = received_snapshot.executable_regions.remove(&FAKE_REGION_2_ADDRESS).unwrap();
        assert_eq!(region2.size, FAKE_REGION_2_SIZE);
        assert_eq!(region2.file_offset, FAKE_REGION_2_FILE_OFFSET);
        assert_eq!(region2.build_id, FAKE_REGION_2_BUILD_ID);
        assert!(received_snapshot.executable_regions.is_empty(), "all entries have been removed");
    }

    #[test_case(|allocation| allocation.size = None => matches
        Err(Error::MissingField { container: "Allocation", field: "size" }) ; "size")]
    #[test_case(|allocation| allocation.stack_trace_key = None => matches
        Err(Error::MissingField { container: "Allocation", field: "stack_trace_key" }) ; "stack_trace_key")]
    #[test_case(|allocation| allocation.address = None => matches
        Ok(_) ; "address_is_optional")]
    #[test_case(|allocation| allocation.thread_info_key = None => matches
        Ok(_) ; "thread_info_is_optional")]
    #[test_case(|allocation| allocation.timestamp = None => matches
        Ok(_) ; "timestamp_is_optional")]
    #[test_case(|_| () /* if we do not set any field to None, the result should be Ok */ => matches
        Ok(_) ; "success")]
    #[fuchsia::test]
    async fn test_allocation_required_fields(
        set_one_field_to_none: fn(&mut fheapdump_client::Allocation),
    ) -> Result<Snapshot, Error> {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fheapdump_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_single_from(receiver_stream));

        // Start with an Allocation with all the required fields set.
        let mut allocation = fheapdump_client::Allocation {
            address: Some(FAKE_ALLOCATION_1_ADDRESS),
            size: Some(FAKE_ALLOCATION_1_SIZE),
            thread_info_key: Some(FAKE_THREAD_1_KEY),
            stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
            timestamp: Some(FAKE_ALLOCATION_1_TIMESTAMP),
            ..Default::default()
        };

        // Set one of the fields to None, according to the case being tested.
        set_one_field_to_none(&mut allocation);

        // Send it to the SnapshotReceiver along with the thread info and stack trace it references.
        let fut = receiver_proxy.batch(&[
            fheapdump_client::SnapshotElement::Allocation(allocation),
            fheapdump_client::SnapshotElement::ThreadInfo(fheapdump_client::ThreadInfo {
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                koid: Some(FAKE_THREAD_1_KOID),
                name: Some(FAKE_THREAD_1_NAME.to_string()),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::StackTrace(fheapdump_client::StackTrace {
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                program_addresses: Some(FAKE_STACK_TRACE_1_ADDRESSES.to_vec()),
                ..Default::default()
            }),
        ]);
        let _ = fut.await; // ignore result, as the peer may detect the error and close the channel

        // Send the end of stream marker.
        let fut = receiver_proxy.batch(&[]);
        let _ = fut.await; // ignore result, as the peer may detect the error and close the channel

        // Return the result.
        receive_worker.await
    }

    #[test_case(|thread_info| thread_info.thread_info_key = None => matches
        Err(Error::MissingField { container: "ThreadInfo", field: "thread_info_key" }) ; "thread_info_key")]
    #[test_case(|thread_info| thread_info.koid = None => matches
        Err(Error::MissingField { container: "ThreadInfo", field: "koid" }) ; "koid")]
    #[test_case(|thread_info| thread_info.name = None => matches
        Err(Error::MissingField { container: "ThreadInfo", field: "name" }) ; "name")]
    #[test_case(|_| () /* if we do not set any field to None, the result should be Ok */ => matches
        Ok(_) ; "success")]
    #[fuchsia::test]
    async fn test_thread_info_required_fields(
        set_one_field_to_none: fn(&mut fheapdump_client::ThreadInfo),
    ) -> Result<Snapshot, Error> {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fheapdump_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_single_from(receiver_stream));

        // Start with a ThreadInfo with all the required fields set.
        let mut thread_info = fheapdump_client::ThreadInfo {
            thread_info_key: Some(FAKE_THREAD_1_KEY),
            koid: Some(FAKE_THREAD_1_KOID),
            name: Some(FAKE_THREAD_1_NAME.to_string()),
            ..Default::default()
        };

        // Set one of the fields to None, according to the case being tested.
        set_one_field_to_none(&mut thread_info);

        // Send it to the SnapshotReceiver.
        let fut =
            receiver_proxy.batch(&[fheapdump_client::SnapshotElement::ThreadInfo(thread_info)]);
        let _ = fut.await; // ignore result, as the peer may detect the error and close the channel

        // Send the end of stream marker.
        let fut = receiver_proxy.batch(&[]);
        let _ = fut.await; // ignore result, as the peer may detect the error and close the channel

        // Return the result.
        receive_worker.await
    }

    #[test_case(|stack_trace| stack_trace.stack_trace_key = None => matches
        Err(Error::MissingField { container: "StackTrace", field: "stack_trace_key" }) ; "stack_trace_key")]
    #[test_case(|stack_trace| stack_trace.program_addresses = None => matches
        Err(Error::MissingField { container: "StackTrace", field: "program_addresses" }) ; "program_addresses")]
    #[test_case(|_| () /* if we do not set any field to None, the result should be Ok */ => matches
        Ok(_) ; "success")]
    #[fuchsia::test]
    async fn test_stack_trace_required_fields(
        set_one_field_to_none: fn(&mut fheapdump_client::StackTrace),
    ) -> Result<Snapshot, Error> {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fheapdump_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_single_from(receiver_stream));

        // Start with a StackTrace with all the required fields set.
        let mut stack_trace = fheapdump_client::StackTrace {
            stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
            program_addresses: Some(FAKE_STACK_TRACE_1_ADDRESSES.to_vec()),
            ..Default::default()
        };

        // Set one of the fields to None, according to the case being tested.
        set_one_field_to_none(&mut stack_trace);

        // Send it to the SnapshotReceiver.
        let fut =
            receiver_proxy.batch(&[fheapdump_client::SnapshotElement::StackTrace(stack_trace)]);
        let _ = fut.await; // ignore result, as the peer may detect the error and close the channel

        // Send the end of stream marker.
        let fut = receiver_proxy.batch(&[]);
        let _ = fut.await; // ignore result, as the peer may detect the error and close the channel

        // Return the result.
        receive_worker.await
    }

    #[test_case(|region| region.address = None => matches
        Err(Error::MissingField { container: "ExecutableRegion", field: "address" }) ; "address")]
    #[test_case(|region| region.size = None => matches
        Err(Error::MissingField { container: "ExecutableRegion", field: "size" }) ; "size")]
    #[test_case(|region| region.file_offset = None => matches
        Err(Error::MissingField { container: "ExecutableRegion", field: "file_offset" }) ; "file_offset")]
    #[test_case(|region| region.build_id = None => matches
        Err(Error::MissingField { container: "ExecutableRegion", field: "build_id" }) ; "build_id")]
    #[test_case(|region| region.vaddr = None => matches
        Err(Error::MissingField { container: "ExecutableRegion", field: "vaddr" }) ; "vaddr")]
    #[test_case(|_| () /* if we do not set any field to None, the result should be Ok */ => matches
        Ok(_) ; "success")]
    #[fuchsia::test]
    async fn test_executable_region_required_fields(
        set_one_field_to_none: fn(&mut fheapdump_client::ExecutableRegion),
    ) -> Result<Snapshot, Error> {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fheapdump_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_single_from(receiver_stream));

        // Start with an ExecutableRegion with all the required fields set.
        let mut region = fheapdump_client::ExecutableRegion {
            address: Some(FAKE_REGION_1_ADDRESS),
            size: Some(FAKE_REGION_1_SIZE),
            file_offset: Some(FAKE_REGION_1_FILE_OFFSET),
            build_id: Some(fheapdump_client::BuildId { value: FAKE_REGION_1_BUILD_ID.to_vec() }),
            vaddr: Some(FAKE_REGION_1_VADDR),
            ..Default::default()
        };

        // Set one of the fields to None, according to the case being tested.
        set_one_field_to_none(&mut region);

        // Send it to the SnapshotReceiver.
        let fut =
            receiver_proxy.batch(&[fheapdump_client::SnapshotElement::ExecutableRegion(region)]);
        let _ = fut.await; // ignore result, as the peer may detect the error and close the channel

        // Send the end of stream marker.
        let fut = receiver_proxy.batch(&[]);
        let _ = fut.await; // ignore result, as the peer may detect the error and close the channel

        // Return the result.
        receive_worker.await
    }

    #[test_case(|block_contents| block_contents.address = None => matches
        Err(Error::MissingField { container: "BlockContents", field: "address" }) ; "address")]
    #[test_case(|block_contents| block_contents.contents = None => matches
        Err(Error::MissingField { container: "BlockContents", field: "contents" }) ; "contents")]
    #[test_case(|_| () /* if we do not set any field to None, the result should be Ok */ => matches
        Ok(_) ; "success")]
    #[fuchsia::test]
    async fn test_block_contents_required_fields(
        set_one_field_to_none: fn(&mut fheapdump_client::BlockContents),
    ) -> Result<Snapshot, Error> {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fheapdump_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_single_from(receiver_stream));

        // Start with a BlockContents with all the required fields set.
        let mut block_contents = fheapdump_client::BlockContents {
            address: Some(FAKE_ALLOCATION_1_ADDRESS),
            contents: Some(FAKE_ALLOCATION_1_CONTENTS.to_vec()),
            ..Default::default()
        };

        // Set one of the fields to None, according to the case being tested.
        set_one_field_to_none(&mut block_contents);

        // Send it to the SnapshotReceiver along with the allocation it references.
        let fut = receiver_proxy.batch(&[
            fheapdump_client::SnapshotElement::BlockContents(block_contents),
            fheapdump_client::SnapshotElement::Allocation(fheapdump_client::Allocation {
                address: Some(FAKE_ALLOCATION_1_ADDRESS),
                size: Some(FAKE_ALLOCATION_1_SIZE),
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                timestamp: Some(FAKE_ALLOCATION_1_TIMESTAMP),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::ThreadInfo(fheapdump_client::ThreadInfo {
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                koid: Some(FAKE_THREAD_1_KOID),
                name: Some(FAKE_THREAD_1_NAME.to_string()),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::StackTrace(fheapdump_client::StackTrace {
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                program_addresses: Some(FAKE_STACK_TRACE_1_ADDRESSES.to_vec()),
                ..Default::default()
            }),
        ]);
        let _ = fut.await; // ignore result, as the peer may detect the error and close the channel

        // Send the end of stream marker.
        let fut = receiver_proxy.batch(&[]);
        let _ = fut.await; // ignore result, as the peer may detect the error and close the channel

        // Return the result.
        receive_worker.await
    }

    #[fuchsia::test]
    async fn test_conflicting_allocations() {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fheapdump_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_single_from(receiver_stream));

        // Send two allocations with the same address along with the stack trace they reference.
        let fut = receiver_proxy.batch(&[
            fheapdump_client::SnapshotElement::Allocation(fheapdump_client::Allocation {
                address: Some(FAKE_ALLOCATION_1_ADDRESS),
                size: Some(FAKE_ALLOCATION_1_SIZE),
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                timestamp: Some(FAKE_ALLOCATION_1_TIMESTAMP),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::Allocation(fheapdump_client::Allocation {
                address: Some(FAKE_ALLOCATION_1_ADDRESS),
                size: Some(FAKE_ALLOCATION_1_SIZE),
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                timestamp: Some(FAKE_ALLOCATION_1_TIMESTAMP),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::ThreadInfo(fheapdump_client::ThreadInfo {
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                koid: Some(FAKE_THREAD_1_KOID),
                name: Some(FAKE_THREAD_1_NAME.to_string()),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::StackTrace(fheapdump_client::StackTrace {
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                program_addresses: Some(FAKE_STACK_TRACE_1_ADDRESSES.to_vec()),
                ..Default::default()
            }),
        ]);
        let _ = fut.await; // ignore result, as the peer may detect the error and close the channel

        // Send the end of stream marker.
        let fut = receiver_proxy.batch(&[]);
        let _ = fut.await; // ignore result, as the peer may detect the error and close the channel

        // Verify expected error.
        assert_matches!(
            receive_worker.await,
            Err(Error::ConflictingElement { element_type: "Allocation" })
        );
    }

    #[fuchsia::test]
    async fn test_conflicting_executable_regions() {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fheapdump_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_single_from(receiver_stream));

        // Send two executable regions with the same address.
        let fut = receiver_proxy.batch(&[
            fheapdump_client::SnapshotElement::ExecutableRegion(
                fheapdump_client::ExecutableRegion {
                    address: Some(FAKE_REGION_1_ADDRESS),
                    size: Some(FAKE_REGION_1_SIZE),
                    file_offset: Some(FAKE_REGION_1_FILE_OFFSET),
                    build_id: Some(fheapdump_client::BuildId {
                        value: FAKE_REGION_1_BUILD_ID.to_vec(),
                    }),
                    vaddr: Some(FAKE_REGION_1_VADDR),
                    ..Default::default()
                },
            ),
            fheapdump_client::SnapshotElement::ExecutableRegion(
                fheapdump_client::ExecutableRegion {
                    address: Some(FAKE_REGION_1_ADDRESS),
                    size: Some(FAKE_REGION_1_SIZE),
                    file_offset: Some(FAKE_REGION_1_FILE_OFFSET),
                    build_id: Some(fheapdump_client::BuildId {
                        value: FAKE_REGION_1_BUILD_ID.to_vec(),
                    }),
                    vaddr: Some(FAKE_REGION_1_VADDR),
                    ..Default::default()
                },
            ),
        ]);
        let _ = fut.await; // ignore result, as the peer may detect the error and close the channel

        // Send the end of stream marker.
        let fut = receiver_proxy.batch(&[]);
        let _ = fut.await; // ignore result, as the peer may detect the error and close the channel

        // Verify expected error.
        assert_matches!(
            receive_worker.await,
            Err(Error::ConflictingElement { element_type: "ExecutableRegion" })
        );
    }

    #[fuchsia::test]
    async fn test_block_contents_wrong_size() {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fheapdump_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_single_from(receiver_stream));

        // Send an allocation whose BlockContents has the wrong size.
        let contents_with_wrong_size = vec![0; FAKE_ALLOCATION_1_SIZE as usize + 1];
        let fut = receiver_proxy.batch(&[
            fheapdump_client::SnapshotElement::Allocation(fheapdump_client::Allocation {
                address: Some(FAKE_ALLOCATION_1_ADDRESS),
                size: Some(FAKE_ALLOCATION_1_SIZE),
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                timestamp: Some(FAKE_ALLOCATION_1_TIMESTAMP),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::BlockContents(fheapdump_client::BlockContents {
                address: Some(FAKE_ALLOCATION_1_ADDRESS),
                contents: Some(contents_with_wrong_size),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::ThreadInfo(fheapdump_client::ThreadInfo {
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                koid: Some(FAKE_THREAD_1_KOID),
                name: Some(FAKE_THREAD_1_NAME.to_string()),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::StackTrace(fheapdump_client::StackTrace {
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                program_addresses: Some(FAKE_STACK_TRACE_1_ADDRESSES.to_vec()),
                ..Default::default()
            }),
        ]);
        let _ = fut.await; // ignore result, as the peer may detect the error and close the channel

        // Send the end of stream marker.
        let fut = receiver_proxy.batch(&[]);
        let _ = fut.await; // ignore result, as the peer may detect the error and close the channel

        // Verify expected error.
        assert_matches!(
            receive_worker.await,
            Err(Error::ConflictingElement { element_type: "BlockContents" })
        );
    }

    #[fuchsia::test]
    async fn test_empty_stack_trace() {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fheapdump_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_single_from(receiver_stream));

        // Send an allocation that references an empty stack trace.
        let fut = receiver_proxy.batch(&[
            fheapdump_client::SnapshotElement::Allocation(fheapdump_client::Allocation {
                address: Some(FAKE_ALLOCATION_1_ADDRESS),
                size: Some(FAKE_ALLOCATION_1_SIZE),
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                timestamp: Some(FAKE_ALLOCATION_1_TIMESTAMP),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::ThreadInfo(fheapdump_client::ThreadInfo {
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                koid: Some(FAKE_THREAD_1_KOID),
                name: Some(FAKE_THREAD_1_NAME.to_string()),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::StackTrace(fheapdump_client::StackTrace {
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                program_addresses: Some(vec![]),
                ..Default::default()
            }),
        ]);
        fut.await.unwrap();

        // Send the end of stream marker.
        let fut = receiver_proxy.batch(&[]);
        fut.await.unwrap();

        // Verify that the stack trace has been reconstructed correctly.
        let received_snapshot = receive_worker.await.unwrap();
        let allocation1 = received_snapshot
            .allocations
            .iter()
            .find(|a| a.address == Some(FAKE_ALLOCATION_1_ADDRESS))
            .unwrap();
        assert_eq!(allocation1.stack_trace.program_addresses, [0u64; 0]);
    }

    #[fuchsia::test]
    async fn test_chunked_stack_trace() {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fheapdump_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_single_from(receiver_stream));

        // Send an allocation and the first chunk of its stack trace.
        let fut = receiver_proxy.batch(&[
            fheapdump_client::SnapshotElement::Allocation(fheapdump_client::Allocation {
                address: Some(FAKE_ALLOCATION_1_ADDRESS),
                size: Some(FAKE_ALLOCATION_1_SIZE),
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                timestamp: Some(FAKE_ALLOCATION_1_TIMESTAMP),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::StackTrace(fheapdump_client::StackTrace {
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                program_addresses: Some(vec![1111, 2222]),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::ThreadInfo(fheapdump_client::ThreadInfo {
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                koid: Some(FAKE_THREAD_1_KOID),
                name: Some(FAKE_THREAD_1_NAME.to_string()),
                ..Default::default()
            }),
        ]);
        fut.await.unwrap();

        // Send the second chunk.
        let fut = receiver_proxy.batch(&[fheapdump_client::SnapshotElement::StackTrace(
            fheapdump_client::StackTrace {
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                program_addresses: Some(vec![3333]),
                ..Default::default()
            },
        )]);
        fut.await.unwrap();

        // Send the end of stream marker.
        let fut = receiver_proxy.batch(&[]);
        fut.await.unwrap();

        // Verify that the stack trace has been reconstructed correctly.
        let received_snapshot = receive_worker.await.unwrap();
        let allocation1 = received_snapshot
            .allocations
            .iter()
            .find(|alloc| alloc.address == Some(FAKE_ALLOCATION_1_ADDRESS))
            .unwrap();
        assert_eq!(allocation1.stack_trace.program_addresses, [1111, 2222, 3333]);
    }

    #[fuchsia::test]
    async fn test_empty_block_contents() {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fheapdump_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_single_from(receiver_stream));

        // Send a zero-sized allocation and its empty contents.
        let fut = receiver_proxy.batch(&[
            fheapdump_client::SnapshotElement::Allocation(fheapdump_client::Allocation {
                address: Some(FAKE_ALLOCATION_1_ADDRESS),
                size: Some(0),
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                timestamp: Some(FAKE_ALLOCATION_1_TIMESTAMP),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::ThreadInfo(fheapdump_client::ThreadInfo {
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                koid: Some(FAKE_THREAD_1_KOID),
                name: Some(FAKE_THREAD_1_NAME.to_string()),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::StackTrace(fheapdump_client::StackTrace {
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                program_addresses: Some(FAKE_STACK_TRACE_1_ADDRESSES.to_vec()),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::BlockContents(fheapdump_client::BlockContents {
                address: Some(FAKE_ALLOCATION_1_ADDRESS),
                contents: Some(vec![]),
                ..Default::default()
            }),
        ]);
        fut.await.unwrap();

        // Send the end of stream marker.
        let fut = receiver_proxy.batch(&[]);
        fut.await.unwrap();

        // Verify that the allocation has been reconstructed correctly.
        let received_snapshot = receive_worker.await.unwrap();
        let allocation1 = received_snapshot
            .allocations
            .iter()
            .find(|alloc| alloc.address == Some(FAKE_ALLOCATION_1_ADDRESS))
            .unwrap();
        assert_eq!(allocation1.contents.as_ref().expect("contents must be set"), &[] as &[u8]);
    }

    #[fuchsia::test]
    async fn test_chunked_block_contents() {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fheapdump_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_single_from(receiver_stream));

        // Split the contents in two halves.
        let (content_first_chunk, contents_second_chunk) =
            FAKE_ALLOCATION_1_CONTENTS.split_at(FAKE_ALLOCATION_1_CONTENTS.len() / 2);

        // Send an allocation and the first chunk of its contents.
        let fut = receiver_proxy.batch(&[
            fheapdump_client::SnapshotElement::Allocation(fheapdump_client::Allocation {
                address: Some(FAKE_ALLOCATION_1_ADDRESS),
                size: Some(FAKE_ALLOCATION_1_SIZE),
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                timestamp: Some(FAKE_ALLOCATION_1_TIMESTAMP),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::ThreadInfo(fheapdump_client::ThreadInfo {
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                koid: Some(FAKE_THREAD_1_KOID),
                name: Some(FAKE_THREAD_1_NAME.to_string()),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::StackTrace(fheapdump_client::StackTrace {
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                program_addresses: Some(FAKE_STACK_TRACE_1_ADDRESSES.to_vec()),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::BlockContents(fheapdump_client::BlockContents {
                address: Some(FAKE_ALLOCATION_1_ADDRESS),
                contents: Some(content_first_chunk.to_vec()),
                ..Default::default()
            }),
        ]);
        fut.await.unwrap();

        // Send the second chunk.
        let fut = receiver_proxy.batch(&[fheapdump_client::SnapshotElement::BlockContents(
            fheapdump_client::BlockContents {
                address: Some(FAKE_ALLOCATION_1_ADDRESS),
                contents: Some(contents_second_chunk.to_vec()),
                ..Default::default()
            },
        )]);
        fut.await.unwrap();

        // Send the end of stream marker.
        let fut = receiver_proxy.batch(&[]);
        fut.await.unwrap();

        // Verify that the allocation's block contents have been reconstructed correctly.
        let received_snapshot = receive_worker.await.unwrap();
        let allocation1 = received_snapshot
            .allocations
            .iter()
            .find(|a| a.address == Some(FAKE_ALLOCATION_1_ADDRESS))
            .unwrap();
        assert_eq!(allocation1.contents, Some(FAKE_ALLOCATION_1_CONTENTS.to_vec()));
    }

    #[fuchsia::test]
    async fn test_missing_end_of_stream() {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fheapdump_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_single_from(receiver_stream));

        // Send an allocation and its stack trace.
        let fut = receiver_proxy.batch(&[
            fheapdump_client::SnapshotElement::Allocation(fheapdump_client::Allocation {
                address: Some(FAKE_ALLOCATION_1_ADDRESS),
                size: Some(FAKE_ALLOCATION_1_SIZE),
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                timestamp: Some(FAKE_ALLOCATION_1_TIMESTAMP),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::ThreadInfo(fheapdump_client::ThreadInfo {
                thread_info_key: Some(FAKE_THREAD_1_KEY),
                koid: Some(FAKE_THREAD_1_KOID),
                name: Some(FAKE_THREAD_1_NAME.to_string()),
                ..Default::default()
            }),
            fheapdump_client::SnapshotElement::StackTrace(fheapdump_client::StackTrace {
                stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                program_addresses: Some(FAKE_STACK_TRACE_1_ADDRESSES.to_vec()),
                ..Default::default()
            }),
        ]);
        fut.await.unwrap();

        // Close the channel without sending an end of stream marker.
        std::mem::drop(receiver_proxy);

        // Expect an UnexpectedEndOfStream error.
        assert_matches!(receive_worker.await, Err(Error::UnexpectedEndOfStream));
    }

    #[fuchsia::test]
    async fn test_multi_contents() {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fheapdump_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_multi_from(receiver_stream));

        // Send two snapshots with different KOIDs.
        for koid in [1111, 2222] {
            receiver_proxy
                .batch(&[fheapdump_client::SnapshotElement::SnapshotHeader(
                    fheapdump_client::SnapshotHeader {
                        process_name: Some(format!("test-process-{koid}")),
                        process_koid: Some(koid),
                        ..Default::default()
                    },
                )])
                .await
                .unwrap();

            receiver_proxy
                .batch(&[
                    fheapdump_client::SnapshotElement::Allocation(fheapdump_client::Allocation {
                        address: Some(FAKE_ALLOCATION_1_ADDRESS),
                        size: Some(FAKE_ALLOCATION_1_SIZE),
                        thread_info_key: Some(FAKE_THREAD_1_KEY),
                        stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                        timestamp: Some(FAKE_ALLOCATION_1_TIMESTAMP),
                        ..Default::default()
                    }),
                    fheapdump_client::SnapshotElement::ThreadInfo(fheapdump_client::ThreadInfo {
                        thread_info_key: Some(FAKE_THREAD_1_KEY),
                        koid: Some(FAKE_THREAD_1_KOID),
                        name: Some(FAKE_THREAD_1_NAME.to_string()),
                        ..Default::default()
                    }),
                    fheapdump_client::SnapshotElement::StackTrace(fheapdump_client::StackTrace {
                        stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                        program_addresses: Some(FAKE_STACK_TRACE_1_ADDRESSES.to_vec()),
                        ..Default::default()
                    }),
                ])
                .await
                .unwrap();

            receiver_proxy.batch(&[]).await.unwrap(); // end of snapshot
        }

        // Send end of stream marker.
        receiver_proxy.batch(&[]).await.unwrap();

        // Validate the received snapshots.
        let received_snapshots = receive_worker.await.unwrap();
        assert_eq!(received_snapshots.len(), 2);
        assert_eq!(received_snapshots[0].process_name, "test-process-1111");
        assert_eq!(received_snapshots[0].process_koid, 1111);
        assert_eq!(received_snapshots[1].process_name, "test-process-2222");
        assert_eq!(received_snapshots[1].process_koid, 2222);
    }

    #[fuchsia::test]
    async fn test_multi_missing_end_of_stream() {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fheapdump_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_multi_from(receiver_stream));

        // Send two snapshots with different KOIDs.
        for koid in [1111, 2222] {
            receiver_proxy
                .batch(&[fheapdump_client::SnapshotElement::SnapshotHeader(
                    fheapdump_client::SnapshotHeader {
                        process_name: Some("test-process-name".to_string()),
                        process_koid: Some(koid),
                        ..Default::default()
                    },
                )])
                .await
                .unwrap();

            receiver_proxy
                .batch(&[
                    fheapdump_client::SnapshotElement::Allocation(fheapdump_client::Allocation {
                        address: Some(FAKE_ALLOCATION_1_ADDRESS),
                        size: Some(FAKE_ALLOCATION_1_SIZE),
                        thread_info_key: Some(FAKE_THREAD_1_KEY),
                        stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                        timestamp: Some(FAKE_ALLOCATION_1_TIMESTAMP),
                        ..Default::default()
                    }),
                    fheapdump_client::SnapshotElement::ThreadInfo(fheapdump_client::ThreadInfo {
                        thread_info_key: Some(FAKE_THREAD_1_KEY),
                        koid: Some(FAKE_THREAD_1_KOID),
                        name: Some(FAKE_THREAD_1_NAME.to_string()),
                        ..Default::default()
                    }),
                    fheapdump_client::SnapshotElement::StackTrace(fheapdump_client::StackTrace {
                        stack_trace_key: Some(FAKE_STACK_TRACE_1_KEY),
                        program_addresses: Some(FAKE_STACK_TRACE_1_ADDRESSES.to_vec()),
                        ..Default::default()
                    }),
                ])
                .await
                .unwrap();

            receiver_proxy.batch(&[]).await.unwrap(); // end of snapshot
        }

        // Close the channel without sending an end of stream marker.
        std::mem::drop(receiver_proxy);

        // Expect an UnexpectedEndOfStream error.
        assert_matches!(receive_worker.await, Err(Error::UnexpectedEndOfStream));
    }
}
