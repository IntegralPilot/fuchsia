// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Error;
use flex_fuchsia_memory_stacktrack_client as fstacktrack_client;
use futures::stream::StreamExt;
use std::collections::HashMap;

/// Contains all the data received over a `SnapshotReceiver` channel.
#[derive(Debug, Default)]
pub struct Snapshot {
    /// The page size of the system, if reported.
    pub page_size: u64,

    /// All the stack traces collected, one per thread, indexed by thread koid.
    pub stack_traces: HashMap<u64, StackTrace>,

    /// All the executable memory regions in the analyzed process.
    pub executable_regions: HashMap<u64, ExecutableRegion>,
}

/// A memory region containing code loaded from an ELF file.
#[derive(Debug)]
pub struct ExecutableRegion {
    /// Region name for human consumption (usually either the ELF soname or the VMO name), if known.
    pub name: String,

    /// Region size, in bytes.
    pub size: u64,

    /// The address of the memory region relative to the file's load address.
    pub vaddr: u64,

    /// The Build ID of the ELF file.
    pub build_id: Vec<u8>,
}

/// A stack trace collected from a thread.
#[derive(Debug)]
pub struct StackTrace {
    /// The stack frames, listed bottom-to-top.
    pub frames: Vec<CallFrame>,
}

/// A frame in a stack trace.
#[derive(Debug, Clone)]
pub struct CallFrame {
    /// The program counter (PC) or return address.
    pub program_address: u64,

