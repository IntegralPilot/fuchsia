// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use byteorder::{LittleEndian, WriteBytesExt};
use fdomain_client::AsHandleRef;
use fdomain_client::fidl::{RequestStream as FRequestStream, ServerEnd as FServerEnd};
use fdomain_fuchsia_io as fio_f;
use fdomain_fuchsia_sys2 as fsys2_f;
use fidl::endpoints::{ServerEnd, create_endpoints, create_proxy};
use fidl_fuchsia_component_decl::{
    Capability, Component, Dictionary, Expose, ExposeDictionary, ExposeProtocol, ParentRef,
    Protocol, Ref, SelfRef,
};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_sys2 as fsys2;
use futures::{StreamExt, TryStreamExt};
use moniker::Moniker;
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;
use std::rc::Rc;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use zx_status::Status;

/// Builder struct for `RealmQueryResult`/
/// This is an builder interface meant to simplify building of test fixtures.
/// Example usage:
/// ```
///   MockRealmQuery.add()
///   .when("other/component") // when client queries for this string ("other/component").
///   .moniker("./other/component") // Returns the following.
///   .exposes(vec![Expose::Protocol(ExposeProtocol {
///       source: Some(Ref::Self_(SelfRef)),
///       target: Some(Ref::Self_(SelfRef)),
///       source_name: Some("src".to_owned()),
///       target_name: Some("fuchsia.io.SomeOtherThing".to_owned()),
///       ..Default::default()
///   })])
///   .add() // Finish building the result.
///   .when("some/thing") // Start another build.
///   ...
/// ```
#[derive(Default)]
pub struct MockRealmQueryBuilder {
    mapping: HashMap<String, Box<MockRealmQueryBuilderInner>>,
}

/// Inner struct of `MockRealmQueryBuilder` to provide a builder interface for
/// RealmQuery protocol responses.
pub struct MockRealmQueryBuilderInner {
    when: Moniker,
    moniker: Moniker,
    exposes: Vec<Expose>,
    parent: Option<Box<MockRealmQueryBuilder>>,
}

impl MockRealmQueryBuilderInner {
    /// Sets the result moniker.
    pub fn moniker(mut self, moniker: &str) -> Self {
        self.moniker = moniker.try_into().unwrap();
        self
    }

    /// Sets the result vector of `Expose`s.
    pub fn exposes(mut self, exposes: Vec<Expose>) -> Self {
        self.exposes = exposes;
        self
    }

    /// Sets the accessors exposed by the component in the diagnostics-accessors dictionary.
    pub fn accessors(mut self, accessors: &[&str]) -> Self {
        let mut exposes = vec![Expose::Dictionary(ExposeDictionary {
            source_name: Some("diagnostics-accessors".into()),
            target_name: Some("diagnostics-accessors".into()),
            source: Some(Ref::Self_(SelfRef)),
            target: Some(Ref::Parent(ParentRef)),
            ..Default::default()
        })];
        for accessor in accessors {
            exposes.push(Expose::Protocol(ExposeProtocol {
                source: Some(Ref::Self_(SelfRef)),
                target: Some(Ref::Parent(ParentRef)),
                source_name: Some((*accessor).to_owned()),
                target_name: Some((*accessor).to_owned()),
                source_dictionary: Some("diagnostics-accessors".to_owned()),
                ..Default::default()
            }));
        }
        self.exposes = exposes;
        self
    }

    /// Completes the build and returns a `MockRealmQueryBuilder`.
    pub fn add(mut self) -> MockRealmQueryBuilder {
        let mut parent = *self.parent.unwrap();
        self.parent = None;

        parent.mapping.insert(self.when.to_string(), Box::new(self));
        parent
    }

    pub fn serve_exposed_dir_f(&self, server_end: FServerEnd<fio_f::DirectoryMarker>, path: &str) {
        let mut mock_dir_top = MockDir::new("expose".to_owned());
        let mut mock_accessors = MockDir::new("diagnostics-accessors".to_owned());
        for expose in &self.exposes {
            let Expose::Protocol(ExposeProtocol {
                source_name: Some(name), source_dictionary, ..
            }) = expose
            else {
                continue;
            };
            if matches!(source_dictionary, Some(d) if d == "diagnostics-accessors") {
                mock_accessors = mock_accessors.add_entry(MockFile::new_arc(name.to_owned()));
            }
        }

        match path {
            "diagnostics-accessors" => {
                fuchsia_async::Task::local(async move {
                    Rc::new(mock_accessors).serve_f(server_end).await
                })
                .detach();
            }
            _ => {
                mock_dir_top = mock_dir_top.add_entry(Rc::new(mock_accessors));
                fuchsia_async::Task::local(async move {
                    Rc::new(mock_dir_top).serve_f(server_end).await
                })
                .detach();
            }
        }
    }

