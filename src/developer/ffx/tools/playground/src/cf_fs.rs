// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_debug_fdomain::realm::{GetAllInstancesError, Instance};
use fdomain_fuchsia_io as fio;
use fdomain_fuchsia_sys2 as sys2;
use futures::channel::oneshot;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::ops::Bound;
use std::sync::{Arc, Mutex, Weak};
use vfs::directory::dirents_sink::{self, AppendResult};
use vfs::directory::entry::{DirectoryEntry, EntryInfo, GetEntryInfo, OpenRequest};
use vfs::directory::entry_container::{Directory, DirectoryWatcher};
use vfs::directory::immutable::connection::ImmutableConnection;
use vfs::directory::traversal_position::TraversalPosition;
use vfs::execution_scope::ExecutionScope;
use vfs::immutable_attributes;
use vfs::node::Node;
use zx_status::Status;

/// This contains a cache of the entire component hierarchy, as a map from
/// path-like string keys to instance information. We refresh this cache every
/// time we successfully open a directory. We don't have a good way of polling
/// CF to know if we should refresh so we chose that heuristic
/// (https://fxbug.dev/331829928)
enum CacheState {
    /// We have a fresh cache ready for use.
    Here(BTreeMap<String, (Instance, Option<Weak<CFDirectory>>)>),
    /// There is no cache ready but someone is refreshing it now. The senders in
    /// the vec will all be sent to when it's time to check again.
    Refreshing(Vec<oneshot::Sender<()>>),
    /// The cache isn't ready and nobody's refreshing it. The observer of this
    /// state should change it to `Refreshing` and start refreshing the cache.
    /// The stale cache state, if available, is included as the argument to the
    /// variant.
    NeedsRefresh(Option<BTreeMap<String, (Instance, Option<Weak<CFDirectory>>)>>),
}

/// Lock-protected portion of [`CFDirectory`]
struct CFDirectoryInner {
    /// Proxy to the `RealmQuery`. We can clone these to use them concurrently
    /// but it makes some operations impossible so better to just have one
    /// that's lock-protected. If we need it outside the lock we can create a
    /// short-lived clone.
    proxy: sys2::RealmQueryProxy,
    /// Cached result from getting a list of component instances from the proxy.
    cache: CacheState,
}

impl CFDirectoryInner {
    /// Construct a new [`CFDirectoryInner`].
    fn new(proxy: sys2::RealmQueryProxy) -> Self {
        CFDirectoryInner { proxy, cache: CacheState::NeedsRefresh(None) }
    }
}

/// Given an [`Instance`] construct a `u64` which somewhat uniquely identifies
/// that instance if possible. Instances have unique IDs that are strings, but
/// they are optional, and we need a `u64` to fill out inode-like fields in vfs.
/// So we generate the ID by hashing the optional string identifier if we have
/// it.
fn id_from_instance(instance: &Instance) -> Option<u64> {
    let Some(instance_id) = &instance.instance_id else {
        return None;
    };

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    instance_id.hash(&mut hasher);
    Some(hasher.finish())
}

/// Represents a category of file which is present in each CFDirectory to
/// provide metadata and namespace access.
enum DotFile {
    EnvDir(&'static str, sys2::OpenDirType),
    InfoFile(&'static str, &'static (dyn Fn(&Instance) -> Option<String> + Send + Sync)),
}

impl DotFile {
    /// The name of this dotfile when present in the folder.
    fn name(&self) -> &str {
        match self {
            DotFile::EnvDir(a, _) => a,
            DotFile::InfoFile(a, _) => a,
        }
    }

