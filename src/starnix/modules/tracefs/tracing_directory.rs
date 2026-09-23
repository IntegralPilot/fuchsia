// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use starnix_core::fileops_impl_nonseekable;
use starnix_core::perf::{TraceEvent, TraceEventQueueList};
use starnix_core::task::CurrentTask;
use starnix_core::vfs::buffers::InputBuffer;
use starnix_core::vfs::pseudo::simple_file::SimpleFileNode;
use starnix_core::vfs::{FileObject, FileOps, FsNodeOps, OutputBuffer, fileops_impl_noop_sync};
use starnix_logging::CATEGORY_TRACE_META;

use starnix_uapi::errors::Errno;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

pub struct TraceMarkerFile {
    event_queue_collection: Arc<TraceEventQueueList>,
    num_cpus: usize,
    counter: AtomicUsize,
}
impl TraceMarkerFile {
    pub fn new_node(event_queue_collection: Arc<TraceEventQueueList>) -> impl FsNodeOps {
        let num_cpus = event_queue_collection.queues.len();
        SimpleFileNode::new(move |_| {
            Ok(Self {
                event_queue_collection: event_queue_collection.clone(),
                num_cpus,
                counter: AtomicUsize::new(0),
            })
        })
    }
}

impl FileOps for TraceMarkerFile {
    fileops_impl_noop_sync!();
    fileops_impl_nonseekable!();

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        Ok(0)
    }

    fn write(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        //TODO(b/502606269): Get the current CPU index from the current task.
        let cpu_index =
            self.counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % self.num_cpus;
        let queue = &self.event_queue_collection.queues[cpu_index];
        let _guard = fuchsia_trace::async_enter!(
            queue.async_id_write,
            CATEGORY_TRACE_META,
            queue.write_track_name(),
            "tid" => current_task.get_tid()
        );
        if self.event_queue_collection.is_enabled() {
            let mut bytes = data.read_all()?;
            let bytes_read = bytes.len();
            // The TraceEvent struct appends a new line to the trace data unconditionally, so
            // remove the trailing newline if here to avoid generating empty events when reading.
            if bytes.ends_with(&['\n' as u8]) {
                bytes.truncate(bytes.len() - 1);
            }
            let trace_event = TraceEvent::new(
                // This pid is a Kernel pid (do not confuse with userspace pid aka tgid), so we use
                // the task thread id, the pid and tid are equal when the thread is the "main thread"
                // of the thread group/process.
                // It is used when CPU scheduling information is not available.
                current_task.get_tid(),
                bytes.len(),
            );
            queue.push_event(trace_event, &bytes)?;
            Ok(bytes_read) // Includes '\n', if present
        } else {
            Ok(data.available())
        }
    }
}
