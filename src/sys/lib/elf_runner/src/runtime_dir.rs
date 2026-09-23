// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl::endpoints::ServerEnd;
use fidl_fuchsia_io as fio;
use pseudo_fs::{
    LazyPseudoDirectory, LazyPseudoDirectoryState, PseudoDirectory, PseudoFile, ToPseudoDirectory,
};
use std::sync::Arc;
use vfs::ToObjectRequest;
use vfs::directory::entry::{DirectoryEntry, OpenRequest};
use vfs::directory::helper::DirectlyMutable;
use vfs::execution_scope::ExecutionScope;

// Simple directory type which is used to implement `ComponentStartInfo.runtime_directory`.
pub struct RuntimeDirectory(Arc<LazyPseudoDirectory<RuntimeDirectoryInfo>>);

struct RuntimeDirectoryInfo {
    args: Box<[Box<str>]>,
    job_id: Option<u64>,
    process_id: Option<u64>,
    process_start_time: Option<i64>,
    process_start_time_utc_estimate: Option<Box<str>>,
}

impl ToPseudoDirectory for RuntimeDirectoryInfo {
    fn to_pseudo_directory(self) -> Arc<PseudoDirectory> {
        // Create the runtime tree structure:
        //
        // runtime
        // |- args
        // |  |- 0
        // |  |- 1
        // |  \- ...
        // \- elf
        //    |- job_id
        //    |- process_id
        //    |- process_start_time
        //    \- process_start_time_utc_estimate

        let args_dir = PseudoDirectory::new();
        for (i, arg) in self.args.iter().enumerate() {
            args_dir
                .add_entry(i.to_string(), PseudoFile::from_data(arg.as_bytes()))
                .expect("Failed to add arg to runtime/args directory");
        }

        let elf_dir = PseudoDirectory::new();
        if let Some(job_id) = self.job_id {
            elf_dir
                .add_entry("job_id", PseudoFile::from_data(job_id.to_string()))
                .expect("Failed to add job_id to runtime/elf directory");
        }
        if let Some(process_id) = self.process_id {
            elf_dir
                .add_entry("process_id", PseudoFile::from_data(process_id.to_string()))
                .expect("Failed to add process_id to runtime/elf directory");
        }
        if let Some(process_start_time) = self.process_start_time {
            elf_dir
                .add_entry(
                    "process_start_time",
                    PseudoFile::from_data(process_start_time.to_string()),
                )
                .expect("Failed to add process_start_time to runtime/elf directory");
        }
        if let Some(process_start_time_utc_estimate) = self.process_start_time_utc_estimate {
            elf_dir
                .add_entry(
                    "process_start_time_utc_estimate",
                    PseudoFile::from_data(process_start_time_utc_estimate.as_bytes()),
                )
                .expect("Failed to add process_start_time_utc_estimate to runtime/elf directory");
        }

        let dir = PseudoDirectory::new();
        dir.add_entry("args", args_dir).expect("Failed to add args directory to runtime directory");
        dir.add_entry("elf", elf_dir).expect("Failed to add elf directory to runtime directory");
        dir
    }
}

impl RuntimeDirectory {
    pub fn add_process_id(&self, value: u64) {
        match self.0.state() {
            LazyPseudoDirectoryState::Data(mut data) => data.process_id = Some(value),
            LazyPseudoDirectoryState::Directory(dir) => get_elf_dir(&*dir)
                .add_entry("process_id", PseudoFile::from_data(value.to_string()))
                .expect("failed to add process_id"),
        }
    }

    pub fn add_process_start_time(&self, value: i64) {
        match self.0.state() {
            LazyPseudoDirectoryState::Data(mut data) => data.process_start_time = Some(value),
            LazyPseudoDirectoryState::Directory(dir) => get_elf_dir(&*dir)
                .add_entry("process_start_time", PseudoFile::from_data(value.to_string()))
                .expect("failed to add process_start_time"),
        }
    }

    pub fn add_process_start_time_utc_estimate(&self, value: String) {
        match self.0.state() {
            LazyPseudoDirectoryState::Data(mut data) => {
                data.process_start_time_utc_estimate = Some(value.into_boxed_str())
            }
            LazyPseudoDirectoryState::Directory(dir) => get_elf_dir(&*dir)
                .add_entry("process_start_time_utc_estimate", PseudoFile::from_data(value))
                .expect("failed to add process_start_time_utc_estimate"),
        }
    }

    // Create an empty runtime directory, for test purpose only.
    #[cfg(test)]
    pub fn empty() -> Self {
        RuntimeDirectory(LazyPseudoDirectory::new(RuntimeDirectoryInfo {
            args: Box::default(),
            job_id: None,
            process_id: None,
            process_start_time: None,
            process_start_time_utc_estimate: None,
        }))
    }
}

pub struct RuntimeDirBuilder {
    args: Box<[Box<str>]>,
    job_id: Option<u64>,
    server_end: ServerEnd<fio::DirectoryMarker>,
}

impl RuntimeDirBuilder {
    pub fn new(server_end: ServerEnd<fio::DirectoryMarker>) -> Self {
        Self { args: Box::default(), job_id: None, server_end }
    }