    /// Type of directory entry this dotfile uses.
    fn ty(&self) -> fio::DirentType {
        match self {
            DotFile::EnvDir(_, _) => fio::DirentType::Directory,
            DotFile::InfoFile(_, _) => fio::DirentType::File,
        }
    }
}

/// A directory exposing the component topology. Paths are essentially monikers,
/// and there are some hidden/extra files for getting at metadata and the
/// component's namespaces.
#[derive(Clone)]
pub struct CFDirectory {
    /// Inner state.
    inner: Arc<Mutex<CFDirectoryInner>>,
    /// Moniker of the component this [`CFDirectory`] is rooted at.
    path: String,
}

impl CFDirectory {
    /// Dot files in each CF directory.
    const DOTFILES: &'static [DotFile] = &[
        DotFile::InfoFile(".url", &|instance| Some(instance.url.clone())),
        DotFile::InfoFile(".environment", &|instance| instance.environment.clone()),
        DotFile::InfoFile(".instance_id", &|instance| instance.instance_id.clone()),
        DotFile::EnvDir(":out", sys2::OpenDirType::OutgoingDir),
        DotFile::EnvDir(":rt", sys2::OpenDirType::RuntimeDir),
        DotFile::EnvDir(":pkg", sys2::OpenDirType::PackageDir),
        DotFile::EnvDir(":ex", sys2::OpenDirType::ExposedDir),
        DotFile::EnvDir(":ns", sys2::OpenDirType::NamespaceDir),
    ];

    /// Create a new [`CFDirectory`] for the root of the topology from a root realm query.
    pub fn new_root(proxy: sys2::RealmQueryProxy) -> Arc<Self> {
        Arc::new(CFDirectory {
            inner: Arc::new(Mutex::new(CFDirectoryInner::new(proxy))),
            path: ".".to_owned(),
        })
    }

    /// Get an inode-like ID for this instance if possible.
    async fn id(&self) -> Result<Option<u64>, GetAllInstancesError> {
        self.with_cache(|cache| id_from_instance(&cache.get(&self.path)?.0)).await
    }

    /// Refresh the cache of instances if necessary then access it via the given
    /// callback. Using this and this alone to access the cache prevents
    /// footguns with locking semantics.
    async fn with_cache<T>(
        &self,
        f: impl FnOnce(&mut BTreeMap<String, (Instance, Option<Weak<CFDirectory>>)>) -> T,
    ) -> Result<T, GetAllInstancesError> {
        enum NextAction {
            Wait(oneshot::Receiver<()>),
            Refresh(
                sys2::RealmQueryProxy,
                Option<BTreeMap<String, (Instance, Option<Weak<CFDirectory>>)>>,
            ),
        }

        loop {
            let next_action = {
                let mut inner = self.inner.lock().unwrap();
                match &mut inner.cache {
                    CacheState::Here(cache) => {
                        let res = f(cache);
                        return Ok(res);
                    }
                    CacheState::Refreshing(queue) => {
                        let (sender, receiver) = oneshot::channel();
                        queue.push(sender);
                        NextAction::Wait(receiver)
                    }
                    CacheState::NeedsRefresh(stale) => {
                        let stale = stale.take();
                        inner.cache = CacheState::Refreshing(Vec::new());
                        NextAction::Refresh(inner.proxy.clone(), stale)
                    }
                }
            };

            match next_action {
                NextAction::Wait(receiver) => {
                    let _ = receiver.await;
                }
                NextAction::Refresh(proxy, mut stale) => {
                    let mut cache = component_debug_fdomain::realm::get_all_instances(&proxy)
                        .await?
                        .into_iter()
                        .map(|x| {
                            let moniker = x.moniker.to_string();
                            let entry =
                                stale.as_mut().and_then(|y| y.remove(&moniker).and_then(|x| x.1));
                            (moniker, (x, entry))
                        })
                        .collect();
                    let ret = f(&mut cache);

                    let CacheState::Refreshing(queue) = std::mem::replace(
                        &mut self.inner.lock().unwrap().cache,
                        CacheState::Here(cache),
                    ) else {
                        panic!("Broke locking semantics!");
                    };
                    queue.into_iter().for_each(|x| {
                        let _ = x.send(());
                    });
                    return Ok(ret);
                }
            }
        }
    }