    /// The frame pointer (FP).
    pub frame_pointer: u64,
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
    pub async fn receive_from(
        mut stream: fstacktrack_client::SnapshotReceiverRequestStream,
    ) -> Result<Snapshot, Error> {
        let mut page_size = None;
        let mut stack_traces = HashMap::new();
        let mut executable_regions = HashMap::new();

        loop {
            // Wait for the next batch of elements.
            let batch = match stream.next().await.transpose()? {
                Some(fstacktrack_client::SnapshotReceiverRequest::Batch { batch, responder }) => {
                    // Send acknowledgment as quickly as possible, then keep processing the received
                    // batch.
                    responder.send()?;
                    batch
                }
                Some(fstacktrack_client::SnapshotReceiverRequest::ReportError {
                    error,
                    responder,
                }) => {
                    let _ = responder.send(); // Ignore the result of the acknowledgment.
                    return Err(Error::CollectorError(error));
                }
                None => return Err(Error::UnexpectedEndOfStream),
            };

            if batch.is_empty() {
                let page_size = page_size.ok_or(Error::PageSizeMissing)?;
                return Ok(Snapshot { page_size, stack_traces, executable_regions });
            }

            for element in batch {
                match element {
                    fstacktrack_client::SnapshotElement::PageSize(size) => {
                        page_size = Some(size);
                    }
                    fstacktrack_client::SnapshotElement::StackTrace(trace) => {
                        let thread_koid = read_field!(trace => StackTrace, thread_koid)?;
                        let frames = read_field!(trace => StackTrace, frames)?
                            .into_iter()
                            .map(|f| CallFrame {
                                program_address: f.program_address,
                                frame_pointer: f.frame_pointer,
                            })
                            .collect();

                        stack_traces.insert(thread_koid, StackTrace { frames });
                    }
                    fstacktrack_client::SnapshotElement::ExecutableRegion(region) => {
                        let address = read_field!(region => ExecutableRegion, address)?;
                        let name = region.name.unwrap_or_default();
                        let size = read_field!(region => ExecutableRegion, size)?;
                        let vaddr = read_field!(region => ExecutableRegion, vaddr)?;
                        let build_id = read_field!(region => ExecutableRegion, build_id)?.value;

                        executable_regions
                            .insert(address, ExecutableRegion { name, size, vaddr, build_id });
                    }
                    _ => return Err(Error::UnexpectedElementType),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::create_client;
    use fuchsia_async as fasync;
    use test_case::test_case;

    #[fuchsia::test]
    async fn test_receive_snapshot() {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fstacktrack_client::SnapshotReceiverMarker>();

        let receive_task = fasync::Task::local(Snapshot::receive_from(receiver_stream));

        let elements = vec![
            fstacktrack_client::SnapshotElement::PageSize(4096),
            fstacktrack_client::SnapshotElement::ExecutableRegion(
                fstacktrack_client::ExecutableRegion {
                    address: Some(0x10000),
                    size: Some(0x2000),
                    name: Some("test".to_string()),
                    vaddr: Some(0x5000),
                    build_id: Some(fstacktrack_client::BuildId { value: vec![0xAA, 0xBB] }),
                    ..Default::default()
                },
            ),
            fstacktrack_client::SnapshotElement::StackTrace(fstacktrack_client::StackTrace {
                thread_koid: Some(8888),
                frames: Some(vec![fstacktrack_client::CallFrame {
                    program_address: 0x1234,
                    frame_pointer: 0x5678,
                }]),
                ..Default::default()
            }),
        ];

        receiver_proxy.batch(&elements).await.expect("failed to send batch");
        receiver_proxy.batch(&[]).await.expect("failed to send end marker");

        let snapshot = receive_task.await.expect("failed to receive snapshot");

        assert_eq!(snapshot.page_size, 4096);
        assert_eq!(snapshot.executable_regions.len(), 1);
        assert_eq!(snapshot.stack_traces.len(), 1);

        let (address, region) = snapshot.executable_regions.iter().next().unwrap();
        assert_eq!(*address, 0x10000);
        assert_eq!(region.size, 0x2000);
        assert_eq!(region.name, "test");
        assert_eq!(region.vaddr, 0x5000);
        assert_eq!(region.build_id, vec![0xAA, 0xBB]);

        let (thread_koid, trace) = snapshot.stack_traces.iter().next().unwrap();
        assert_eq!(*thread_koid, 8888);
        assert_eq!(trace.frames.len(), 1);
        assert_eq!(trace.frames[0].program_address, 0x1234);
        assert_eq!(trace.frames[0].frame_pointer, 0x5678);
    }

    #[test_case(|trace| trace.thread_koid = None => matches
        Err(Error::MissingField { container: "StackTrace", field: "thread_koid" }) ; "thread_koid")]
    #[test_case(|trace| trace.frames = None => matches
        Err(Error::MissingField { container: "StackTrace", field: "frames" }) ; "frames")]
    #[test_case(|_| () /* if we do not set any field to None, the result should be Ok */ => matches
        Ok(_) ; "success")]
    #[fuchsia::test]
    async fn test_stack_trace_required_fields(
        set_one_field_to_none: fn(&mut fstacktrack_client::StackTrace),
    ) -> Result<Snapshot, Error> {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fstacktrack_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_from(receiver_stream));

        let mut stack_trace = fstacktrack_client::StackTrace {
            thread_koid: Some(123),
            frames: Some(vec![fstacktrack_client::CallFrame {
                program_address: 0x100,
                frame_pointer: 0x200,
            }]),
            ..Default::default()
        };
        set_one_field_to_none(&mut stack_trace);

        // Ignore result, as the peer may detect the error and close the channel.
        let _ = receiver_proxy
            .batch(&[
                fstacktrack_client::SnapshotElement::PageSize(4096),
                fstacktrack_client::SnapshotElement::StackTrace(stack_trace),
            ])
            .await;
        let _ = receiver_proxy.batch(&[]).await;

        receive_worker.await
    }

    #[test_case(|region| region.address = None => matches
        Err(Error::MissingField { container: "ExecutableRegion", field: "address" }) ; "address")]
    #[test_case(|region| region.size = None => matches
        Err(Error::MissingField { container: "ExecutableRegion", field: "size" }) ; "size")]
    #[test_case(|region| region.vaddr = None => matches
        Err(Error::MissingField { container: "ExecutableRegion", field: "vaddr" }) ; "vaddr")]
    #[test_case(|region| region.build_id = None => matches
        Err(Error::MissingField { container: "ExecutableRegion", field: "build_id" }) ; "build_id")]
    #[test_case(|_| () /* if we do not set any field to None, the result should be Ok */ => matches
        Ok(_) ; "success")]
    #[fuchsia::test]
    async fn test_executable_region_required_fields(
        set_one_field_to_none: fn(&mut fstacktrack_client::ExecutableRegion),
    ) -> Result<Snapshot, Error> {
        let client = create_client();
        let (receiver_proxy, receiver_stream) =
            client.create_proxy_and_stream::<fstacktrack_client::SnapshotReceiverMarker>();
        let receive_worker = fasync::Task::local(Snapshot::receive_from(receiver_stream));

        let mut region = fstacktrack_client::ExecutableRegion {
            address: Some(0x10000),
            size: Some(0x2000),
            name: Some("test".to_string()),
            vaddr: Some(0x5000),
            build_id: Some(fstacktrack_client::BuildId { value: vec![0xAA, 0xBB] }),
            ..Default::default()
        };
        set_one_field_to_none(&mut region);

        // Ignore result, as the peer may detect the error and close the channel.
        let _ = receiver_proxy
            .batch(&[
                fstacktrack_client::SnapshotElement::PageSize(4096),
                fstacktrack_client::SnapshotElement::ExecutableRegion(region),
            ])
            .await;
        let _ = receiver_proxy.batch(&[]).await;

        receive_worker.await
    }
}
