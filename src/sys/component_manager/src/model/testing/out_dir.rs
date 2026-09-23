// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::mocks::HostFn;
use fidl::endpoints::ServerEnd;
use fidl_fidl_examples_routing_echo::{EchoRequest, EchoRequestStream};
use fidl_fuchsia_io as fio;
use futures::TryStreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use vfs::directory::entry::DirectoryEntry;
use vfs::directory::immutable::simple as pfs;
use vfs::execution_scope::ExecutionScope;
use vfs::file::vmo::read_only;
use vfs::remote::remote_dir;
use vfs::service::host;
use vfs::tree_builder::TreeBuilder;

/// Used to construct and then host an outgoing directory.
#[derive(Clone)]
pub struct OutDir {
    paths: HashMap<cm_types::Path, Arc<dyn DirectoryEntry>>,
}

impl OutDir {
    pub fn new() -> OutDir {
        OutDir { paths: HashMap::new() }
    }

    /// Add a `DirectoryEntry` served at the given path.
    pub fn add_entry(&mut self, path: cm_types::Path, entry: Arc<dyn DirectoryEntry>) {
        self.paths.insert(path, entry);
    }

    /// Adds a file providing the echo protocol at the given path.
    pub fn add_echo_protocol(&mut self, path: cm_types::Path) {
        self.add_entry(path, host(Self::echo_protocol_fn));
    }

    /// Adds a static file at the given path.
    pub fn add_static_file(&mut self, path: cm_types::Path, contents: &str) {
        self.add_entry(path, read_only(String::from(contents)));
    }

    /// Adds the given directory proxy at location "/data".
    pub fn add_directory_proxy(&mut self, test_dir_proxy: &fio::DirectoryProxy) {
        self.add_entry(
            "/data".parse().unwrap(),
            remote_dir(
                fuchsia_fs::directory::clone(&test_dir_proxy).expect("could not clone directory"),
            ),
        );
    }

    /// Build the output directory.
    fn build_out_dir(&self) -> Result<Arc<pfs::Simple>, anyhow::Error> {
        let mut tree = TreeBuilder::empty_dir();
        // Add any external files.
        for (path, entry) in self.paths.iter() {
            let path = path.split();
            let path = path.iter().map(|x| x.as_str()).collect::<Vec<_>>();
            tree.add_entry(&path, entry.clone())?;
        }

        Ok(tree.build())
    }

    /// Returns a function that will host this outgoing directory on the given ServerEnd.
    pub fn host_fn(&self) -> HostFn {
        // Build the output directory.
        let dir = self.build_out_dir().expect("could not build out directory");
        // Construct a function. Each time it is invoked, we connect a new Zircon channel
        // `server_end` to the directory.
        Box::new(move |server_end: ServerEnd<fio::DirectoryMarker>| {
            vfs::directory::serve_on(
                dir.clone(),
                fio::PERM_READABLE | fio::PERM_WRITABLE | fio::Flags::PROTOCOL_DIRECTORY,
                ExecutionScope::new(),
                server_end,
            );
        })
    }

    /// Hosts a new protocol on `server_end` that implements `fidl.examples.routing.echo.Echo`.
    pub async fn echo_protocol_fn(mut stream: EchoRequestStream) {
        while let Some(EchoRequest::EchoString { value, responder }) =
            stream.try_next().await.unwrap()
        {
            responder.send(value.as_ref().map(|s| &**s)).unwrap();
        }
    }
}