    fn do_open(
        self: Arc<Self>,
        scope: ExecutionScope,
        flags: fio::Flags,
        mut path: vfs::path::Path,
        object_request: vfs::ObjectRequestRef<'_>,
    ) {
        // Pop the next component to visit off of the path. If there is none, we
        // are opening the directory itself.
        let Some(next_component) = path.next() else {
            {
                let mut inner = self.inner.lock().unwrap();
                if matches!(inner.cache, CacheState::Here(_)) {
                    let CacheState::Here(got) =
                        std::mem::replace(&mut inner.cache, CacheState::NeedsRefresh(None))
                    else {
                        unreachable!();
                    };
                    inner.cache = CacheState::NeedsRefresh(Some(got));
                }
            }
            object_request
                .take()
                .create_connection_sync::<ImmutableConnection<_>, _>(scope, self, flags);
            return;
        };

        if let Some(dotfile) = Self::DOTFILES.iter().find(|x| x.name() == next_component) {
            let mut object_request = object_request.take();
            scope.clone().spawn(async move {
                /// We need the cache to determine how to service a dotfile, but
                /// we don't want to lock the cache while performing that
                /// service. So we take the cache, generate one of these
                /// actions, drop the cache, then perform the action.
                enum Action {
                    InfoFile(String),
                    EnvDir(sys2::OpenDirType),
                    None,
                }
                let Ok(action) = self
                    .with_cache(|cache| {
                        let Some((instance, _)) = cache.get(&self.path) else {
                            return Action::None;
                        };
                        match dotfile {
                            DotFile::EnvDir(_, dir) => Action::EnvDir(*dir),
                            DotFile::InfoFile(_, f) => {
                                f(instance).map(Action::InfoFile).unwrap_or(Action::None)
                            }
                        }
                    })
                    .await
                else {
                    object_request.shutdown(Status::INTERNAL);
                    return;
                };

                match action {
                    Action::InfoFile(x) => {
                        let _ = object_request.handle(|object_request| {
                            if !path.is_empty() {
                                return Err(Status::NOT_DIR);
                            }
                            vfs::file::serve(vfs::file::read_only(x), scope, &flags, object_request)
                        });
                    }
                    Action::EnvDir(ty) => {
                        let Ok(local_path) = self.path.as_str().try_into() else {
                            object_request.shutdown(Status::INTERNAL);
                            return;
                        };
                        let proxy = self.inner.lock().unwrap().proxy.clone();
                        let Ok(dir) = component_debug_fdomain::dirs::open_instance_directory(
                            &local_path,
                            ty.into(),
                            &proxy,
                        )
                        .await
                        else {
                            object_request.shutdown(Status::INTERNAL);
                            return;
                        };
                        let _ = dir.open(
                            path.as_ref(),
                            flags,
                            &object_request.options(),
                            object_request.take().into_channel(),
                        );
                    }
                    Action::None => (),
                }
            });
        } else {
            let path_to_here = if &self.path == "." {
                next_component.to_owned()
            } else {
                format!("{}/{next_component}", self.path)
            };

            let mut object_request = object_request.take();
            scope.clone().spawn(async move {
                match self
                    .with_cache(|cache| {
                        if let Some((_, node)) = cache.get_mut(&path_to_here) {
                            if let Some(node) = node.as_ref().and_then(Weak::upgrade) {
                                Ok(node)
                            } else {
                                let new_node = Arc::new(CFDirectory {
                                    inner: Arc::clone(&self.inner),
                                    path: path_to_here,
                                });
                                *node = Some(Arc::downgrade(&new_node));
                                Ok(new_node)
                            }
                        } else {
                            Err(Status::NOT_FOUND)
                        }
                    })
                    .await
                    .map_err(|_| Status::INTERNAL)
                    .and_then(|x| x)
                {
                    Ok(x) => x.do_open(scope, flags, path, &mut object_request),
                    Err(e) => object_request.shutdown(e),
                }
            });
        }
    }
}

impl Node for CFDirectory {
    async fn get_attributes(
        &self,
        requested_attributes: fio::NodeAttributesQuery,
    ) -> Result<fio::NodeAttributes2, Status> {
        Ok(immutable_attributes!(
            requested_attributes,
            Immutable {
                protocols: fio::NodeProtocolKinds::DIRECTORY,
                abilities: fio::Operations::GET_ATTRIBUTES
                    | fio::Operations::ENUMERATE
                    | fio::Operations::TRAVERSE,
                id: self.id().await.map_err(|_| Status::BAD_STATE)?,
            }
        ))
    }
}