    pub fn serve_exposed_dir(&self, server_end: ServerEnd<fio::DirectoryMarker>, path: &str) {
        let mut mock_dir_top = MockDir::new("expose".to_owned());
        let mut mock_accessors = MockDir::new("diagnostics-accessors".to_owned());
        for expose in &self.exposes {
            let Expose::Protocol(ExposeProtocol {
                source_name: Some(name), source_dictionary, ..
            }) = expose
            else {
                continue;
            };
            if matches!(source_dictionary, Some(d) if d == "diagnostics-accessors") {
                mock_accessors = mock_accessors.add_entry(MockFile::new_arc(name.to_owned()));
            }
        }

        match path {
            "diagnostics-accessors" => {
                fuchsia_async::Task::local(async move {
                    Rc::new(mock_accessors).serve(server_end).await
                })
                .detach();
            }
            _ => {
                mock_dir_top = mock_dir_top.add_entry(Rc::new(mock_accessors));
                fuchsia_async::Task::local(
                    async move { Rc::new(mock_dir_top).serve(server_end).await },
                )
                .detach();
            }
        }
    }

    fn to_instance(&self) -> fsys2::Instance {
        fsys2::Instance {
            moniker: Some(self.moniker.to_string()),
            url: Some("".to_owned()),
            instance_id: None,
            resolved_info: Some(fsys2::ResolvedInfo {
                resolved_url: Some("".to_owned()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn make_manifest(&self) -> Component {
        let capabilities = self
            .exposes
            .iter()
            .map(|expose| match expose {
                Expose::Protocol(ExposeProtocol {
                    source_name: Some(name),
                    source: Some(Ref::Self_(SelfRef)),
                    ..
                }) => Capability::Protocol(Protocol {
                    name: Some(name.clone()),
                    source_path: Some(format!("/svc/{name}")),
                    ..Protocol::default()
                }),
                Expose::Dictionary(ExposeDictionary {
                    source_name: Some(name),
                    source: Some(Ref::Self_(SelfRef)),
                    ..
                }) => Capability::Dictionary(Dictionary {
                    name: Some(name.clone()),
                    source: Some(Ref::Self_(SelfRef)),
                    ..Dictionary::default()
                }),
                _ => unreachable!("we just add protocols for the test purposes"),
            })
            .collect::<Vec<_>>();
        Component {
            capabilities: Some(capabilities),
            exposes: Some(self.exposes.clone()),
            ..Default::default()
        }
    }
}

impl MockRealmQueryBuilder {
    /// Create a new empty `MockRealmQueryBuilder`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a build of `RealmQueryResult` by specifying the
    /// expected query string.
    pub fn when(self, at: &str) -> MockRealmQueryBuilderInner {
        MockRealmQueryBuilderInner {
            when: at.try_into().unwrap(),
            moniker: Moniker::root(),
            exposes: vec![],
            parent: Some(Box::new(self)),
        }
    }

    /// Finish the build and return servable `MockRealmQuery`.
    pub fn build(self) -> MockRealmQuery {
        MockRealmQuery { mapping: self.mapping }
    }

    pub fn prefilled() -> Self {
        Self::new()
            .when("example/component")
            .moniker("./example/component")
            .exposes(vec![
                Expose::Protocol(ExposeProtocol {
                    source: Some(Ref::Self_(SelfRef)),
                    target: Some(Ref::Parent(ParentRef)),
                    source_name: Some("fuchsia.diagnostics.ArchiveAccessor".to_owned()),
                    target_name: Some("fuchsia.diagnostics.ArchiveAccessor".to_owned()),
                    source_dictionary: Some("diagnostics-accessors".to_owned()),
                    ..Default::default()
                }),
                Expose::Dictionary(ExposeDictionary {
                    source_name: Some("diagnostics-accessors".into()),
                    target_name: Some("diagnostics-accessors".into()),
                    source: Some(Ref::Self_(SelfRef)),
                    target: Some(Ref::Parent(ParentRef)),
                    ..Default::default()
                }),
            ])
            .add()
            .when("other/component")
            .moniker("./other/component")
            .exposes(vec![Expose::Protocol(ExposeProtocol {
                source: Some(Ref::Self_(SelfRef)),
                target: Some(Ref::Parent(ParentRef)),
                source_name: Some("src".to_owned()),
                target_name: Some("fuchsia.io.SomeOtherThing".to_owned()),
                ..Default::default()
            })])
            .add()
            .when("other/component")
            .moniker("./other/component")
            .exposes(vec![Expose::Protocol(ExposeProtocol {
                source: Some(Ref::Self_(SelfRef)),
                target: Some(Ref::Parent(ParentRef)),
                source_name: Some("src".to_owned()),
                target_name: Some("fuchsia.io.MagicStuff".to_owned()),
                ..Default::default()
            })])
            .add()
            .when("foo/component")
            .moniker("./foo/component")
            .exposes(vec![
                Expose::Protocol(ExposeProtocol {
                    source: Some(Ref::Self_(SelfRef)),
                    target: Some(Ref::Parent(ParentRef)),
                    source_name: Some("fuchsia.diagnostics.ArchiveAccessor.feedback".to_owned()),
                    target_name: Some("fuchsia.diagnostics.ArchiveAccessor.feedback".to_owned()),
                    source_dictionary: Some("diagnostics-accessors".to_owned()),
                    ..Default::default()
                }),
                Expose::Dictionary(ExposeDictionary {
                    source_name: Some("diagnostics-accessors".into()),
                    target_name: Some("diagnostics-accessors".into()),
                    source: Some(Ref::Self_(SelfRef)),
                    target: Some(Ref::Parent(ParentRef)),
                    ..Default::default()
                }),
            ])
            .add()
            .when("foo/bar/thing:instance")
            .moniker("./foo/bar/thing:instance")
            .exposes(vec![
                Expose::Protocol(ExposeProtocol {
                    source: Some(Ref::Self_(SelfRef)),
                    target: Some(Ref::Parent(ParentRef)),
                    source_name: Some("fuchsia.diagnostics.ArchiveAccessor.feedback".to_owned()),
                    target_name: Some("fuchsia.diagnostics.ArchiveAccessor.feedback".to_owned()),
                    source_dictionary: Some("diagnostics-accessors".to_owned()),
                    ..Default::default()
                }),
                Expose::Dictionary(ExposeDictionary {
                    source_name: Some("diagnostics-accessors".into()),
                    target_name: Some("diagnostics-accessors".into()),
                    source: Some(Ref::Self_(SelfRef)),
                    target: Some(Ref::Parent(ParentRef)),
                    ..Default::default()
                }),
            ])
            .add()
    }
}

/// Provides a mock `RealmQuery` interface.
pub struct MockRealmQuery {
    /// Mapping from Moniker -> Expose.
    mapping: HashMap<String, Box<MockRealmQueryBuilderInner>>,
}

/// Creates the default test fixures for `MockRealmQuery`.
impl Default for MockRealmQuery {
    fn default() -> Self {
        MockRealmQueryBuilder::prefilled().build()
    }
}

impl MockRealmQuery {
    /// Serves the `RealmQuery` interface asynchronously and runs until the program terminates.
    pub async fn serve_f(self: Rc<Self>, object: FServerEnd<fsys2_f::RealmQueryMarker>) {
        let client = object.domain();
        let mut stream = object.into_stream();
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                fsys2_f::RealmQueryRequest::GetInstance { moniker, responder } => {
                    let query_moniker = Moniker::from_str(moniker.as_str()).unwrap();
                    let res = self.mapping.get(&query_moniker.to_string()).unwrap();
                    responder.send(Ok(&res.to_instance())).unwrap();
                }
                fsys2_f::RealmQueryRequest::OpenDirectory {
                    moniker,
                    dir_type,
                    object,
                    responder,
                    ..
                } => {
                    let query_moniker = Moniker::from_str(moniker.as_str()).unwrap();
                    if let Some(res) = self.mapping.get(&query_moniker.to_string()) {
                        if dir_type == fsys2_f::OpenDirType::ExposedDir {
                            // Serve the exposed dir, everything else doesn't get served.
                            res.serve_exposed_dir_f(object, "");
                        }
                        responder.send(Ok(())).unwrap();
                    } else {
                        responder.send(Err(fsys2_f::OpenError::InstanceNotFound)).unwrap();
                    }
                }
                fsys2_f::RealmQueryRequest::GetResolvedDeclaration {
                    moniker, responder, ..
                } => {
                    let query_moniker = Moniker::from_str(moniker.as_str()).unwrap();
                    let res = self.mapping.get(&query_moniker.to_string()).unwrap();
                    let manifest = res.make_manifest();
                    let manifest = fidl::persist(&manifest).unwrap();
                    let (client_end, server_end) =
                        client.create_endpoints::<fsys2_f::ManifestBytesIteratorMarker>();

                    fuchsia_async::Task::spawn(async move {
                        let mut stream = server_end.into_stream();
                        let fsys2_f::ManifestBytesIteratorRequest::Next { responder } =
                            stream.next().await.unwrap().unwrap();
                        responder.send(manifest.as_slice()).unwrap();
                        let fsys2_f::ManifestBytesIteratorRequest::Next { responder } =
                            stream.next().await.unwrap().unwrap();
                        responder.send(&[]).unwrap();
                    })
                    .detach();

                    responder.send(Ok(client_end)).unwrap();
                }
                fsys2_f::RealmQueryRequest::GetAllInstances { responder } => {
                    let instances: Vec<fsys2_f::Instance> =
                        self.mapping.values().map(|m| m.to_instance()).collect();

                    let (client_end, server_end) =
                        client.create_endpoints::<fsys2_f::InstanceIteratorMarker>();

                    fuchsia_async::Task::spawn(async move {
                        let mut stream = server_end.into_stream();
                        let fsys2_f::InstanceIteratorRequest::Next { responder } =
                            stream.next().await.unwrap().unwrap();
                        responder.send(&instances).unwrap();
                        let fsys2_f::InstanceIteratorRequest::Next { responder } =
                            stream.next().await.unwrap().unwrap();
                        responder.send(&[]).unwrap();
                    })
                    .detach();

                    responder.send(Ok(client_end)).unwrap();
                }
                _ => unreachable!("request {:?}", request),
            }
        }
    }

    /// Serves the `RealmQuery` interface asynchronously and runs until the program terminates.
    pub async fn serve(self: Rc<Self>, object: ServerEnd<fsys2::RealmQueryMarker>) {
        let mut stream = object.into_stream();
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                fsys2::RealmQueryRequest::GetInstance { moniker, responder } => {
                    let query_moniker = Moniker::from_str(moniker.as_str()).unwrap();
                    let res = self.mapping.get(&query_moniker.to_string()).unwrap();
                    responder.send(Ok(&res.to_instance())).unwrap();
                }
                fsys2::RealmQueryRequest::OpenDirectory {
                    moniker,
                    dir_type,
                    object,
                    responder,
                    ..
                } => {
                    let query_moniker = Moniker::from_str(moniker.as_str()).unwrap();
                    if let Some(res) = self.mapping.get(&query_moniker.to_string()) {
                        if dir_type == fsys2::OpenDirType::ExposedDir {
                            // Serve the exposed dir, everything else doesn't get served.
                            res.serve_exposed_dir(object, "");
                        }
                        responder.send(Ok(())).unwrap();
                    } else {
                        responder.send(Err(fsys2::OpenError::InstanceNotFound)).unwrap();
                    }
                }
                fsys2::RealmQueryRequest::GetResolvedDeclaration { moniker, responder, .. } => {
                    let query_moniker = Moniker::from_str(moniker.as_str()).unwrap();
                    let res = self.mapping.get(&query_moniker.to_string()).unwrap();
                    let manifest = res.make_manifest();
                    let manifest = fidl::persist(&manifest).unwrap();
                    let (client_end, server_end) =
                        create_endpoints::<fsys2::ManifestBytesIteratorMarker>();

                    fuchsia_async::Task::spawn(async move {
                        let mut stream = server_end.into_stream();
                        let fsys2::ManifestBytesIteratorRequest::Next { responder } =
                            stream.next().await.unwrap().unwrap();
                        responder.send(manifest.as_slice()).unwrap();
                        let fsys2::ManifestBytesIteratorRequest::Next { responder } =
                            stream.next().await.unwrap().unwrap();
                        responder.send(&[]).unwrap();
                    })
                    .detach();

                    responder.send(Ok(client_end)).unwrap();
                }
                fsys2::RealmQueryRequest::GetAllInstances { responder } => {
                    let instances: Vec<fsys2::Instance> =
                        self.mapping.values().map(|m| m.to_instance()).collect();

                    let (client_end, server_end) =
                        create_endpoints::<fsys2::InstanceIteratorMarker>();

                    fuchsia_async::Task::spawn(async move {
                        let mut stream = server_end.into_stream();
                        let fsys2::InstanceIteratorRequest::Next { responder } =
                            stream.next().await.unwrap().unwrap();
                        responder.send(&instances).unwrap();
                        let fsys2::InstanceIteratorRequest::Next { responder } =
                            stream.next().await.unwrap().unwrap();
                        responder.send(&[]).unwrap();
                    })
                    .detach();

                    responder.send(Ok(client_end)).unwrap();
                }
                _ => unreachable!("request {:?}", request),
            }
        }
    }

    /// Serves the `RealmQuery` interface asynchronously and runs until the program terminates.
    /// Then, instead of needing the client to discover the protocol, return the proxy for futher
    /// test use.
    pub async fn get_proxy(self: Rc<Self>) -> fsys2::RealmQueryProxy {
        let (proxy, server_end) = create_proxy::<fsys2::RealmQueryMarker>();
        fuchsia_async::Task::local(async move { self.serve(server_end).await }).detach();
        proxy
    }
}

// Mock directory structure.
pub trait Entry {
    fn open(self: Rc<Self>, path: &str, object: fidl::Channel);
    fn open_f(self: Rc<Self>, path: &str, object: fdomain_client::Channel);
    fn encode(&self, buf: &mut Vec<u8>);
    fn name(&self) -> String;
}

pub struct MockDir {
    subdirs: HashMap<String, Rc<dyn Entry>>,
    name: String,
    at_end: AtomicBool,
}

impl MockDir {
    pub fn new(name: String) -> Self {
        MockDir { name, subdirs: HashMap::new(), at_end: AtomicBool::new(false) }
    }