    pub fn args(mut self, args: Vec<String>) -> Self {
        self.args = args.into_iter().map(|arg| arg.into_boxed_str()).collect();
        self
    }

    pub fn job_id(mut self, job_id: u64) -> Self {
        self.job_id = Some(job_id);
        self
    }

    pub fn serve(self) -> RuntimeDirectory {
        let runtime_directory = LazyPseudoDirectory::new(RuntimeDirectoryInfo {
            args: self.args,
            job_id: self.job_id,
            process_id: None,
            process_start_time: None,
            process_start_time_utc_estimate: None,
        });
        let flags = fio::PERM_READABLE | fio::PERM_WRITABLE;
        let object_request = flags.to_object_request(self.server_end);
        object_request.handle(|object_request| {
            let open_request =
                OpenRequest::new(ExecutionScope::new(), flags, vfs::Path::dot(), object_request);
            runtime_directory.clone().open_entry(open_request)
        });

        RuntimeDirectory(runtime_directory)
    }
}

fn get_elf_dir(dir: &PseudoDirectory) -> Arc<PseudoDirectory> {
    dir.get_entry("elf")
        .expect("elf directory should be present")
        .into_any()
        .downcast::<PseudoDirectory>()
        .expect("could not downcast elf to a directory")
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuchsia_fs::directory::read_file_to_string;

    async fn force_create_directory(client: &fio::DirectoryProxy) {
        fuchsia_fs::directory::open_directory(client, "elf", fio::PERM_READABLE)
            .await
            .expect("failed to open elf directory");
    }

    #[fuchsia::test]
    async fn test_read_job_id() {
        let (client, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        RuntimeDirBuilder::new(server_end).job_id(1234).serve();

        assert_eq!(
            read_file_to_string(&client, "elf/job_id").await.expect("failed to read file"),
            "1234"
        );
    }

    #[fuchsia::test]
    async fn test_read_args() {
        let (client, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        RuntimeDirBuilder::new(server_end)
            .args(vec!["arg1".to_string(), "arg2".to_string()])
            .serve();

        assert_eq!(
            read_file_to_string(&client, "args/0").await.expect("failed to read file"),
            "arg1"
        );
        assert_eq!(
            read_file_to_string(&client, "args/1").await.expect("failed to read file"),
            "arg2"
        );
    }

    #[fuchsia::test]
    async fn test_add_process_id_before_connecting() {
        let (client, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let runtime_directory = RuntimeDirBuilder::new(server_end).serve();
        assert!(runtime_directory.0.state().is_data());

        runtime_directory.add_process_id(1234);
        assert_eq!(
            read_file_to_string(&client, "elf/process_id").await.expect("failed to read file"),
            "1234"
        );
    }

    #[fuchsia::test]
    async fn test_add_process_id_after_connecting() {
        let (client, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let runtime_directory = RuntimeDirBuilder::new(server_end).serve();
        force_create_directory(&client).await;
        assert!(runtime_directory.0.state().is_directory());

        runtime_directory.add_process_id(1234);
        assert_eq!(
            read_file_to_string(&client, "elf/process_id").await.expect("failed to read file"),
            "1234"
        );
    }

    #[fuchsia::test]
    async fn test_add_process_start_time_before_connecting() {
        let (client, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let runtime_directory = RuntimeDirBuilder::new(server_end).serve();
        assert!(runtime_directory.0.state().is_data());

        runtime_directory.add_process_start_time(1234);
        assert_eq!(
            read_file_to_string(&client, "elf/process_start_time")
                .await
                .expect("failed to read file"),
            "1234"
        );
    }

    #[fuchsia::test]
    async fn test_add_process_start_time_after_connecting() {
        let (client, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let runtime_directory = RuntimeDirBuilder::new(server_end).serve();
        force_create_directory(&client).await;
        assert!(runtime_directory.0.state().is_directory());

        runtime_directory.add_process_start_time(1234);
        assert_eq!(
            read_file_to_string(&client, "elf/process_start_time")
                .await
                .expect("failed to read file"),
            "1234"
        );
    }

    #[fuchsia::test]
    async fn test_add_process_start_time_utc_estimate_before_connecting() {
        let (client, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let runtime_directory = RuntimeDirBuilder::new(server_end).serve();
        assert!(runtime_directory.0.state().is_data());

        runtime_directory.add_process_start_time_utc_estimate("start-time".to_string());
        assert_eq!(
            read_file_to_string(&client, "elf/process_start_time_utc_estimate")
                .await
                .expect("failed to read file"),
            "start-time"
        );
    }

    #[fuchsia::test]
    async fn test_add_process_start_time_utc_estimate_after_connecting() {
        let (client, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let runtime_directory = RuntimeDirBuilder::new(server_end).serve();
        force_create_directory(&client).await;
        assert!(runtime_directory.0.state().is_directory());

        runtime_directory
            .add_process_start_time_utc_estimate("start-time-utc-estimate".to_string());
        assert_eq!(
            read_file_to_string(&client, "elf/process_start_time_utc_estimate")
                .await
                .expect("failed to read file"),
            "start-time-utc-estimate"
        );
    }
}