impl Directory for CFDirectory {
    fn open(
        self: Arc<Self>,
        scope: ExecutionScope,
        path: vfs::path::Path,
        flags: fio::Flags,
        object_request: vfs::ObjectRequestRef<'_>,
    ) -> Result<(), Status> {
        self.do_open(scope, flags, path, object_request);
        Ok(())
    }

    async fn read_dirents(
        &self,
        pos: &TraversalPosition,
        mut sink: Box<dyn dirents_sink::Sink>,
    ) -> Result<(TraversalPosition, Box<dyn dirents_sink::Sealed>), Status> {
        self.with_cache(move |cache| {
            // We build up an iterator of the files in this directory, then at
            // the for loop at the bottom of this closure we use that iterator
            // to fill in sink as far as we can.

            let dotfiles_unsent = match pos {
                TraversalPosition::Start => &Self::DOTFILES[..],
                TraversalPosition::Name(n) if Self::DOTFILES.iter().any(|x| x.name() == n) => {
                    let pos =
                        Self::DOTFILES.iter().enumerate().find(|(_, x)| x.name() == n).unwrap().0;
                    &Self::DOTFILES[pos..]
                }
                TraversalPosition::Name(_) | TraversalPosition::End => &[],
                _ => unreachable!("Unexpected TraversalPosition variant"),
            };

            for item in dotfiles_unsent.iter() {
                match sink.append(&EntryInfo::new(fio::INO_UNKNOWN, item.ty()), item.name()) {
                    AppendResult::Ok(new_sink) => sink = new_sink,
                    AppendResult::Sealed(sealed) => {
                        return Ok((TraversalPosition::Name(item.name().to_owned()), sealed));
                    }
                }
            }

            // The cache is a table of nodes by full path. We want to find only
            // our sub directories. Every sub directory path will have one more
            // slash than the path of this node does.
            let num_slashes = if &self.path == "." {
                0
            } else {
                self.path.chars().filter(|&x| x == '/').count() + 1
            };

            let mut iter = cache
                // All paths of our subdirectories will be prefixed by our path.
                // All paths prefixed by our path will be lexicographically
                // after our path, so start with that.
                .range::<str, _>((Bound::Excluded(self.path.as_str()), Bound::Unbounded))
                // Filter out paths that don't start with our path, or that
                // don't have a separator after the portion that starts with our
                // path (e.g. if we're foo/bar, accept foo/bar/baz but not
                // foo/barbarella)
                .take_while(|(x, _)| {
                    &self.path == "."
                        || (x.starts_with(&self.path) && (x[self.path.len()..].starts_with("/")))
                })
                // Filter out paths that have more than one more slash than us
                // (e.g. if we're foo/bar, return foo/bar/baz but not
                // foo/bar/baz/bang)
                .filter(|(x, _)| x.chars().filter(|&x| x == '/').count() == num_slashes)
                .map(|(x, y)| (x, (&y.0)));
            let mut skip_iter;
            let mut empty_iter = std::iter::empty();
            let iter = match pos {
                TraversalPosition::Name(n) if dotfiles_unsent.is_empty() => {
                    skip_iter = iter.skip_while(|&(x, _)| x < n);
                    (&mut skip_iter) as &mut dyn Iterator<Item = _>
                }
                TraversalPosition::Start | TraversalPosition::Name(_) => &mut iter,
                TraversalPosition::End => &mut empty_iter,
                _ => unreachable!(),
            };

            for (path, instance) in iter {
                let prefix_len = if &self.path == "." { 0 } else { self.path.len() + 1 };
                match sink.append(
                    &EntryInfo::new(
                        id_from_instance(&instance).unwrap_or(fio::INO_UNKNOWN),
                        fio::DirentType::Directory,
                    ),
                    path[prefix_len..].split('/').next().unwrap(),
                ) {
                    AppendResult::Ok(new_sink) => sink = new_sink,
                    AppendResult::Sealed(sealed) => {
                        return Ok((TraversalPosition::Name(path.to_owned()), sealed));
                    }
                }
            }
            Ok((TraversalPosition::End, sink.seal()))
        })
        .await
        .map_err(|_| Status::BAD_STATE)?
    }