    pub fn new_arc(name: String) -> Rc<Self> {
        Rc::new(Self::new(name))
    }

    pub fn add_entry(mut self, entry: Rc<dyn Entry>) -> Self {
        self.subdirs.insert(entry.name(), entry);
        self
    }

    async fn serve_f(self: Rc<Self>, object: FServerEnd<fio_f::DirectoryMarker>) {
        let mut stream = object.into_stream();
        let _ = stream.control_handle().send_on_open_(
            zx_status::sys::ZX_OK,
            Some(fio_f::NodeInfoDeprecated::Directory(fio_f::DirectoryObject {})),
        );
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                fio_f::DirectoryRequest::Open { path, object, .. } => {
                    self.clone().open_f(&path, object);
                }
                fio_f::DirectoryRequest::Rewind { responder, .. } => {
                    self.at_end.store(false, Ordering::Relaxed);
                    responder.send(zx_status::sys::ZX_OK).unwrap();
                }
                fio_f::DirectoryRequest::ReadDirents { max_bytes: _, responder, .. } => {
                    let entries = match self.at_end.compare_exchange(
                        false,
                        true,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    ) {
                        Ok(false) => encode_entries(&self.subdirs),
                        Err(true) => Vec::new(),
                        _ => unreachable!(),
                    };
                    responder.send(zx_status::sys::ZX_OK, &entries).unwrap();
                }
                x => panic!("unsupported request: {x:?}"),
            }
        }
    }

    async fn serve(self: Rc<Self>, object: ServerEnd<fio::DirectoryMarker>) {
        let mut stream = object.into_stream();
        while let Ok(Some(request)) = stream.try_next().await {
            match request {
                fio::DirectoryRequest::Open { path, object, .. } => {
                    self.clone().open(&path, object);
                }
                fio::DirectoryRequest::Rewind { responder, .. } => {
                    self.at_end.store(false, Ordering::Relaxed);
                    responder.send(zx_status::sys::ZX_OK).unwrap();
                }
                fio::DirectoryRequest::ReadDirents { max_bytes: _, responder, .. } => {
                    let entries = match self.at_end.compare_exchange(
                        false,
                        true,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    ) {
                        Ok(false) => encode_entries(&self.subdirs),
                        Err(true) => Vec::new(),
                        _ => unreachable!(),
                    };
                    responder.send(zx_status::sys::ZX_OK, &entries).unwrap();
                }
                x => panic!("unsupported request: {x:?}"),
            }
        }
    }
}

