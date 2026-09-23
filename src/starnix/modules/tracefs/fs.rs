// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::tracing_directory::TraceMarkerFile;
use starnix_core::perf::{TraceEventQueue, TraceEventQueueList};
use starnix_core::task::CurrentTask;
use starnix_core::vfs::pseudo::dynamic_file::ConstFile;
use starnix_core::vfs::pseudo::simple_directory::SimpleDirectory;
use starnix_core::vfs::pseudo::simple_file::{BytesFile, BytesFileOps, SimpleFileNode};
use starnix_core::vfs::{
    CacheMode, FileObject, FileOps, FileSystem, FileSystemHandle, FileSystemOps, FileSystemOptions,
    FsNodeOps, FsStr, OutputBuffer,
};
use starnix_core::{fileops_impl_nonseekable, fileops_impl_noop_sync};
use starnix_logging::{CATEGORY_TRACE_META, track_stub};
use starnix_sync::{LockDepMutex, TraceFsDataLock};
use starnix_types::PAGE_SIZE;
use starnix_types::vfs::default_statfs;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::mode;
use starnix_uapi::{TRACEFS_MAGIC, errno, error, statfs};
use std::borrow::Cow;
use std::sync::{Arc, LazyLock};

pub fn trace_fs(
    current_task: &CurrentTask,
    options: FileSystemOptions,
) -> Result<FileSystemHandle, Errno> {
    struct TraceFsHandle(FileSystemHandle);

    let handle = current_task.kernel().expando.get_or_init(|| {
        TraceFsHandle(
            TraceFs::new_fs(current_task, options).expect("tracefs constructed with valid options"),
        )
    });
    Ok(handle.0.clone())
}

pub struct TraceFs;

impl FileSystemOps for TraceFs {
    fn statfs(&self, _fs: &FileSystem, _current_task: &CurrentTask) -> Result<statfs, Errno> {
        Ok(default_statfs(TRACEFS_MAGIC))
    }

    fn name(&self) -> &'static FsStr {
        "tracefs".into()
    }
}

impl TraceFs {
    pub fn new_fs(
        current_task: &CurrentTask,
        options: FileSystemOptions,
    ) -> Result<FileSystemHandle, Errno> {
        let kernel = current_task.kernel();
        let event_queue_collection = TraceEventQueueList::from(kernel);
        let fs = FileSystem::new(kernel, CacheMode::Uncached, TraceFs, options)?;
        let dir = SimpleDirectory::new();
        dir.edit(&fs, |dir| {
            dir.entry("trace", TraceFile::new_node(), mode!(IFREG, 0o755));
            dir.subdir("per_cpu", 0o755, |dir| {
                /// A name for each cpu directory, cached to provide a 'static lifetime.
                static CPU_DIR_NAMES: LazyLock<Vec<String>> = LazyLock::new(|| {
                    (0..zx::system_get_num_cpus()).map(|cpu| format!("cpu{}", cpu)).collect()
                });
                for (cpu, dir_name) in CPU_DIR_NAMES.iter().map(|s| s.as_str()).enumerate() {
                    dir.subdir(dir_name, 0o755, |dir| {
                        let ops: Box<dyn FsNodeOps> = Box::new(TraceRawFile::new_node(
                            event_queue_collection.queues[cpu].clone(),
                        ));
                        dir.entry("trace_pipe_raw", ops, mode!(IFREG, 0o444));
                        dir.entry("trace", TraceFile::new_node(), mode!(IFREG, 0o444));
                    });
                }
            });
            dir.subdir("events", 0o755, |dir| {
                dir.entry("header_page", EventsHeaderPage::new_node(), mode!(IFREG, 0o444));
                dir.subdir("ftrace", 0o755, |dir| {
                    dir.subdir("print", 0o755, |dir| {
                        dir.entry("format", FtracePrintFormatFile::new_node(), mode!(IFREG, 0o444));
                    });
                    if kernel.features.selinux_test_suite {
                        // Necessary for the perf_event SELinux testsuite case.
                        // See https://fxbug.dev/398663320.
                        dir.subdir("function", 0o755, |dir| {
                            dir.entry(
                                "id",
                                BytesFile::new_node(b"1\n".to_vec()),
                                mode!(IFREG, 0o444),
                            );
                        });
                    }
                });
                dir.entry("enable", TraceBytesFile::new_node(), mode!(IFREG, 0o755));
            });
            dir.subdir("options", 0o755, |dir| {
                dir.entry("overwrite", TraceBytesFile::new_node(), mode!(IFREG, 0o444));
            });
            dir.entry(
                "tracing_on",
                TracingOnFile::new_node(event_queue_collection.clone()),
                mode!(IFREG, 0o666),
            );
            dir.entry("current_tracer", ConstFile::new_node("nop".into()), mode!(IFREG, 0o755));
            dir.entry(
                "trace_marker",
                TraceMarkerFile::new_node(event_queue_collection.clone()),
                mode!(IFREG, 0o222),
            );
            dir.entry("printk_formats", TraceBytesFile::new_node(), mode!(IFREG, 0o755));
            dir.entry(
                "trace_clock",
                ConstFile::new_node(
                    "[local] global counter uptime perf mono mono_raw boot tai x86-tsc ".into(),
                ),
                mode!(IFREG, 0o755),
            );
            dir.entry("buffer_size_kb", ConstFile::new_node("7".into()), mode!(IFREG, 0o755));
        });

        let root_ino = fs.allocate_ino();
        fs.create_root(root_ino, dir);
        Ok(fs)
    }
}