    fn register_watcher(
        self: Arc<Self>,
        _scope: ExecutionScope,
        _mask: fio::WatchMask,
        _watcher: DirectoryWatcher,
    ) -> Result<(), Status> {
        Err(Status::NOT_SUPPORTED)
    }

    fn unregister_watcher(self: Arc<Self>, _key: usize) {}
}

impl DirectoryEntry for CFDirectory {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), Status> {
        request.open_dir(self)
    }
}

impl GetEntryInfo for CFDirectory {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use futures::StreamExt;
    use moniker::Moniker;
    use std::collections::HashMap;

    // TODO(https://fxbug.dev/330828033): component_debug has functions like this in its test_utils module
    // but the module is marked test only, and doesn't build on host.
    fn serve_instance_iterator(
        client: Arc<fdomain_client::Client>,
        instances: Vec<sys2::Instance>,
    ) -> fdomain_client::fidl::ClientEnd<sys2::InstanceIteratorMarker> {
        let (client, mut stream) = client.create_request_stream::<sys2::InstanceIteratorMarker>();
        fuchsia_async::Task::spawn(async move {
            let sys2::InstanceIteratorRequest::Next { responder } =
                stream.next().await.unwrap().unwrap();
            responder.send(&instances).unwrap();
            let sys2::InstanceIteratorRequest::Next { responder } =
                stream.next().await.unwrap().unwrap();
            responder.send(&[]).unwrap();
        })
        .detach();
        client
    }

    // TODO(https://fxbug.dev/330828033): See above.
    fn serve_realm_query(
        fdomain_client: Arc<fdomain_client::Client>,
        instances: Vec<sys2::Instance>,
        dirs: HashMap<(String, sys2::OpenDirType), fio::DirectoryProxy>,
    ) -> sys2::RealmQueryProxy {
        let (client, mut stream) =
            fdomain_client.create_proxy_and_stream::<sys2::RealmQueryMarker>();

        let mut instance_map = HashMap::new();
        for instance in instances {
            let moniker = Moniker::parse_str(instance.moniker.as_ref().unwrap()).unwrap();
            let previous = instance_map.insert(moniker.to_string(), instance);
            assert!(previous.is_none());
        }

        let dirs = dirs
            .into_iter()
            .map(|((moniker, opentype), dir)| {
                let moniker = Moniker::parse_str(&moniker).unwrap().to_string();
                ((moniker, opentype), dir)
            })
            .collect::<HashMap<_, _>>();

        fuchsia_async::Task::spawn(async move {
            loop {
                match stream.next().await.unwrap().unwrap() {
                    sys2::RealmQueryRequest::GetAllInstances { responder } => {
                        eprintln!("GetAllInstances call");
                        let instances = instance_map.values().cloned().collect();
                        let iterator = serve_instance_iterator(fdomain_client.clone(), instances);
                        responder.send(Ok(iterator)).unwrap();
                    }
                    sys2::RealmQueryRequest::OpenDirectory {
                        moniker,
                        dir_type,
                        object,
                        responder,
                    } => {
                        eprintln!("OpenDirectory call for {moniker} for {dir_type:?}");
                        let moniker = Moniker::parse_str(&moniker).unwrap().to_string();
                        if let Some(dir) = dirs.get(&(moniker, dir_type)) {
                            dir.clone(object.into_channel().into()).unwrap();
                            responder.send(Ok(())).unwrap();
                        } else {
                            responder.send(Err(sys2::OpenError::NoSuchDir)).unwrap();
                        }
                    }
                    _ => panic!("Unexpected RealmQuery request"),
                }
            }
        })
        .detach();
        client
    }