fn encode_entries(subdirs: &HashMap<String, Rc<dyn Entry>>) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut data = subdirs.iter().collect::<Vec<(_, _)>>();
    data.sort_by(|a, b| a.0.cmp(b.0));
    for (_, entry) in data.iter() {
        entry.encode(&mut buf);
    }
    buf
}

impl Entry for MockDir {
    fn open(self: Rc<Self>, path: &str, object: fidl::Channel) {
        let path = Path::new(path);
        let mut path_iter = path.iter();
        let segment = if let Some(segment) = path_iter.next() {
            if let Some(segment) = segment.to_str() {
                segment
            } else {
                let _ =
                    ServerEnd::<fio::NodeMarker>::new(object).close_with_epitaph(Status::NOT_FOUND);
                return;
            }
        } else {
            "."
        };
        if segment == "." {
            fuchsia_async::Task::local(self.serve(ServerEnd::new(object))).detach();
            return;
        }
        if let Some(entry) = self.subdirs.get(segment) {
            entry.clone().open(path_iter.as_path().to_str().unwrap(), object);
        } else {
            let _ = ServerEnd::<fio::NodeMarker>::new(object).close_with_epitaph(Status::NOT_FOUND);
        }
    }

    fn open_f(self: Rc<Self>, path: &str, object: fdomain_client::Channel) {
        let path = Path::new(path);
        let mut path_iter = path.iter();
        let segment = if let Some(segment) = path_iter.next() {
            if let Some(segment) = segment.to_str() {
                segment
            } else {
                let _ = FServerEnd::<fio_f::NodeMarker>::new(object)
                    .close_with_epitaph(Status::NOT_FOUND);
                return;
            }
        } else {
            "."
        };
        if segment == "." {
            fuchsia_async::Task::local(self.serve_f(FServerEnd::new(object))).detach();
            return;
        }
        if let Some(entry) = self.subdirs.get(segment) {
            entry.clone().open_f(path_iter.as_path().to_str().unwrap(), object);
        } else {
            let _ =
                FServerEnd::<fio_f::NodeMarker>::new(object).close_with_epitaph(Status::NOT_FOUND);
        }
    }

