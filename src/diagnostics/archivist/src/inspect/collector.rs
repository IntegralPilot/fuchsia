// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::inspect::container::InspectHandle;
use diagnostics_data::InspectHandleName;
use fidl::endpoints::{DiscoverableProtocolMarker, Proxy};
use fidl_fuchsia_inspect::{TreeMarker, TreeProxy};
use fidl_fuchsia_inspect_deprecated::{InspectMarker, InspectProxy};
use fidl_fuchsia_io as fio;
use futures::stream::StreamExt;
use log::error;
use std::pin::pin;
use std::sync::{Arc, Weak};

/// Pairs a diagnostics data-object's name to the underlying encoding of that data.
pub type InspectHandleDeque = std::collections::VecDeque<(Option<InspectHandleName>, InspectData)>;

/// Data associated with a component.
/// This data is stored by data collectors and passed by the collectors to processors.
#[derive(Debug)]
pub enum InspectData {
    /// A VMO containing data associated with the event.
    Vmo { data: Arc<zx::Vmo>, escrowed: bool },

    /// A file containing data associated with the event.
    ///
    /// Because we can't synchronously retrieve file contents like we can for VMOs, this holds
    /// the full file contents. Future changes should make streaming ingestion feasible.
    File(Vec<u8>),

    /// A connection to a Tree service.
    Tree(TreeProxy),

    /// A connection to the deprecated Inspect service.
    DeprecatedFidl(InspectProxy),
}

fn maybe_load_service<P: DiscoverableProtocolMarker>(
    dir_proxy: &fio::DirectoryProxy,
    entry: &fuchsia_fs::directory::DirEntry,
) -> Result<Option<P::Proxy>, anyhow::Error> {
    if entry.name.ends_with(P::PROTOCOL_NAME) {
        let (proxy, server) = fidl::endpoints::create_proxy::<P>();
        fdio::service_connect_at(
            dir_proxy.as_channel().as_ref(),
            &entry.name,
            server.into_channel(),
        )?;
        return Ok(Some(proxy));
    }
    Ok(None)
}

pub async fn populate_data_map(inspect_handles: &[Weak<InspectHandle>]) -> InspectHandleDeque {
    let mut data_map = InspectHandleDeque::new();
    for inspect_handle in inspect_handles {
        let Some(handle) = inspect_handle.upgrade() else {
            continue;
        };
        match handle.as_ref() {
            InspectHandle::Directory { proxy: dir } => {
                return populate_data_map_from_dir(dir).await;
            }
            InspectHandle::Tree { proxy, name } => {
                data_map.push_back((
                    name.as_ref().map(|name| InspectHandleName::name(name.clone())),
                    InspectData::Tree(proxy.clone()),
                ));
            }
            InspectHandle::Escrow { vmo, name, .. } => {
                data_map.push_back((
                    name.as_ref().map(|name| InspectHandleName::name(name.clone())),
                    InspectData::Vmo { data: Arc::clone(vmo), escrowed: true },
                ));
            }
        }
    }

    data_map
}