#[derive(Default)]
struct TraceBytesFile {
    data: LockDepMutex<Vec<u8>, TraceFsDataLock>,
}

impl TraceBytesFile {
    #[track_caller]
    fn new_node() -> impl FsNodeOps {
        SimpleFileNode::new(move |_| Ok(BytesFile::new(Self::default())))
    }
}

impl BytesFileOps for TraceBytesFile {
    fn write(&self, _current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        *self.data.lock() = data;
        Ok(())
    }
    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        let data = self.data.lock().clone();
        Ok(data.into())
    }
}

#[derive(Clone)]
struct TraceRawFile {
    queue: Arc<TraceEventQueue>,
}

impl TraceRawFile {
    pub fn new_node(queue: Arc<TraceEventQueue>) -> impl FsNodeOps {
        SimpleFileNode::new(move |_| Ok(Self { queue: queue.clone() }))
    }
}

impl FileOps for TraceRawFile {
    fileops_impl_nonseekable!();
    fileops_impl_noop_sync!();

    fn read(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        // TODO(b/505532201): Consider supporting larger buffers.
        if data.available() as u64 != *PAGE_SIZE {
            return error!(EINVAL);
        }
        let _guard = fuchsia_trace::async_enter!(
            self.queue.async_id_read,
            CATEGORY_TRACE_META,
            self.queue.read_track_name(),
            "tid" => current_task.tid.id
        );
        self.queue.read(data)
    }

    fn write(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn starnix_core::vfs::InputBuffer,
    ) -> Result<usize, Errno> {
        error!(ENOSYS)
    }
}

#[derive(Clone)]
struct TracingOnFile {
    event_queue_collection: Arc<TraceEventQueueList>,
}

impl TracingOnFile {
    pub fn new_node(event_queue_collection: Arc<TraceEventQueueList>) -> impl FsNodeOps {
        BytesFile::new_node(Self { event_queue_collection })
    }
}

impl BytesFileOps for TracingOnFile {
    fn write(&self, _current_task: &CurrentTask, data: Vec<u8>) -> Result<(), Errno> {
        let state_str = std::str::from_utf8(&data).map_err(|_| errno!(EINVAL))?;
        let clean_state_str = state_str.split('\n').next().unwrap_or("");
        match clean_state_str {
            "0" => self.event_queue_collection.disable(),
            "1" => self.event_queue_collection.enable(),
            _ => error!(EINVAL),
        }
    }