    fn encode(&self, buf: &mut Vec<u8>) {
        buf.write_u64::<LittleEndian>(fio::INO_UNKNOWN).expect("writing mockdir ino to work");
        buf.write_u8(self.name.len() as u8).expect("writing mockdir size to work");
        buf.write_u8(fio::DirentType::Directory.into_primitive())
            .expect("writing mockdir type to work");
        buf.write_all(self.name.as_ref()).expect("writing mockdir name to work");
    }

    fn name(&self) -> String {
        self.name.clone()
    }
}

struct MockFile {
    name: String,
}

impl MockFile {
    pub fn new(name: String) -> Self {
        MockFile { name }
    }
    pub fn new_arc(name: String) -> Rc<Self> {
        Rc::new(Self::new(name))
    }
}

impl Entry for MockFile {
    fn open(self: Rc<Self>, _path: &str, _object: fidl::Channel) {
        unimplemented!();
    }

    fn open_f(self: Rc<Self>, _path: &str, _object: fdomain_client::Channel) {
        unimplemented!();
    }

    fn encode(&self, buf: &mut Vec<u8>) {
        buf.write_u64::<LittleEndian>(fio::INO_UNKNOWN).expect("writing mockdir ino to work");
        buf.write_u8(self.name.len() as u8).expect("writing mockdir size to work");
        buf.write_u8(fio::DirentType::Service.into_primitive())
            .expect("writing mockdir type to work");
        buf.write_all(self.name.as_ref()).expect("writing mockdir name to work");
    }

    fn name(&self) -> String {
        self.name.clone()
    }
}