/// Searches the directory specified by inspect_directory_proxy for
/// .inspect files and populates the `inspect_data_map` with the found VMOs.
async fn populate_data_map_from_dir(inspect_proxy: &fio::DirectoryProxy) -> InspectHandleDeque {
    // TODO(https://fxbug.dev/42112326): Use a streaming and bounded readdir API when available to avoid
    // being hung.
    let mut entries = pin!(
        fuchsia_fs::directory::readdir_recursive(inspect_proxy, /* timeout= */ None).filter_map(
            |result| {
                async move {
                    // TODO(https://fxbug.dev/42126094): decide how to show directories that we
                    // failed to read.
                    result.ok()
                }
            }
        )
    );
    let mut data_map = InspectHandleDeque::new();
    // TODO(https://fxbug.dev/42138410) convert this async loop to a stream so we can carry backpressure
    while let Some(entry) = entries.next().await {
        // We are only currently interested in inspect VMO files (root.inspect) and
        // inspect services.
        if let Ok(Some(proxy)) = maybe_load_service::<TreeMarker>(inspect_proxy, &entry) {
            data_map.push_back((
                Some(InspectHandleName::filename(entry.name)),
                InspectData::Tree(proxy),
            ));
            continue;
        }

        if let Ok(Some(proxy)) = maybe_load_service::<InspectMarker>(inspect_proxy, &entry) {
            data_map.push_back((
                Some(InspectHandleName::filename(entry.name)),
                InspectData::DeprecatedFidl(proxy),
            ));
            continue;
        }

        if !entry.name.ends_with(".inspect")
            || entry.kind != fuchsia_fs::directory::DirentKind::File
        {
            continue;
        }

        let file_proxy = match fuchsia_fs::directory::open_file_async(
            inspect_proxy,
            &entry.name,
            fio::PERM_READABLE,
        ) {
            Ok(proxy) => proxy,
            Err(_) => {
                // It should be ok to not be able to read a file. The file might be closed by the
                // time we get here.
                continue;
            }
        };

        // Obtain the backing vmo.
        let vmo = match file_proxy.get_backing_memory(fio::VmoFlags::READ).await {
            Ok(vmo) => vmo,
            Err(_) => {
                // It should be ok to not be able to read a file. The file might be closed by the
                // time we get here.
                continue;
            }
        };

        let data = match vmo.map_err(zx::Status::err_from_raw) {
            Ok(vmo) => InspectData::Vmo { data: Arc::new(vmo), escrowed: false },
            Err(err) => {
                match err {
                    zx::Status::NOT_SUPPORTED => {}
                    err => {
                        error!(
                            file:% = entry.name, err:?;
                            "unexpected error from GetBackingMemory",
                        )
                    }
                }
                match fuchsia_fs::file::read(&file_proxy).await {
                    Ok(contents) => InspectData::File(contents),
                    Err(_) => {
                        // It should be ok to not be able to read a file. The file might be closed
                        // by the time we get here.
                        continue;
                    }
                }
            }
        };
        data_map.push_back((Some(InspectHandleName::filename(entry.name)), data));
    }

    data_map
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use diagnostics_assertions::assert_data_tree;
    use fidl::endpoints::create_request_stream;
    use fuchsia_async as fasync;
    use fuchsia_component::server::ServiceFs;
    use fuchsia_inspect::{Inspector, reader};
    use inspect_runtime::TreeServerSendPreference;
    use inspect_runtime::service::spawn_tree_server_with_stream;
    use zx::Peered;

    fn get_vmo(text: &[u8]) -> zx::Vmo {
        let vmo = zx::Vmo::create(4096).unwrap();
        vmo.write(text, 0).unwrap();
        vmo
    }

    #[fuchsia::test]
    async fn populate_data_map_with_trees() {
        let insp1 = Inspector::default();
        let insp2 = Inspector::default();
        let insp3 = Inspector::default();

        insp1.root().record_int("one", 1);
        insp2.root().record_int("two", 2);
        insp3.root().record_int("three", 3);

        let scope = fasync::Scope::new();

        let (tree1, request_stream) = create_request_stream::<TreeMarker>();
        spawn_tree_server_with_stream(
            insp1,
            TreeServerSendPreference::default(),
            request_stream,
            &scope,
        );
        let (tree2, request_stream) = create_request_stream::<TreeMarker>();
        spawn_tree_server_with_stream(
            insp2,
            TreeServerSendPreference::default(),
            request_stream,
            &scope,
        );
        let (tree3, request_stream) = create_request_stream::<TreeMarker>();
        spawn_tree_server_with_stream(
            insp3,
            TreeServerSendPreference::default(),
            request_stream,
            &scope,
        );

        let name1 = Some(InspectHandleName::name("tree1"));
        let name2 = Some(InspectHandleName::name("tree2"));
        let name3 = None;

        let handles = [
            Arc::new(InspectHandle::Tree { proxy: tree1.into_proxy(), name: Some("tree1".into()) }),
            Arc::new(InspectHandle::Tree { proxy: tree2.into_proxy(), name: Some("tree2".into()) }),
            Arc::new(InspectHandle::Tree { proxy: tree3.into_proxy(), name: None }),
        ];
        let data = populate_data_map(&handles.iter().map(Arc::downgrade).collect::<Vec<_>>()).await;
        assert_eq!(data.len(), 3);

        let (_, tree1) = data.iter().find(|(n, _)| *n == name1).unwrap();

        assert_matches!(tree1, InspectData::Tree(t) => {
            let h = reader::read(t).await.unwrap();
            assert_data_tree!(h, root: {
                one: 1i64,
            });
        });

        let (_, tree2) = data.iter().find(|(n, _)| *n == name2).unwrap();

        assert_matches!(tree2, InspectData::Tree(t) => {
            let h = reader::read(t).await.unwrap();
            assert_data_tree!(h, root: {
                two: 2i64,
            });
        });

        let (_, tree3) = data.iter().find(|(n, _)| *n == name3).unwrap();

        assert_matches!(tree3, InspectData::Tree(t) => {
            let h = reader::read(t).await.unwrap();
            assert_data_tree!(h, root: {
                three: 3i64,
            });
        });
    }

    #[fuchsia::test]
    async fn inspect_data_collector() {
        let path = "/test-bindings/out";
        // Make a ServiceFs containing two files.
        // One is an inspect file, and one is not.
        let mut fs = ServiceFs::new();
        let vmo = get_vmo(b"test1");
        let vmo2 = get_vmo(b"test2");
        let vmo3 = get_vmo(b"test3");
        let vmo4 = get_vmo(b"test4");
        fs.dir("diagnostics").add_vmo_file_at("root.inspect", vmo);
        fs.dir("diagnostics").add_vmo_file_at("root_not_inspect", vmo2);
        fs.dir("diagnostics").dir("a").add_vmo_file_at("root.inspect", vmo3);
        fs.dir("diagnostics").dir("b").add_vmo_file_at("root.inspect", vmo4);
        // Create a connection to the ServiceFs.
        let (h0, h1) = fidl::endpoints::create_endpoints();
        fs.serve_connection(h1).unwrap();

        let ns = fdio::Namespace::installed().unwrap();
        ns.bind(path, h0).unwrap();

        fasync::Task::spawn(fs.collect()).detach();

        let (done0, done1) = zx::Channel::create();

        // Run the actual test in a separate thread so that it does not block on FS operations.
        // Use signalling on a zx::Channel to indicate that the test is done.
        std::thread::spawn(move || {
            let done = done1;
            let mut executor = fasync::LocalExecutor::default();

            executor.run_singlethreaded(async {
                let inspect_proxy = Arc::new(InspectHandle::directory(
                    fuchsia_fs::directory::open_in_namespace(
                        &format!("{path}/diagnostics"),
                        fio::PERM_READABLE,
                    )
                    .expect("Failed to open directory"),
                ));
                let extra_data = populate_data_map(&[Arc::downgrade(&inspect_proxy)]).await;
                assert_eq!(3, extra_data.len());

                let assert_extra_data = |path: &str, content: &[u8]| {
                    let (_, extra) = extra_data
                        .iter()
                        .find(|(n, _)| *n == Some(InspectHandleName::filename(path)))
                        .unwrap();

                    match extra {
                        InspectData::Vmo { data: vmo, escrowed: _ } => {
                            let mut buf = [0u8; 5];
                            vmo.read(&mut buf, 0).expect("reading vmo");
                            assert_eq!(content, &buf);
                        }
                        v => {
                            panic!("Expected Vmo, got {v:?}");
                        }
                    }
                };

                assert_extra_data("root.inspect", b"test1");
                assert_extra_data("a/root.inspect", b"test3");
                assert_extra_data("b/root.inspect", b"test4");

                done.signal_peer(zx::Signals::NONE, zx::Signals::USER_0).expect("signalling peer");
            });
        });

        fasync::OnSignals::new(&done0, zx::Signals::USER_0).await.unwrap();
        ns.unbind(path).unwrap();
    }
}