    #[fuchsia::test]
    async fn test_ns_access() {
        let client = fdomain_local::local_client_empty();
        let core_foo_ns_dir = vfs::pseudo_directory! {
            "top_dir" => vfs::pseudo_directory! {
                "bottom_dir" => vfs::pseudo_directory! {
                    "bottom_dir_file" => vfs::file::read_only("bottom dir file contents"),
                },
                "top_dir_file" => vfs::file::read_only("top dir file contents"),
            },
            "root_file" => vfs::file::read_only("root file contents"),
        };
        let core_foo_ns_dir =
            vfs::directory::serve_read_only(core_foo_ns_dir, ExecutionScope::new(client.clone()));

        let instances = vec![
            sys2::Instance {
                moniker: Some("/".to_owned()),
                url: Some("fuchsia://root.cml".to_owned()),
                instance_id: Some("5551212".to_owned()),
                resolved_info: None,
                environment: Some("root env".to_owned()),
                ..Default::default()
            },
            sys2::Instance {
                moniker: Some("/core".to_owned()),
                url: Some("fuchsia://core.cml".to_owned()),
                instance_id: Some("123456".to_owned()),
                resolved_info: None,
                environment: Some("core env".to_owned()),
                ..Default::default()
            },
            sys2::Instance {
                moniker: Some("/core/foo".to_owned()),
                url: Some("fuchsia://core_foo.cml".to_owned()),
                instance_id: Some("7891011".to_owned()),
                resolved_info: None,
                environment: Some("core foo env".to_owned()),
                ..Default::default()
            },
        ];
        let query = serve_realm_query(
            client.clone(),
            instances.clone(),
            [(("/core/foo".to_owned(), sys2::OpenDirType::NamespaceDir), core_foo_ns_dir)]
                .into_iter()
                .collect(),
        );

        let root = CFDirectory::new_root(query);
        let proxy = vfs::directory::serve_read_only(root, ExecutionScope::new(client.clone()));

        let root_file = fuchsia_fs_fdomain::directory::open_file(
            &proxy,
            "/core/foo/:ns/root_file",
            fio::PERM_READABLE,
        )
        .await
        .unwrap();
        let top_dir_file = fuchsia_fs_fdomain::directory::open_file(
            &proxy,
            "/core/foo/:ns/top_dir/top_dir_file",
            fio::PERM_READABLE,
        )
        .await
        .unwrap();
        let bottom_dir_file = fuchsia_fs_fdomain::directory::open_file(
            &proxy,
            "/core/foo/:ns/top_dir/bottom_dir/bottom_dir_file",
            fio::PERM_READABLE,
        )
        .await
        .unwrap();

        assert_eq!(
            "root file contents",
            &fuchsia_fs_fdomain::file::read_to_string(&root_file).await.unwrap()
        );
        assert_eq!(
            "top dir file contents",
            &fuchsia_fs_fdomain::file::read_to_string(&top_dir_file).await.unwrap()
        );
        assert_eq!(
            "bottom dir file contents",
            &fuchsia_fs_fdomain::file::read_to_string(&bottom_dir_file).await.unwrap()
        );
    }