    fn read(&self, _current_task: &CurrentTask) -> Result<Cow<'_, [u8]>, Errno> {
        if self.event_queue_collection.is_enabled() {
            Ok(Cow::Borrowed(&b"1\n"[..]))
        } else {
            Ok(Cow::Borrowed(&b"0\n"[..]))
        }
    }
}

#[derive(Clone)]
struct EventsHeaderPage;
impl EventsHeaderPage {
    fn new_node() -> impl FsNodeOps {
        ConstFile::new_node(
            format!(
                "\tfield: u64 timestamp;\toffset:0;\tsize:8;\tsigned:0\n\
            \tfield: local_t commit;\toffset:8;\tsize:8;\tsigned:1\n\
            \tfield: int overwrite;\toffset:8;\tsize:1;\tsigned:1\n\
            \tfield: char data;\toffset:16;\tsize:{};\tsigned:0\n",
                *PAGE_SIZE - 16
            )
            .into_bytes(),
        )
    }
}

#[derive(Clone)]
struct FtracePrintFormatFile;
impl FtracePrintFormatFile {
    fn new_node() -> impl FsNodeOps {
        ConstFile::new_node(
            "name: print\n\
        ID: 5\n\
        format:\n\
        \tfield:unsigned short common_type;\toffset:0;\tsize:2;\tsigned:0;\n\
        \tfield:unsigned char common_flags;\toffset:2;\tsize:1;\tsigned:0;\n\
        \tfield:unsigned char common_preempt_count;\toffset:3;\tsize:1;\tsigned:0;\n\
        \tfield:int common_pid;\toffset:4;\tsize:4;\tsigned:1;\n\
        \n\
        \tfield:unsigned long ip;\toffset:8;\tsize:8;\tsigned:0;\n\
        \tfield:char buf[];\toffset:16;\tsize:0;\tsigned:0;\n\
        \n\
        print fmt: \"%ps: %s\", (void *)REC->ip, REC- >buf\n"
                .into(),
        )
    }
}

#[derive(Clone)]
struct TraceFile;
impl TraceFile {
    fn new_node() -> impl FsNodeOps {
        track_stub!(TODO("https://fxbug.dev/357665908"), "/sys/kernel/tracing/trace");
        ConstFile::new_node("# tracer: nop\n".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use starnix_core::testing::spawn_kernel_and_run;
    use starnix_core::vfs::buffers::VecOutputBuffer;
    use starnix_core::vfs::{FsNodeInfo, NamespaceNode, OpenAccessCheck};
    use starnix_uapi::auth::FsCred;
    use starnix_uapi::file_mode::mode;
    use starnix_uapi::open_flags::OpenFlags;

    #[::fuchsia::test]
    async fn test_events_header_page_size() {
        spawn_kernel_and_run(async move |current_task| {
            let fs = current_task.fs().root().entry.node.fs();
            let info = FsNodeInfo::new(mode!(IFREG, 0o444), FsCred::root());
            let node = fs.create_node_and_allocate_node_id(EventsHeaderPage::new_node(), info);
            let ns_node = NamespaceNode::new_anonymous_unrooted(current_task, node);
            let file = ns_node
                .open(current_task, OpenAccessCheck::skip(OpenFlags::RDONLY))
                .expect("open header_page node");

            let mut buffer = VecOutputBuffer::new(*PAGE_SIZE as usize);
            let bytes_read = file.read(current_task, &mut buffer).expect("read header_page");
            let content = std::str::from_utf8(&buffer.data()[..bytes_read]).expect("valid utf8");

            let expected_content = format!(
                "\tfield: u64 timestamp;\toffset:0;\tsize:8;\tsigned:0\n\
            \tfield: local_t commit;\toffset:8;\tsize:8;\tsigned:1\n\
            \tfield: int overwrite;\toffset:8;\tsize:1;\tsigned:1\n\
            \tfield: char data;\toffset:16;\tsize:{};\tsigned:0\n",
                *PAGE_SIZE - 16
            );
            assert_eq!(
                content, expected_content,
                "Expected header_page to match exactly:\n{}\nGot:\n{}",
                expected_content, content
            );
        })
        .await;
    }
}