    #[fuchsia::test]
    async fn test_component_tree() {
        let client = fdomain_local::local_client_empty();
        let instances = vec![
            sys2::Instance {
                moniker: Some("/".to_owned()),
                url: Some("fuchsia://root.cml".to_owned()),
                instance_id: Some("5551212".to_owned()),
                resolved_info: None,
                environment: Some("root env".to_owned()),
                ..Default::default()
            },
            sys2::Instance {
                moniker: Some("/core".to_owned()),
                url: Some("fuchsia://core.cml".to_owned()),
                instance_id: Some("123456".to_owned()),
                resolved_info: None,
                environment: Some("core env".to_owned()),
                ..Default::default()
            },
            sys2::Instance {
                moniker: Some("/core/foo".to_owned()),
                url: Some("fuchsia://core_foo.cml".to_owned()),
                instance_id: Some("7891011".to_owned()),
                resolved_info: None,
                environment: Some("core foo env".to_owned()),
                ..Default::default()
            },
            sys2::Instance {
                moniker: Some("/core/bar".to_owned()),
                url: Some("fuchsia://core_bar.cml".to_owned()),
                instance_id: Some("000".to_owned()),
                resolved_info: None,
                environment: Some("core bar env".to_owned()),
                ..Default::default()
            },
        ];
        let query = serve_realm_query(client.clone(), instances.clone(), HashMap::new());

        let root = CFDirectory::new_root(query);
        let proxy = vfs::directory::serve_read_only(root, ExecutionScope::new(client.clone()));

        for instance in &instances {
            let instance_moniker = instance.moniker.as_deref().unwrap();
            let proxy = fuchsia_fs_fdomain::directory::open_directory(
                &proxy,
                instance_moniker,
                fio::PERM_READABLE,
            )
            .await
            .unwrap();
            let mut items = fuchsia_fs_fdomain::directory::readdir(&proxy)
                .await
                .unwrap()
                .into_iter()
                .map(|x| (x.name.clone(), x))
                .collect::<HashMap<_, _>>();
            for child in &instances {
                let child_moniker = child.moniker.as_deref().unwrap();
                if child_moniker.starts_with(instance_moniker) {
                    let child_moniker = &child_moniker[instance_moniker.len()..];
                    if child_moniker.is_empty() {
                        continue;
                    }
                    let child_moniker = if child_moniker.starts_with("/") {
                        &child_moniker[1..]
                    } else {
                        child_moniker
                    };
                    assert!(!child_moniker.is_empty());
                    if child_moniker.contains('/') {
                        continue;
                    }
                    let root = items.remove(child_moniker).unwrap();
                    assert_eq!(fuchsia_fs_fdomain::directory::DirentKind::Directory, root.kind);
                }
            }

            let url = items.remove(".url").unwrap();
            assert_eq!(fuchsia_fs_fdomain::directory::DirentKind::File, url.kind);

            let environment = items.remove(".environment").unwrap();
            assert_eq!(fuchsia_fs_fdomain::directory::DirentKind::File, environment.kind);

            let instance_id = items.remove(".instance_id").unwrap();
            assert_eq!(fuchsia_fs_fdomain::directory::DirentKind::File, instance_id.kind);

            let out = items.remove(":out").unwrap();
            assert_eq!(fuchsia_fs_fdomain::directory::DirentKind::Directory, out.kind);

            let rt = items.remove(":rt").unwrap();
            assert_eq!(fuchsia_fs_fdomain::directory::DirentKind::Directory, rt.kind);

            let ex = items.remove(":ex").unwrap();
            assert_eq!(fuchsia_fs_fdomain::directory::DirentKind::Directory, ex.kind);

            let pkg = items.remove(":pkg").unwrap();
            assert_eq!(fuchsia_fs_fdomain::directory::DirentKind::Directory, pkg.kind);

            let ns = items.remove(":ns").unwrap();
            assert_eq!(fuchsia_fs_fdomain::directory::DirentKind::Directory, ns.kind);

            assert!(items.is_empty());

            let fuchsia_fs_fdomain::node::OpenError::OpenError(url_isnt_a_folder) =
                fuchsia_fs_fdomain::directory::open_file(&proxy, ".url/foo", fio::PERM_READABLE)
                    .await
                    .unwrap_err()
            else {
                panic!();
            };
            assert_eq!(Status::NOT_DIR, url_isnt_a_folder);

            let url = fuchsia_fs_fdomain::directory::open_file(&proxy, ".url", fio::PERM_READABLE)
                .await
                .unwrap();
            assert_eq!(instance.url, fuchsia_fs_fdomain::file::read_to_string(&url).await.ok());

            let environment = fuchsia_fs_fdomain::directory::open_file(
                &proxy,
                ".environment",
                fio::PERM_READABLE,
            )
            .await
            .unwrap();
            assert_eq!(
                instance.environment,
                fuchsia_fs_fdomain::file::read_to_string(&environment).await.ok()
            );

            let instance_id = fuchsia_fs_fdomain::directory::open_file(
                &proxy,
                ".instance_id",
                fio::PERM_READABLE,
            )
            .await
            .unwrap();
            assert_eq!(
                instance.instance_id,
                fuchsia_fs_fdomain::file::read_to_string(&instance_id).await.ok()
            );
        }
    }
}
