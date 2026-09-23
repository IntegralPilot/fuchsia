// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::component::WeakComponentInstance;
use crate::model::component::instance::InstanceState;
use async_trait::async_trait;
use capability_source::{AggregateInstance, AggregateMember, CapabilitySource, ComponentSource};
use cm_rust::{CapabilityTypeName, ChildRef};
use cm_types::{IterablePath, Name, RelativePath};
use errors::{ModelError, VfsError};
use fidl_fuchsia_component_runtime as fruntime;
use fidl_fuchsia_io as fio;
use flyweights::FlyStr;
use fuchsia_async::{DurationExt, TimeoutExt};
use fuchsia_fs::directory::WatcherCreateError;
use fuchsia_sync::Mutex;
use futures::channel::oneshot;
use futures::future::{BoxFuture, join_all};
use futures::stream::TryStreamExt;
use hooks::{Event, EventPayload, EventType, Hook, HooksRegistration};
use log::{error, warn};
use moniker::{ExtendedMoniker, Moniker};
use rand::Rng;
use routing::component_instance::ComponentInstanceInterface;
use routing::error::RoutingError;
use runtime_capabilities::{DirConnector, Router};
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fmt::Write;
use std::path::Path;
use std::sync::{Arc, Weak};
use vfs::ToObjectRequest;
use vfs::directory::entry::{
    DirectoryEntry, DirectoryEntryAsync, EntryInfo, GetEntryInfo, OpenRequest,
};
use vfs::directory::helper::DirectlyMutable;
use vfs::directory::immutable::simple::{
    Simple as SimpleImmutableDir, simple as simple_immutable_dir,
};

/// Timeout for opening a service capability when aggregating.
const OPEN_SERVICE_TIMEOUT: zx::MonotonicDuration = zx::MonotonicDuration::from_seconds(5);

const FLAGS: fio::Flags = fio::PERM_READABLE.union(fio::Flags::PROTOCOL_DIRECTORY);

/// Safely extracts a string from a VFS event without allocating
/// unless ownership is explicitly required.
pub fn extract_event_filename<'a>(path: &'a Path) -> Option<Cow<'a, str>> {
    let s = path.to_str()?;
    if s == "." {
        // Return a borrowed reference, no allocation
        Some(Cow::Borrowed("."))
    } else {
        // We can still borrow here, deferring .to_owned() until the caller
        // proves they need it.
        Some(Cow::Borrowed(s))
    }
}

/// A zero-reallocation builder for extending paths in loops.
pub struct ReusablePathBuffer {
    buffer: String,
}

impl ReusablePathBuffer {
    pub fn new(base: &RelativePath) -> Self {
        // Pre-allocate enough space for most capability paths
        let mut buffer = String::with_capacity(128);
        write!(&mut buffer, "{}", base).expect("writing to a String buffer never fails");
        Self { buffer }
    }

    pub fn push_segment(&mut self, segment: &str) {
        if self.buffer == "." {
            self.buffer.clear();
        } else if !self.buffer.ends_with('/') {
            self.buffer.push('/');
        }
        self.buffer.push_str(segment);
    }

    pub fn as_str(&self) -> &str {
        &self.buffer
    }
}

/// Generates a hex-encoded 128-bit ID using exactly one allocation.
pub fn generate_hex_name(rng: &mut impl Rng) -> Name {
    let mut num: [u8; 16] = [0; 16];
    rng.fill_bytes(&mut num);

    let mut hex_string = String::with_capacity(32);
    for byte in num {
        write!(&mut hex_string, "{:02x}", byte).unwrap();
    }

    Name::new(hex_string).expect("hex string is valid Name syntax")
}

/// Represents a routed service capability from an anonymized aggregate defined in a component.
#[derive(Debug, Hash, PartialEq, Eq, Clone)]
pub struct AnonymizedServiceRoute {
    /// Moniker of the component that defines the anonymized aggregate.
    pub source_moniker: Moniker,

    /// All members relative to `source_moniker` which make up the aggregate.
    pub members: Vec<AggregateMember>,

    /// Name of the service exposed from the collection.
    pub service_name: Name,
}

impl AnonymizedServiceRoute {
    /// Returns true if the component with `moniker` is a member of a collection or static child in
    /// this route.
    fn matches_child_component(&self, moniker: &Moniker) -> bool {
        let component_parent_moniker = match moniker.parent() {
            Some(moniker) => moniker,
            None => {
                // Component is the root component, and so cannot be in an aggregate.
                return false;
            }
        };

        let component_leaf_name = match moniker.leaf() {
            Some(n) => n,
            None => {
                // Component is the root component, and so cannot be in an aggregate.
                return false;
            }
        };

        if self.source_moniker != component_parent_moniker {
            return false;
        }

        if let Some(collection) = component_leaf_name.collection().as_ref() {
            self.members
                .iter()
                .any(|m| matches!(m, AggregateMember::Collection(c) if c == *collection))
        } else {
            let component_leaf_name: ChildRef = component_leaf_name.into();
            self.members
                .iter()
                .any(|m| matches!(m, AggregateMember::Child(c) if c == &component_leaf_name))
        }
    }
}

enum WatcherEntry {
    /// The watcher has not reached idle yet. The inner sender option will be used by the watcher
    /// to notify when it does transition to the ReachedIdle state if one exists.
    WaitingForIdle(Option<oneshot::Sender<()>>),
    /// The watcher transitions to this state when it has seen at least one idle event.
    ReachedIdle,
}

impl WatcherEntry {
    // Transition the watcher to the ReachedIdle state, notifying the
    // idle_sender if it exists.
    fn transition_to_idle(&mut self) {
        match self {
            WatcherEntry::WaitingForIdle(sender_option) => {
                if let Some(sender) = sender_option.take() {
                    // Ignore the send result since we still transition to
                    // ReachedIdle.
                    let _send_result = sender.send(());
                }
                *self = WatcherEntry::ReachedIdle;
            }
            WatcherEntry::ReachedIdle => {}
        }
    }
}

struct AnonymizedAggregateServiceDirInner {
    /// Directory that contains all aggregated service instances.
    pub dir: Arc<SimpleImmutableDir>,

    /// Directory entries in `dir`.
    ///
    /// This is used to find directory entries after they have been inserted into `dir`,
    /// as `dir` does not directly expose its entries.
    entries:
        HashMap<ServiceInstanceDirectoryKey<AggregateInstance>, Arc<ServiceInstanceDirectoryEntry>>,

    /// This contains entries for directory watchers that are listening for service instances.
    /// The value is an enum to indicate the various states that the watcher can be in.
    watchers_spawned: HashMap<AggregateInstance, WatcherEntry>,
}

/// A provider of a capability from an aggregation of one or more collections and static children.
/// The instance names in the aggregate will be anonymized.
///
/// This trait type-erases the capability type, so it can be handled and hosted generically.
#[async_trait]
pub trait AnonymizedAggregateCapabilityProvider: Send + Sync {
    /// Lists the instances of the capability.
    ///
    /// The instance is an opaque identifier that is only meaningful for a subsequent
    /// call to `route_instance`.
    async fn list_instances(&self) -> Result<Vec<AggregateInstance>, RoutingError>;

    /// Route the given `instance` of the capability to its source.
    async fn route_instance(
        &self,
        instance: &AggregateInstance,
    ) -> Result<(Arc<Router<DirConnector>>, CapabilitySource), RoutingError>;
}

pub struct AnonymizedAggregateServiceDir {
    /// The parent component of the collection and aggregated service.
    parent: WeakComponentInstance,

    /// The route for the service capability backed by this directory.
    route: AnonymizedServiceRoute,

    /// The provider of service capabilities for the collection being aggregated.
    ///
    /// This returns routed `CapabilitySourceInterface`s to a service capability for a
    /// component instance in the collection.
    aggregate_capability_provider: Box<dyn AnonymizedAggregateCapabilityProvider>,

    inner: Mutex<AnonymizedAggregateServiceDirInner>,
}

impl AnonymizedAggregateServiceDir {
    pub fn new(
        parent: WeakComponentInstance,
        route: AnonymizedServiceRoute,
        aggregate_capability_provider: Box<dyn AnonymizedAggregateCapabilityProvider>,
    ) -> Self {
        AnonymizedAggregateServiceDir {
            parent,
            route,
            aggregate_capability_provider,
            inner: Mutex::new(AnonymizedAggregateServiceDirInner {
                dir: simple_immutable_dir(),
                entries: HashMap::new(),
                watchers_spawned: HashMap::new(),
            }),
        }
    }

    pub fn hooks(self: &Arc<Self>) -> Vec<HooksRegistration> {
        vec![HooksRegistration::new(
            "AnonymizedAggregateServiceDir",
            vec![EventType::Started, EventType::Stopped],
            Arc::downgrade(self) as Weak<dyn Hook>,
        )]
    }

    /// Returns the backing directory that represents this service directory.
    pub fn dir_entry(&self) -> Arc<SimpleImmutableDir> {
        self.inner.lock().dir.clone()
    }

    /// Returns metadata about all the service instances in their original representation,
    /// useful for exposing debug info. The results are returned in no particular order.
    pub fn entries(&self) -> Vec<Arc<ServiceInstanceDirectoryEntry>> {
        self.inner.lock().entries.values().cloned().collect()
    }

    /// Adds directory entries from services exposed by a member of the aggregate.
    async fn add_entries_from_instance(
        self: &Arc<Self>,
        instance: &AggregateInstance,
    ) -> Result<(), ModelError> {
        let parent =
            self.parent.upgrade().map_err(|err| ModelError::ComponentInstanceError { err })?;
        let service_name = self.route.service_name.as_str();
        match self.aggregate_capability_provider.route_instance(instance).await {
            Ok((router, source)) => {
                if source.type_name() != Some(CapabilityTypeName::Service) {
                    // This isn't actually a service capability, so it's not part of the
                    // aggregate and we have nothing to do.
                    return Ok(());
                }

                // Add entries for the component `name`, from its `source`,
                // the service exposed by the component.
                // We will use this oneshot channel to know when we have reached the idle state
                // on the directory watcher that is collecting service instances.
                let (idle_sender, idle_receiver) = oneshot::channel::<()>();

                // We don't want the inner lock to be held while doing an await on the
                // idle_receiver so we do our state checking and modification while holding the
                // lock, then return if we should do the idle_receiver wait.
                let do_wait = {
                    let mut inner = self.inner.lock();
                    if let Some(existing) = inner.watchers_spawned.get(instance) {
                        // We have an existing entry. This means the watcher has already been
                        // spawned.
                        match existing {
                            WatcherEntry::WaitingForIdle(None) => {
                                // This means we did not have a idle_sender when we spanwed the watcher
                                // initially in |on_started_async|, so add one now.
                                inner.watchers_spawned.insert(
                                    instance.clone(),
                                    WatcherEntry::WaitingForIdle(Some(idle_sender)),
                                );

                                // Since we put in our idle_sender, we will want to do a wait.
                                true
                            }
                            WatcherEntry::ReachedIdle => {
                                // Since the watcher has already reached idle, don't wait.
                                false
                            }
                            WatcherEntry::WaitingForIdle(Some(_)) => {
                                // This should be impossible as there is no concurrent entry into
                                // this code for the same instance.
                                unreachable!();
                            }
                        }
                    } else {
                        // We have no existing entry, so no watcher has been spawned.
                        // We will insert the entry with our idle_sender and spawn the watcher.
                        inner.watchers_spawned.insert(
                            instance.clone(),
                            WatcherEntry::WaitingForIdle(Some(idle_sender)),
                        );
                        self.spawn_instance_watcher_task(
                            instance.clone(),
                            router.clone(),
                            source.clone(),
                        )?;
                        // Since we put in our idle_sender, we will want to do a wait.
                        true
                    }
                };

                if do_wait {
                    // Waits for the watcher to reach and idle event.
                    idle_receiver.await.map_err(|err| {
                        error!(
                            component:% = instance,
                            service_name:% = service_name,
                            error:% = err;
                            "Failed to reach idle state on the service instance directory watcher.",
                        );

                        ModelError::open_directory_error(
                            parent.moniker.clone(),
                            instance.to_string(),
                        )
                    })?;
                }
            }
            Err(err) => {
                parent
                    .log(
                        log::Level::Error,
                        "Failed to route service capability from component, skipping",
                        &[
                            &("component", format!("{instance}").as_str()),
                            &("service_name", service_name),
                            &("error", format!("{err}").as_str()),
                        ],
                    )
                    .await;
            }
        }
        Ok(())
    }

    /// Spawns a new task on the parent's nonblocking_task_group to create and run a directory
    /// watcher for the service instances for the aggregate.
    fn spawn_instance_watcher_task(
        self: &Arc<Self>,
        instance: AggregateInstance,
        router: Arc<Router<DirConnector>>,
        source: CapabilitySource,
    ) -> Result<(), ModelError> {
        let scope = &self.parent.upgrade()?.execution_scope;
        let self_clone = self.clone();
        let instance_watcher_task = async move {
            let service_name = self_clone.route.service_name.as_str();

            // The CapabilitySource must be for a service capability. This is normally detected
            // by the caller before we get here.
            if source.type_name() != Some(CapabilityTypeName::Service) {
                let type_string = source
                    .type_name()
                    .map(|t| t.to_string())
                    .unwrap_or_else(|| "<unknown>".to_string());
                error!(
                    component:% = instance,
                    service_name:% = service_name;
                    "The CapabilitySource has an invalid type: '{}'.",
                    type_string,
                );
                return;
            }

            let result = self_clone.wait_for_service_directory(&instance, &source).await;
            if let Err(err) = result {
                warn!(
                    component:% = instance,
                    service_name:% = service_name,
                    error:% = err;
                    "Failed to wait_for_service_directory.",
                );
                return;
            }

            let (proxy, watcher) =
                match self_clone.create_instance_watcher(&instance, &router).await {
                    Ok(val) => val,
                    Err(err) => {
                        error!(
                            component:% = instance,
                            service_name:% = service_name,
                            error:% = err;
                            "Failed to create_instance_watcher.",
                        );
                        return;
                    }
                };

            // This is a long running watcher that is alive until the source component
            // removes the service directory.
            self_clone.run_instance_watcher(watcher, &proxy, &instance).await;
        };

        scope.spawn(instance_watcher_task);
        Ok(())
    }

    /// Waits for the service directory to be present. This is done by recursively waiting for each
    /// directory in the source_path. This is a no-op if the source is not a component type.
    async fn wait_for_service_directory(
        &self,
        instance: &AggregateInstance,
        source: &CapabilitySource,
    ) -> Result<(), ModelError> {
        match source {
            CapabilitySource::Component(ComponentSource { capability, moniker }) => {
                let target = self
                    .parent
                    .upgrade()
                    .map_err(|err| ModelError::ComponentInstanceError { err })?;
                let source_component = target.find_absolute(moniker).await?;

                let mut cur_path = RelativePath::dot();
                let mut path_buf = ReusablePathBuffer::new(&cur_path);

                for segment in capability.source_path().unwrap().iter_segments() {
                    let (proxy, server_end) =
                        fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
                    let mut object_request = FLAGS.to_object_request(server_end);
                    source_component
                        .open_outgoing(OpenRequest::new(
                            source_component.execution_scope.clone(),
                            FLAGS,
                            path_buf.as_str().try_into().map_err(|_| ModelError::BadPath)?,
                            &mut object_request,
                        ))
                        .await?;
                    let watcher =
                        fuchsia_fs::directory::Watcher::new(&proxy).await.map_err(|err| {
                            error!(
                                component:% = instance,
                                service_name:% = self.route.service_name,
                                error:% = err;
                                "Failed to get the outgoing watcher for the path '{}'.",
                                cur_path
                            );
                            ModelError::open_directory_error(
                                target.moniker.clone(),
                                instance.to_string(),
                            )
                        })?;

                    enum StreamErrorType {
                        Found,
                        StreamError(fuchsia_fs::directory::WatcherStreamError),
                        Exit,
                    }

                    let result = watcher
                        .map_err(|e| StreamErrorType::StreamError(e))
                        .try_for_each(|entry| async move {
                            let mut inner = self.inner.lock();
                            if !inner.watchers_spawned.contains_key(&instance) {
                                // Our task entry doesn't exist, it is removed in
                                // |on_stopped_async|, so we can exit early.
                                return Err(StreamErrorType::Exit);
                            }

                            match entry.event {
                                fuchsia_fs::directory::WatchEvent::ADD_FILE
                                | fuchsia_fs::directory::WatchEvent::EXISTING => {
                                    let filename = extract_event_filename(entry.filename.as_path())
                                        .expect("filename must be valid utf8");
                                    if filename.as_ref() != segment.as_ref() {
                                        return Ok(());
                                    }

                                    // Use error to terminate the try_for_each and move on to the
                                    // next piece in the path.
                                    return Err(StreamErrorType::Found);
                                }
                                fuchsia_fs::directory::WatchEvent::IDLE => {
                                    // We've hit an idle, so if anyone is waiting on us for routing
                                    // unblock them.
                                    inner
                                        .watchers_spawned
                                        .get_mut(&instance)
                                        .unwrap()
                                        .transition_to_idle();
                                    Ok(())
                                }
                                _ => Ok(()),
                            }
                        })
                        .await;

                    match result {
                        Err(StreamErrorType::Found) => {
                            if !cur_path.push(segment.into()) {
                                return Err(RoutingError::PathTooLong {
                                    moniker: moniker.clone().into(),
                                    path: format!("{cur_path}/{segment}"),
                                    keyword: "source_path from service watcher (this should be impossible)".into(),
                                }.into());
                            }

                            path_buf.push_segment(segment.as_str());
                            continue;
                        }
                        Err(StreamErrorType::StreamError(err)) => {
                            warn!(
                                component:% = instance,
                                service_name:% = self.route.service_name,
                                error:% = err;
                                "Watcher in wait_for_service_directory ran into read error."
                            );
                        }
                        Err(StreamErrorType::Exit) => {
                            // Early exit, no log needed.
                        }
                        Ok(()) => {
                            error!(
                                component:% = instance,
                                service_name:% = self.route.service_name;
                                "Watcher in wait_for_service_directory did not find the path piece before completing.",
                            );
                        }
                    };

                    return Err(ModelError::open_directory_error(
                        target.moniker.clone(),
                        instance.to_string(),
                    ));
                }
            }
            // NOTE: If `source` is `AnonymizedAggregate`, the service
            // directory must already exist, so there is no need to watch
            _ => {}
        };
        Ok(())
    }

    /// Opens the service capability using the `router` and creates a directory_watcher on it.
    ///
    /// # Errors
    /// Returns an error if `source` is not a service capability, or could not be opened.
    async fn create_instance_watcher(
        &self,
        instance: &AggregateInstance,
        router: &Router<DirConnector>,
    ) -> Result<(fio::DirectoryProxy, fuchsia_fs::directory::Watcher), ModelError> {
        let target =
            self.parent.upgrade().map_err(|err| ModelError::ComponentInstanceError { err })?;

        let (proxy, server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let result =
            router.route(fruntime::RouteRequest::default(), self.parent.clone().into()).await;
        let dir_connector = match result? {
            Some(dir_connector) => dir_connector,
            None => {
                return Err(RoutingError::RouteUnexpectedUnavailable {
                    type_name: CapabilityTypeName::Service,
                    moniker: target.moniker.clone().into(),
                }
                .into());
            }
        };
        let _ = dir_connector.send(server_end, RelativePath::dot(), Some(FLAGS));

        let watcher = fuchsia_fs::directory::Watcher::new(&proxy)
            .on_timeout(OPEN_SERVICE_TIMEOUT.after_now(), || {
                Err(WatcherCreateError::WatchError(zx::Status::TIMED_OUT))
            })
            .await
            .map_err(|err| {
                error!(
                    component:% = instance,
                    service_name:% = self.route.service_name,
                    error:% = err;
                    "Failed to create service instance directory watcher.",
                );
                ModelError::open_directory_error(target.moniker.clone(), instance.to_string())
            })?;
        Ok((proxy, watcher))
    }

    /// Runs the directory watcher on the service directory. This will discover additions and
    /// removals of service instances through the various events. For new and existing events,
    /// an entry will be added, and for removed events the entry is removed.
    async fn run_instance_watcher(
        &self,
        watcher: fuchsia_fs::directory::Watcher,
        proxy: &fio::DirectoryProxy,
        instance: &AggregateInstance,
    ) -> () {
        let result = watcher
            .map_err(|e| Some(e))
            .try_for_each(|message| async move {
                let filename = extract_event_filename(message.filename.as_path())
                    .expect("filename must be valid utf8");

                let mut inner = self.inner.lock();
                if !inner.watchers_spawned.contains_key(&instance) {
                    // Our task entry doesn't exist, it is removed in |on_stopped_async|,
                    // so we can exit early.
                    return Err(None);
                }

                if message.event == fuchsia_fs::directory::WatchEvent::DELETED {
                    // Our directory was deleted, so we can exit early.
                    return Err(None);
                }

                if filename == "." {
                    // Ignore the "." file.
                    return Ok::<(), Option<fuchsia_fs::directory::WatcherStreamError>>(());
                }

                match message.event {
                    fuchsia_fs::directory::WatchEvent::ADD_FILE
                    | fuchsia_fs::directory::WatchEvent::EXISTING => {
                        let instance_key = ServiceInstanceDirectoryKey::<AggregateInstance> {
                            source_id: instance.clone(),
                            service_instance: Name::new(filename.as_ref()).unwrap(),
                        };

                        // Check for duplicate entries.
                        if inner.entries.contains_key(&instance_key) {
                            return Ok(());
                        }
                        let name = generate_hex_name(&mut rand::rng());

                        let Ok(child_proxy) = fuchsia_fs::directory::open_directory_async(
                            proxy,
                            filename.as_ref(),
                            fio::PERM_READABLE,
                        ) else {
                            // Our connection to the directory has been broken, so we can exit.
                            return Err(None);
                        };
                        let entry = Arc::new(ServiceInstanceDirectoryEntry {
                            name: name.clone(),
                            directory_proxy: child_proxy,
                            source_id: instance.clone(),
                            service_instance: instance_key.service_instance.clone().into(),
                        });
                        let result = inner.dir.add_node(&name.as_str(), entry.clone());
                        if let Err(err) = result {
                            error!(
                                component:% = instance,
                                service_name:% = self.route.service_name,
                                error:% = err;
                                "Failed to add node to inner directory.",
                            );
                        }
                        inner.entries.insert(instance_key, entry);
                        Ok(())
                    }
                    fuchsia_fs::directory::WatchEvent::REMOVE_FILE => {
                        let instance_key = ServiceInstanceDirectoryKey::<AggregateInstance> {
                            source_id: instance.clone(),
                            service_instance: Name::new(&filename).unwrap(),
                        };

                        let removed_entry = inner.entries.remove(&instance_key);
                        match removed_entry {
                            Some(removed_entry) => {
                                let result = inner.dir.remove_node(removed_entry.name.as_str());
                                if let Err(err) = result {
                                    error!(
                                        component:% = instance,
                                        service_name:% = self.route.service_name,
                                        error:% = err;
                                        "Failed to remove node from inner directory.",
                                    );
                                }
                            }
                            None => {}
                        };

                        Ok(())
                    }
                    fuchsia_fs::directory::WatchEvent::IDLE => {
                        inner.watchers_spawned.get_mut(&instance).unwrap().transition_to_idle();
                        Ok(())
                    }
                    fuchsia_fs::directory::WatchEvent::DELETED => unreachable!(),
                }
            })
            .await;
        if let Err(Some(err)) = result {
            let fuchsia_fs::directory::WatcherStreamError::ChannelRead(status) = err;
            if status != zx::Status::PEER_CLOSED {
                error!(
                    component:% = instance,
                    service_name:% = self.route.service_name;
                    "Instance watcher stream closed with error {:?}.", status
                );
            }
        }
    }

    /// Adds directory entries from services exposed by all children in the aggregated collection.
    pub fn add_entries_from_children<'a>(
        self: &'a Arc<Self>,
    ) -> BoxFuture<'a, Result<(), ModelError>> {
        // Return a boxed future here because this function can be called from routing::get_default_provider
        // which creates a recursive loop when initializing the capability provider for collection sourced
        // services.
        Box::pin(async move {
            let instances = self.aggregate_capability_provider.list_instances().await?;
            // Remove duplicates
            let instances: HashSet<_> = instances.into_iter().collect();
            join_all(instances.iter().map(
                |instance| async move {
                    self.add_entries_from_instance(&instance).await.map_err(|e| {
                        error!(error:% = e, instance:% = instance; "error adding entries from instance");
                        e
                    })
                },
            ))
            .await;
            Ok(())
        })
    }

    async fn on_started_async(
        self: Arc<Self>,
        component_moniker: &Moniker,
    ) -> Result<(), ModelError> {
        // If this component is a child in a collection from which the aggregated service
        // is routed, add service instances from the component's service to the aggregated service.
        if !self.route.matches_child_component(component_moniker) {
            return Ok(());
        }

        // Check that the component exposes the service bound to this route.
        let Ok(parent) = self.parent.upgrade() else {
            return Ok(());
        };
        let child_name = component_moniker.leaf().expect("checked in matches_child_component");
        let component = {
            let guard = parent.lock_state().await;
            let resolved = match &*guard {
                InstanceState::Resolved(r) => r,
                InstanceState::Started(r, _) => r,
                InstanceState::Unresolved(_)
                | InstanceState::Shutdown(_, _)
                | InstanceState::Destroyed => {
                    return Ok(());
                }
            };
            let Some(component) = resolved.get_child(child_name) else {
                return Ok(());
            };
            component.clone()
        };
        {
            let guard = component.lock_state().await;
            let resolved = match &*guard {
                InstanceState::Resolved(r) => r,
                InstanceState::Started(r, _) => r,
                InstanceState::Unresolved(_)
                | InstanceState::Shutdown(_, _)
                | InstanceState::Destroyed => {
                    return Ok(());
                }
            };
            let to_parent = resolved.sandbox.component_output.capabilities();
            if let Some(_) = to_parent.get(&self.route.service_name) {
                // Proceed.
            } else {
                // This component doesn't expose the capability we're matching on, nothing
                // to do.
                return Ok(());
            }
        }

        let instance = AggregateInstance::Child(child_name.into());
        let (router, capability_source) =
            self.aggregate_capability_provider.route_instance(&instance).await?;
        if capability_source.type_name() != Some(CapabilityTypeName::Service) {
            // This isn't actually a service capability, so it's not part of the
            // aggregate and there is nothing to do.
            return Ok(());
        }

        // If we have not already spawned a watcher task we want to do that here.
        let mut inner = self.inner.lock();
        if !inner.watchers_spawned.contains_key(&instance) {
            // We have not spawned the watcher, so we create the entry with a None oneshot
            // sender that will eventually be replaced with one if necessary from the
            // |add_entries_from_instance| method.
            inner.watchers_spawned.insert(instance.clone(), WatcherEntry::WaitingForIdle(None));

            // Spawn the watcher.
            self.spawn_instance_watcher_task(instance, router, capability_source)?;
        }
        Ok(())
    }

    async fn on_stopped_async(&self, target_moniker: &Moniker) -> Result<(), ModelError> {
        // If this component is a child in a collection from which the aggregated service
        // is routed, remove any of its service instances from the aggregated service.
        if self.route.matches_child_component(target_moniker) {
            let target_child_moniker = target_moniker.leaf().expect("root is impossible");
            let mut inner = self.inner.lock();
            for entry in inner.entries.values() {
                if matches!(&entry.source_id, AggregateInstance::Child(n) if n == target_child_moniker)
                {
                    inner.dir.remove_node(entry.name.as_str()).map_err(|err| {
                        ModelError::ServiceDirError { moniker: target_moniker.clone(), err }
                    })?;
                }
            }
            inner.entries.retain(|key, _| !matches!(&key.source_id, AggregateInstance::Child(n) if n == target_child_moniker));
            inner.watchers_spawned.remove(&AggregateInstance::Child(target_child_moniker.into()));
        }
        Ok(())
    }
}

#[async_trait]
impl Hook for AnonymizedAggregateServiceDir {
    async fn on(self: Arc<Self>, event: &Event) -> Result<(), ModelError> {
        match &event.payload {
            EventPayload::Started { .. } => {
                if let ExtendedMoniker::ComponentInstance(component_moniker) = &event.target_moniker
                {
                    self.on_started_async(&component_moniker).await?;
                }
            }
            EventPayload::Stopped { .. } => {
                let target_moniker = event
                    .target_moniker
                    .unwrap_instance_moniker_or(ModelError::UnexpectedComponentManagerMoniker)?;
                self.on_stopped_async(target_moniker).await?;
            }
            _ => {}
        }
        Ok(())
    }
}

/// A directory entry representing an instance of a service.
/// Upon opening, performs capability routing and opens the instance at its source.
pub struct ServiceInstanceDirectoryEntry {
    /// The name of this directory entry, when added as a node into another directory.
    pub name: Name,

    directory_proxy: fio::DirectoryProxy,

    /// An identifier that can be used to find the child component that serves the service
    /// instance.
    /// This is a generic type because it varies between aggregated directory types. For example,
    /// for aggregated offers this an instance in the source instance filter,
    /// while for aggregated collections it is the moniker of the source child.
    // TODO(https://fxbug.dev/294909269): AnonymizedAggregateServiceDir needs this, but
    // FilteredAggregateServiceDir only uses this for debug info. We could probably have
    // AnonymizedAggregateServiceDir use ServiceInstanceDirectoryKey.source_id instead, and either
    // delete this or make it debug-only.
    pub source_id: AggregateInstance,

    /// The name of the service instance directory to open at the source.
    pub service_instance: FlyStr,
}

/// A key that uniquely identifies a ServiceInstanceDirectoryEntry.
#[derive(Hash, PartialEq, Eq)]
struct ServiceInstanceDirectoryKey<T: Send + Sync + 'static + fmt::Display> {
    /// An identifier that can be used to find the child component that serves the service
    /// instance.
    /// This is a generic type because it varies between aggregated directory types. For example,
    /// for aggregated offers this an instance in the source instance filter,
    /// while for aggregated collections it is the moniker of the source child.
    pub source_id: T,

    /// The name of the service instance directory to open at the source.
    pub service_instance: Name,
}

impl DirectoryEntry for ServiceInstanceDirectoryEntry {
    fn open_entry(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), zx::Status> {
        request.spawn(self);
        Ok(())
    }
}

impl GetEntryInfo for ServiceInstanceDirectoryEntry {
    fn entry_info(&self) -> EntryInfo {
        EntryInfo::new(fio::INO_UNKNOWN, fio::DirentType::Directory)
    }
}

impl DirectoryEntryAsync for ServiceInstanceDirectoryEntry {
    async fn open_entry_async(self: Arc<Self>, request: OpenRequest<'_>) -> Result<(), zx::Status> {
        request.open_remote(vfs::remote::remote_dir(Clone::clone(&self.directory_proxy)))
    }
}

/// Trait that allows directory to be mutated.
pub trait MutableDirectory {
    /// Adds an entry to a directory.
    fn add_node(&self, name: &str, entry: Arc<dyn DirectoryEntry>) -> Result<(), VfsError>;

    /// Removes an entry in the directory.
    ///
    /// Returns the entry to the caller if the entry was present in the directory.
    fn remove_node(&self, name: &str) -> Result<Option<Arc<dyn DirectoryEntry>>, VfsError>;
}

impl MutableDirectory for Arc<SimpleImmutableDir> {
    fn add_node(&self, name: &str, entry: Arc<dyn DirectoryEntry>) -> Result<(), VfsError> {
        self.add_entry(name, entry)
            .map_err(|status| VfsError::AddNodeError { name: name.to_string(), status })
    }

    fn remove_node(&self, name: &str) -> Result<Option<Arc<dyn DirectoryEntry>>, VfsError> {
        self.remove_entry(name, false)
            .map_err(|status| VfsError::RemoveNodeError { name: name.to_string(), status })
    }
}

#[cfg(all(test, not(feature = "src_model_tests")))]
mod tests {
    use super::*;
    use crate::model::component::StartReason;
    use crate::model::routing::service::AnonymizedAggregateServiceDir;
    use crate::model::start::Start;
    use crate::model::testing::out_dir::OutDir;
    use crate::model::testing::routing_test_helpers::{RoutingTest, RoutingTestBuilder};
    use ::routing::bedrock::request_metadata::service_metadata;
    use ::routing::component_instance::ComponentInstanceInterface;
    use ::routing::error::{PrettyPrintRef, RouteVerb, RoutingError};
    use ::routing::intermediate_router::{IntermediateRouter, RouteRequest};
    use capability_source::ComponentCapability;
    use cm_rust::offer::*;
    use cm_rust::*;
    use cm_rust_testing::*;
    use fidl_fuchsia_component_decl as fdecl;
    use maplit::hashmap;
    use proptest::prelude::*;
    use rand::SeedableRng as _;
    use router_error::RouterError;
    use runtime_capabilities::{Routable, WeakInstanceToken};
    use std::collections::HashSet;
    use vfs::{ExecutionScope, pseudo_directory};

    #[derive(Clone)]
    struct MockAnonymizedCapabilityProvider {
        /// Use an Arc<Mutex> for the instances so that we can mutate it after the Aggregate
        /// directory has been created in the test.
        instances: Arc<Mutex<HashMap<AggregateInstance, WeakComponentInstance>>>,
    }

    #[async_trait]
    impl AnonymizedAggregateCapabilityProvider for MockAnonymizedCapabilityProvider {
        async fn route_instance(
            &self,
            instance: &AggregateInstance,
        ) -> Result<(Arc<Router<DirConnector>>, CapabilitySource), RoutingError> {
            let instances_guard = self.instances.lock();
            let Some(component_instance) = instances_guard.get(instance) else {
                let err = match instance {
                    AggregateInstance::Parent => RoutingError::RouteSourceNotFound {
                        moniker: Moniker::root(),
                        verb: RouteVerb::Offer,
                        counter_verb: RouteVerb::Offer,
                        source: PrettyPrintRef::Parent,
                        capability_type: CapabilityTypeName::Service,
                        capability_name: RelativePath::new("my.service.Service").unwrap(),
                    },
                    AggregateInstance::Child(instance) => RoutingError::RouteSourceNotFound {
                        moniker: Moniker::root(),
                        verb: RouteVerb::Offer,
                        counter_verb: RouteVerb::Expose,
                        source: instance.clone().into(),
                        capability_type: CapabilityTypeName::Service,
                        capability_name: RelativePath::new("my.service.Service").unwrap(),
                    },
                    AggregateInstance::Self_ => {
                        panic!("not expected");
                    }
                };
                return Err(err.into());
            };
            let source = CapabilitySource::Component(ComponentSource {
                capability: ComponentCapability::Service(ServiceDecl {
                    name: "my.service.Service".parse().unwrap(),
                    source_path: Some("/svc/my.service.Service".parse().unwrap()),
                }),
                moniker: component_instance.moniker.clone(),
            });
            struct TestRouter {
                weak_component: WeakComponentInstance,
            }
            impl TestRouter {
                async fn get_service_router(
                    &self,
                ) -> Result<Arc<Router<DirConnector>>, RoutingError> {
                    let component = self.weak_component.upgrade().map_err(RoutingError::from)?;
                    let program_output_dict = component
                        .lock_resolved_state()
                        .await
                        .expect("failed to resolve component")
                        .sandbox
                        .program_output_dict
                        .clone();
                    Ok(IntermediateRouter::new(
                        Arc::downgrade(&program_output_dict).into(),
                        RelativePath::new("my.service.Service").unwrap(),
                        RouteRequest {
                            build_type_name: CapabilityTypeName::Service,
                            ..Default::default()
                        },
                        self.weak_component.clone().into(),
                        self.weak_component.moniker.clone(),
                        RouteVerb::Declare,
                        fdecl::Ref::Self_(fdecl::SelfRef {}),
                    )
                    .try_into()
                    .expect("wrong type from intermediate router"))
                }
            }
            #[async_trait]
            impl Routable<DirConnector> for TestRouter {
                async fn route(
                    &self,
                    _request: fruntime::RouteRequest,
                    target: Arc<WeakInstanceToken>,
                ) -> Result<Option<Arc<DirConnector>>, RouterError> {
                    let service_router = self.get_service_router().await?;
                    let request = service_metadata(cm_types::Availability::Required);
                    service_router.route(request, target).await
                }

                async fn route_debug(
                    &self,
                    _request: fruntime::RouteRequest,
                    _target: Arc<WeakInstanceToken>,
                ) -> Result<CapabilitySource, RouterError> {
                    panic!("debug routing not expected");
                }
            }
            Ok((Router::new(TestRouter { weak_component: component_instance.clone() }), source))
        }

        async fn list_instances(&self) -> Result<Vec<AggregateInstance>, RoutingError> {
            Ok(self.instances.lock().keys().cloned().collect())
        }
    }

    fn create_test_component_decls() -> Vec<(&'static str, ComponentDecl)> {
        let leaf_component_decl = ComponentDeclBuilder::new()
            .expose(ExposeBuilder::service().name("my.service.Service").source(ExposeSource::Self_))
            .service_default("my.service.Service")
            .build();
        vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .use_(
                        UseBuilder::protocol()
                            .source(UseSource::Framework)
                            .name("fuchsia.component.Realm"),
                    )
                    .expose(
                        ExposeBuilder::service()
                            .name("my.service.Service")
                            .source(ExposeSource::Collection("coll1".parse().unwrap())),
                    )
                    .expose(
                        ExposeBuilder::service()
                            .name("my.service.Service")
                            .source(ExposeSource::Collection("coll2".parse().unwrap())),
                    )
                    .expose(
                        ExposeBuilder::service()
                            .name("my.service.Service")
                            .source_static_child("static_a"),
                    )
                    .expose(
                        ExposeBuilder::service()
                            .name("my.service.Service")
                            .source_static_child("static_b"),
                    )
                    .collection(CollectionBuilder::new().name("coll1"))
                    .collection(CollectionBuilder::new().name("coll2"))
                    .child_default("static_a")
                    .child_default("static_b")
                    // This child is not included in the aggregate.
                    .child_default("static_c")
                    .build(),
            ),
            ("foo", leaf_component_decl.clone()),
            ("bar", leaf_component_decl.clone()),
            ("baz", leaf_component_decl.clone()),
            ("static_a", leaf_component_decl.clone()),
            ("static_b", leaf_component_decl.clone()),
            ("static_c", leaf_component_decl.clone()),
        ]
    }

    async fn wait_for_dir_content_change(
        dir_proxy: &fio::DirectoryProxy,
        original_entries: Vec<fuchsia_fs::directory::DirEntry>,
    ) -> Vec<fuchsia_fs::directory::DirEntry> {
        loop {
            // TODO(https://fxbug.dev/294909269): Now that component manager supports watching for
            // service instances, this loop should be replaced by a watcher.
            let updated_entries = fuchsia_fs::directory::readdir(dir_proxy)
                .await
                .expect("failed to read directory entries");
            if original_entries.len() != updated_entries.len() {
                return updated_entries;
            }
        }
    }

    async fn create_anonymized_service_test_realm(
        init_service_dir: bool,
    ) -> (RoutingTest, Arc<AnonymizedAggregateServiceDir>) {
        let components = create_test_component_decls();

        let mock_single_instance = pseudo_directory! {
            "default" => pseudo_directory! {
                "member" => pseudo_directory! {}
            }
        };
        let mock_dual_instance = pseudo_directory! {
            "default" => pseudo_directory! {
                "member" => pseudo_directory! {}
            },
            "secondary" => pseudo_directory! {
                "member" => pseudo_directory! {},
            }
        };

        let test = RoutingTestBuilder::new("root", components)
            .add_outgoing_path(
                "foo",
                "/svc/my.service.Service".parse().unwrap(),
                mock_single_instance.clone(),
            )
            .add_outgoing_path(
                "bar",
                "/svc/my.service.Service".parse().unwrap(),
                mock_single_instance.clone(),
            )
            .add_outgoing_path(
                "baz",
                "/svc/my.service.Service".parse().unwrap(),
                mock_dual_instance,
            )
            .add_outgoing_path(
                "static_a",
                "/svc/my.service.Service".parse().unwrap(),
                mock_single_instance.clone(),
            )
            .add_outgoing_path(
                "static_b",
                "/svc/my.service.Service".parse().unwrap(),
                mock_single_instance.clone(),
            )
            .add_outgoing_path(
                "static_c",
                "/svc/my.service.Service".parse().unwrap(),
                mock_single_instance,
            )
            .build()
            .await;

        test.create_dynamic_child(&Moniker::root(), "coll1", ChildBuilder::new().name("foo")).await;
        test.create_dynamic_child(&Moniker::root(), "coll1", ChildBuilder::new().name("bar")).await;
        test.create_dynamic_child(&Moniker::root(), "coll2", ChildBuilder::new().name("baz")).await;
        let root = test.model.root();
        let foo_component =
            root.find_and_maybe_resolve(&"coll1:foo".parse().unwrap()).await.unwrap();
        let bar_component =
            root.find_and_maybe_resolve(&"coll1:bar".parse().unwrap()).await.unwrap();
        let baz_component =
            root.find_and_maybe_resolve(&"coll2:baz".parse().unwrap()).await.unwrap();
        let static_a_component =
            root.find_and_maybe_resolve(&"static_a".parse().unwrap()).await.unwrap();
        let static_b_component =
            root.find_and_maybe_resolve(&"static_b".parse().unwrap()).await.unwrap();

        let provider = MockAnonymizedCapabilityProvider {
            instances: Arc::new(Mutex::new(hashmap! {
                AggregateInstance::Child("coll1:foo".try_into().unwrap()) => foo_component.as_weak(),
                AggregateInstance::Child("coll1:bar".try_into().unwrap()) => bar_component.as_weak(),
                AggregateInstance::Child("coll2:baz".try_into().unwrap()) => baz_component.as_weak(),
                AggregateInstance::Child("static_a".try_into().unwrap()) => static_a_component.as_weak(),
                AggregateInstance::Child("static_b".try_into().unwrap()) => static_b_component.as_weak(),
            })),
        };

        let route = AnonymizedServiceRoute {
            source_moniker: Moniker::root(),
            members: vec![
                AggregateMember::Collection("coll1".parse().unwrap()),
                AggregateMember::Collection("coll2".parse().unwrap()),
                AggregateMember::Child(ChildRef {
                    name: "static_a".parse().unwrap(),
                    collection: None,
                }),
                AggregateMember::Child(ChildRef {
                    name: "static_b".parse().unwrap(),
                    collection: None,
                }),
            ],
            service_name: "my.service.Service".parse().unwrap(),
        };

        let dir =
            Arc::new(AnonymizedAggregateServiceDir::new(root.as_weak(), route, Box::new(provider)));

        if init_service_dir {
            dir.add_entries_from_children().await.expect("failed to add entries");
        }

        root.hooks.install(dir.hooks());
        (test, dir)
    }

    #[fuchsia::test]
    async fn test_anonymized_service_directory() {
        let (test, dir_arc) = create_anonymized_service_test_realm(true).await;
        let dir_proxy = vfs::directory::serve_read_only(dir_arc.dir_entry(), ExecutionScope::new());

        // List the entries of the directory served by `open`, and compare them to the
        // internal state.
        let instance_names = {
            let instance_names: HashSet<_> =
                dir_arc.entries().into_iter().map(|e| e.name.to_string()).collect();
            let dir_contents = fuchsia_fs::directory::readdir(&dir_proxy)
                .await
                .expect("failed to read directory entries");
            let dir_instance_names: HashSet<_> = dir_contents.into_iter().map(|d| d.name).collect();
            assert_eq!(instance_names.len(), 6);
            assert_eq!(dir_instance_names, instance_names);
            instance_names
        };

        // Open one of the entries.
        {
            let instance_dir = fuchsia_fs::directory::open_directory(
                &dir_proxy,
                instance_names.iter().next().expect("failed to get instance name"),
                fuchsia_fs::PERM_READABLE,
            )
            .await
            .expect("failed to open collection dir");

            // Make sure we're reading the expected directory.
            let instance_dir_contents = fuchsia_fs::directory::readdir(&instance_dir)
                .await
                .expect("failed to read instances of collection dir");
            assert!(instance_dir_contents.iter().find(|d| d.name == "member").is_some());
        }

        let root = test.model.root();
        let baz_component =
            root.find_and_maybe_resolve(&["coll2:baz"].try_into().unwrap()).await.unwrap();
        let static_a_component =
            root.find_and_maybe_resolve(&["static_a"].try_into().unwrap()).await.unwrap();

        // Add entries from the children again. This should be a no-op since all of them are
        // already there and we prevent duplicates.
        let dir_contents = {
            let previous_entries: HashSet<_> = dir_arc
                .entries()
                .into_iter()
                .map(|e| (e.name.clone(), e.source_id.clone()))
                .collect();
            dir_arc.add_entries_from_children().await.unwrap();
            let entries: HashSet<_> = dir_arc
                .entries()
                .into_iter()
                .map(|e| (e.name.clone(), e.source_id.clone()))
                .collect();
            assert_eq!(entries, previous_entries);
            let dir_contents = fuchsia_fs::directory::readdir(&dir_proxy)
                .await
                .expect("failed to read directory entries");
            let dir_instance_names: HashSet<_> =
                dir_contents.iter().map(|d| d.name.clone()).collect();
            assert_eq!(dir_instance_names, instance_names);
            dir_contents
        };

        // Test that removal of instances works (both dynamic and static).
        {
            baz_component.stop().await.unwrap();
            let dir_contents = wait_for_dir_content_change(&dir_proxy, dir_contents).await;
            assert_eq!(dir_contents.len(), 4);

            static_a_component.stop().await.unwrap();
            let dir_contents = wait_for_dir_content_change(&dir_proxy, dir_contents).await;
            assert_eq!(dir_contents.len(), 3);

            test.start_instance_and_wait_start(static_a_component.moniker()).await.unwrap();
            let dir_contents = wait_for_dir_content_change(&dir_proxy, dir_contents).await;
            assert_eq!(dir_contents.len(), 4);

            test.start_instance_and_wait_start(baz_component.moniker()).await.unwrap();
            let dir_contents = wait_for_dir_content_change(&dir_proxy, dir_contents).await;
            assert_eq!(dir_contents.len(), 6);
        }
    }

    #[fuchsia::test]
    async fn test_anonymized_service_directory_with_dynamic_instances() {
        let components = create_test_component_decls();

        let mut foo_out_dir = OutDir::new();
        let mut bar_out_dir = OutDir::new();
        let mut static_a_out_dir = OutDir::new();

        // Start out "foo" with no entries in the svc directory.
        let foo_svc = pseudo_directory! {};
        foo_out_dir.add_entry("/svc".parse().unwrap(), foo_svc.clone());

        // Start out "bar" with 1 service instance in the svc directory.
        let bar_service = pseudo_directory! {
            "default" => pseudo_directory! {
                "member" => pseudo_directory! {}
            }
        };
        let bar_svc = pseudo_directory! {
            "my.service.Service" => bar_service.clone(),
        };
        bar_out_dir.add_entry("/svc".parse().unwrap(), bar_svc);

        // Start out "static_a" with no entries in the svc directory.
        let static_a_svc = pseudo_directory! {};
        static_a_out_dir.add_entry("/svc".parse().unwrap(), static_a_svc.clone());

        let test = RoutingTestBuilder::new("root", components)
            .set_component_outgoing_host_fn("foo", foo_out_dir.host_fn())
            .set_component_outgoing_host_fn("bar", bar_out_dir.host_fn())
            .set_component_outgoing_host_fn("static_a", static_a_out_dir.host_fn())
            .build()
            .await;

        test.create_dynamic_child(&Moniker::root(), "coll1", ChildBuilder::new().name("foo")).await;
        test.create_dynamic_child(&Moniker::root(), "coll1", ChildBuilder::new().name("bar")).await;
        let root = test.model.root();
        let foo_component =
            root.find_and_maybe_resolve(&"coll1:foo".parse().unwrap()).await.unwrap();
        let bar_component =
            root.find_and_maybe_resolve(&"coll1:bar".parse().unwrap()).await.unwrap();
        let static_a_component =
            root.find_and_maybe_resolve(&"static_a".parse().unwrap()).await.unwrap();

        let provider = MockAnonymizedCapabilityProvider {
            instances: Arc::new(Mutex::new(hashmap! {
                AggregateInstance::Child("coll1:foo".try_into().unwrap()) => foo_component.as_weak(),
                AggregateInstance::Child("coll1:bar".try_into().unwrap()) => bar_component.as_weak(),
                AggregateInstance::Child("static_a".try_into().unwrap()) => static_a_component.as_weak(),
            })),
        };

        let route = AnonymizedServiceRoute {
            source_moniker: Moniker::root(),
            members: vec![
                AggregateMember::Collection("coll1".parse().unwrap()),
                AggregateMember::Collection("coll2".parse().unwrap()),
                AggregateMember::Child(ChildRef {
                    name: "static_a".parse().unwrap(),
                    collection: None,
                }),
            ],
            service_name: "my.service.Service".parse().unwrap(),
        };

        let dir_arc = Arc::new(AnonymizedAggregateServiceDir::new(
            root.as_weak(),
            route,
            Box::new(provider.clone()),
        ));
        root.hooks.install(dir_arc.hooks());
        dir_arc.add_entries_from_children().await.unwrap();

        let dir_proxy = vfs::directory::serve_read_only(dir_arc.dir_entry(), ExecutionScope::new());

        // Ensure the instance we had initially in "bar" is there.
        let entries = dir_arc.entries().len();
        assert_eq!(entries, 1);

        let dir_contents = fuchsia_fs::directory::readdir(&dir_proxy)
            .await
            .expect("failed to read directory entries");
        let previous_entries = entries;

        // Add 1 instance to "foo" and ensure we can get it.
        let foo_service = pseudo_directory! {
            "default" => pseudo_directory! {
                "member" => pseudo_directory! {},
            },
        };
        foo_svc
            .add_node("my.service.Service", foo_service.clone())
            .expect("Could not add service node.");
        let dir_contents = wait_for_dir_content_change(&dir_proxy, dir_contents).await;
        let entries = dir_arc.entries().len();
        assert_eq!(entries, previous_entries + 1);
        let previous_entries = entries;

        // Add another instance (total of 2) to "foo" and ensure we can get it.
        foo_service
            .add_node(
                "secondary",
                pseudo_directory! {
                    "member" => pseudo_directory! {},
                },
            )
            .expect("Could not add service node.");
        let dir_contents = wait_for_dir_content_change(&dir_proxy, dir_contents).await;
        let entries = dir_arc.entries().len();
        assert_eq!(entries, previous_entries + 1);
        let previous_entries = entries;

        // Add another instance to "bar", which had 1 instance previously and ensure we can get it.
        bar_service
            .add_node(
                "secondary",
                pseudo_directory! {
                    "member" => pseudo_directory! {},
                },
            )
            .expect("Could not add service node.");
        let dir_contents = wait_for_dir_content_change(&dir_proxy, dir_contents).await;
        let entries = dir_arc.entries().len();
        assert_eq!(entries, previous_entries + 1);
        let previous_entries = entries;

        // Add 2 instances to the "static_a" and ensure we can get it.
        static_a_svc
            .add_node(
                "my.service.Service",
                pseudo_directory! {
                    "default" => pseudo_directory! {
                        "member" => pseudo_directory! {},
                    },
                    "secondary" => pseudo_directory! {
                        "member" => pseudo_directory! {},
                    },
                },
            )
            .expect("Could not add service node.");
        let dir_contents = wait_for_dir_content_change(&dir_proxy, dir_contents).await;
        if dir_contents.len() == previous_entries + 1 {
            // in case we caught the change before both were seen.
            wait_for_dir_content_change(&dir_proxy, dir_contents).await;
        }
        let entries = dir_arc.entries().len();
        assert_eq!(entries, previous_entries + 2);

        // Read the directory for final check.
        // 2 in each of the 3 components.
        let dir_contents = fuchsia_fs::directory::readdir(&dir_proxy)
            .await
            .expect("failed to read directory entries");
        assert_eq!(dir_contents.len(), 6);
        for entry in &dir_contents {
            let instance_dir = fuchsia_fs::directory::open_directory(
                &dir_proxy,
                &entry.name,
                fuchsia_fs::PERM_READABLE,
            )
            .await
            .expect("failed to open collection dir");

            // Make sure we're reading the expected directory.
            let instance_dir_contents = fuchsia_fs::directory::readdir(&instance_dir)
                .await
                .expect("failed to read instances of collection dir");
            assert!(instance_dir_contents.iter().find(|d| d.name == "member").is_some());
        }

        // Remove some entries to make sure removal flow works.
        bar_service.remove_node("default").expect("Failed to remove default from bar.");
        let dir_contents = wait_for_dir_content_change(&dir_proxy, dir_contents).await;
        assert_eq!(dir_contents.len(), 5);

        bar_service.remove_node("secondary").expect("Failed to remove secondary from bar.");
        let dir_contents = wait_for_dir_content_change(&dir_proxy, dir_contents).await;
        assert_eq!(dir_contents.len(), 4);
    }

    #[fuchsia::test]
    async fn test_anonymized_service_directory_with_dynamic_instances_dynamic_child() {
        let components = create_test_component_decls();
        let mut baz_out_dir = OutDir::new();
        // Setup "baz" with 1 instance.
        let baz_service = pseudo_directory! {
            "default" => pseudo_directory! {
                "member" => pseudo_directory! {}
            }
        };
        let baz_svc = pseudo_directory! {
            "my.service.Service" => baz_service.clone(),
        };
        baz_out_dir.add_entry("/svc".parse().unwrap(), baz_svc);

        let test = RoutingTestBuilder::new("root", components)
            .set_component_outgoing_host_fn("baz", baz_out_dir.host_fn())
            .build()
            .await;

        let root = test.model.root();

        let provider =
            MockAnonymizedCapabilityProvider { instances: Arc::new(Mutex::new(hashmap! {})) };

        let route = AnonymizedServiceRoute {
            source_moniker: Moniker::root(),
            members: vec![AggregateMember::Collection("coll2".parse().unwrap())],
            service_name: "my.service.Service".parse().unwrap(),
        };

        let dir_arc = Arc::new(AnonymizedAggregateServiceDir::new(
            root.as_weak(),
            route,
            Box::new(provider.clone()),
        ));
        root.hooks.install(dir_arc.hooks());

        // Initialize the aggregate. There are no components yet so it will be empty.
        dir_arc.add_entries_from_children().await.unwrap();
        let dir_proxy = vfs::directory::serve_read_only(dir_arc.dir_entry(), ExecutionScope::new());
        let entries = dir_arc.entries().len();
        assert_eq!(entries, 0);
        let dir_contents = fuchsia_fs::directory::readdir(&dir_proxy)
            .await
            .expect("failed to read directory entries");

        // Create and start "baz" component. Ensure the start hook added the entry since the
        // aggregate already exists.
        test.create_dynamic_child(&Moniker::root(), "coll2", ChildBuilder::new().name("baz")).await;
        let baz_component =
            root.find_and_maybe_resolve(&"coll2:baz".parse().unwrap()).await.unwrap();
        provider.instances.lock().insert(
            AggregateInstance::Child("coll2:baz".try_into().unwrap()),
            baz_component.as_weak(),
        );
        baz_component.ensure_started(&StartReason::Eager).await.unwrap();
        let dir_contents = wait_for_dir_content_change(&dir_proxy, dir_contents).await;
        let entries = dir_arc.entries().len();
        assert_eq!(entries, 1);
        let previous_entries = entries;

        // Add one more instance to "baz"
        baz_service
            .add_node(
                "secondary",
                pseudo_directory! {
                    "member" => pseudo_directory! {},
                },
            )
            .expect("Could not add service node.");
        wait_for_dir_content_change(&dir_proxy, dir_contents).await;
        let entries = dir_arc.entries().len();
        assert_eq!(entries, previous_entries + 1);

        // Validate they both have "member".
        let dir_contents = fuchsia_fs::directory::readdir(&dir_proxy)
            .await
            .expect("failed to read directory entries");
        assert_eq!(dir_contents.len(), 2);
        for entry in &dir_contents {
            let instance_dir = fuchsia_fs::directory::open_directory(
                &dir_proxy,
                &entry.name,
                fuchsia_fs::PERM_READABLE,
            )
            .await
            .expect("failed to open collection dir");

            // Make sure we're reading the expected directory.
            let instance_dir_contents = fuchsia_fs::directory::readdir(&instance_dir)
                .await
                .expect("failed to read instances of collection dir");
            assert!(instance_dir_contents.iter().find(|d| d.name == "member").is_some());
        }

        // Remove some entries to make sure removal flow works.
        baz_service.remove_node("default").expect("Failed to remove default from baz.");
        let dir_contents = wait_for_dir_content_change(&dir_proxy, dir_contents).await;
        assert_eq!(dir_contents.len(), 1);

        baz_service.remove_node("secondary").expect("Failed to remove secondary from baz.");
        let dir_contents = wait_for_dir_content_change(&dir_proxy, dir_contents).await;
        assert_eq!(dir_contents.len(), 0);
    }

    #[fuchsia::test]
    async fn test_anonymized_service_directory_with_dynamic_instances_start_before_add() {
        let components = create_test_component_decls();
        let mut static_b_out_dir = OutDir::new();

        // Setup "static_b" with 1 instance.
        let static_b_service = pseudo_directory! {
            "default" => pseudo_directory! {
                "member" => pseudo_directory! {}
            }
        };
        let static_b_svc = pseudo_directory! {
            "my.service.Service" => static_b_service.clone(),
        };
        static_b_out_dir.add_entry("/svc".parse().unwrap(), static_b_svc);

        let test = RoutingTestBuilder::new("root", components)
            .set_component_outgoing_host_fn("static_b", static_b_out_dir.host_fn())
            .build()
            .await;

        let root = test.model.root();
        let static_b_component =
            root.find_and_maybe_resolve(&"static_b".parse().unwrap()).await.unwrap();

        let provider = MockAnonymizedCapabilityProvider {
            instances: Arc::new(Mutex::new(hashmap! {
                AggregateInstance::Child("static_b".try_into().unwrap()) => static_b_component.as_weak(),
            })),
        };

        let route = AnonymizedServiceRoute {
            source_moniker: Moniker::root(),
            members: vec![AggregateMember::Child(ChildRef {
                name: "static_b".parse().unwrap(),
                collection: None,
            })],
            service_name: "my.service.Service".parse().unwrap(),
        };

        let dir_arc = Arc::new(AnonymizedAggregateServiceDir::new(
            root.as_weak(),
            route,
            Box::new(provider.clone()),
        ));
        root.hooks.install(dir_arc.hooks());

        let dir_proxy = vfs::directory::serve_read_only(dir_arc.dir_entry(), ExecutionScope::new());

        // Ensure we are starting with 0 entries.
        let dir_contents = fuchsia_fs::directory::readdir(&dir_proxy)
            .await
            .expect("failed to read directory entries");
        assert_eq!(dir_contents.len(), 0);

        // We will start "static_b" before we add_entries_from_children.
        static_b_component.ensure_started(&StartReason::Eager).await.unwrap();

        // Ensure that the start hook added the instance.
        let dir_contents = wait_for_dir_content_change(&dir_proxy, dir_contents).await;
        let entries = dir_arc.entries().len();
        assert_eq!(entries, 1);
        assert_eq!(dir_contents.len(), 1);

        // This should be a no-op.
        dir_arc.add_entries_from_children().await.unwrap();
        let dir_contents = fuchsia_fs::directory::readdir(&dir_proxy)
            .await
            .expect("failed to read directory entries");
        let entries = dir_arc.entries().len();
        assert_eq!(entries, 1);
        assert_eq!(dir_contents.len(), 1);

        // Add another instance to "static_b", which had 1 instance previously.
        static_b_service
            .add_node(
                "secondary",
                pseudo_directory! {
                    "member" => pseudo_directory! {},
                },
            )
            .expect("Could not add service node.");
        wait_for_dir_content_change(&dir_proxy, dir_contents).await;
        let entries = dir_arc.entries().len();
        assert_eq!(entries, 2);

        // Check both.
        let dir_contents = fuchsia_fs::directory::readdir(&dir_proxy)
            .await
            .expect("failed to read directory entries");
        assert_eq!(dir_contents.len(), 2);
        for entry in &dir_contents {
            let instance_dir = fuchsia_fs::directory::open_directory(
                &dir_proxy,
                &entry.name,
                fuchsia_fs::PERM_READABLE,
            )
            .await
            .expect("failed to open collection dir");

            // Make sure we're reading the expected directory.
            let instance_dir_contents = fuchsia_fs::directory::readdir(&instance_dir)
                .await
                .expect("failed to read instances of collection dir");
            assert!(instance_dir_contents.iter().find(|d| d.name == "member").is_some());
        }

        // Remove some entries to make sure removal flow works.
        static_b_service.remove_node("default").expect("Failed to remove default from bar.");
        let dir_contents = wait_for_dir_content_change(&dir_proxy, dir_contents).await;
        assert_eq!(dir_contents.len(), 1);

        static_b_service.remove_node("secondary").expect("Failed to remove secondary from bar.");
        let dir_contents = wait_for_dir_content_change(&dir_proxy, dir_contents).await;
        assert_eq!(dir_contents.len(), 0);
    }

    #[fuchsia::test]
    async fn test_anonymized_service_directory_with_parent_and_self() {
        let leaf_component_decl = ComponentDeclBuilder::new()
            .expose(ExposeBuilder::service().name("my.service.Service").source(ExposeSource::Self_))
            .service_default("my.service.Service")
            .build();
        let components = vec![
            (
                "root",
                ComponentDeclBuilder::new()
                    .service_default("my.service.Service")
                    .offer(
                        OfferBuilder::service()
                            .name("my.service.Service")
                            .source(OfferSource::Self_)
                            .target_static_child("container")
                            .availability(cm_rust::Availability::Required),
                    )
                    .child_default("container")
                    .build(),
            ),
            (
                "container",
                ComponentDeclBuilder::new()
                    .service_default("my.service.Service")
                    .use_(
                        UseBuilder::protocol()
                            .source(UseSource::Framework)
                            .name("fuchsia.component.Realm"),
                    )
                    .offer(
                        OfferBuilder::service()
                            .name("my.service.Service")
                            .source(OfferSource::Collection("coll".parse().unwrap()))
                            .target_static_child("target")
                            .availability(cm_rust::Availability::Required),
                    )
                    .offer(
                        OfferBuilder::service()
                            .name("my.service.Service")
                            .source(OfferSource::Parent)
                            .target_static_child("target")
                            .availability(cm_rust::Availability::Required),
                    )
                    .offer(
                        OfferBuilder::service()
                            .name("my.service.Service")
                            .source(OfferSource::Self_)
                            .target_static_child("target")
                            .availability(cm_rust::Availability::Required),
                    )
                    .collection_default("coll")
                    .child_default("target")
                    .build(),
            ),
            (
                "target",
                ComponentDeclBuilder::new()
                    .use_(UseBuilder::protocol().name("my.service.Service"))
                    .build(),
            ),
            ("foo", leaf_component_decl.clone()),
            ("bar", leaf_component_decl.clone()),
        ];

        let mock_single_instance = pseudo_directory! {
            "default" => pseudo_directory! {
                "member" => pseudo_directory! {}
            }
        };

        let test = RoutingTestBuilder::new("root", components)
            .add_outgoing_path(
                "foo",
                "/svc/my.service.Service".parse().unwrap(),
                mock_single_instance.clone(),
            )
            .add_outgoing_path(
                "bar",
                "/svc/my.service.Service".parse().unwrap(),
                mock_single_instance.clone(),
            )
            .add_outgoing_path(
                "container",
                "/svc/my.service.Service".parse().unwrap(),
                mock_single_instance.clone(),
            )
            .add_outgoing_path(
                "root",
                "/svc/my.service.Service".parse().unwrap(),
                mock_single_instance,
            )
            .build()
            .await;

        test.create_dynamic_child(
            &"container".parse().unwrap(),
            "coll",
            ChildBuilder::new().name("foo"),
        )
        .await;
        test.create_dynamic_child(
            &"container".parse().unwrap(),
            "coll",
            ChildBuilder::new().name("bar"),
        )
        .await;

        let root = test.model.root();
        let container_component =
            root.find_and_maybe_resolve(&"container".parse().unwrap()).await.unwrap();
        let foo_component =
            root.find_and_maybe_resolve(&"container/coll:foo".parse().unwrap()).await.unwrap();
        let bar_component =
            root.find_and_maybe_resolve(&"container/coll:bar".parse().unwrap()).await.unwrap();

        let provider = MockAnonymizedCapabilityProvider {
            instances: Arc::new(Mutex::new(hashmap! {
                AggregateInstance::Parent => root.as_weak(),
                AggregateInstance::Self_ => container_component.as_weak(),
                AggregateInstance::Child("coll:foo".try_into().unwrap()) => foo_component.as_weak(),
                AggregateInstance::Child("coll:bar".try_into().unwrap()) => bar_component.as_weak(),
            })),
        };

        let route = AnonymizedServiceRoute {
            source_moniker: "container".parse().unwrap(),
            members: vec![
                AggregateMember::Collection("coll".parse().unwrap()),
                AggregateMember::Parent,
                AggregateMember::Self_,
            ],
            service_name: "my.service.Service".parse().unwrap(),
        };

        let dir = Arc::new(AnonymizedAggregateServiceDir::new(
            container_component.as_weak(),
            route,
            Box::new(provider),
        ));
        dir.add_entries_from_children().await.unwrap();

        root.hooks.install(dir.hooks());

        let dir_proxy = vfs::directory::serve_read_only(dir.dir_entry(), ExecutionScope::new());

        // List the entries of the directory served by `open`, and compare them to the
        // internal state.
        let instance_names = {
            let instance_names: HashSet<_> =
                dir.entries().into_iter().map(|e| e.name.to_string()).collect();
            let dir_contents = fuchsia_fs::directory::readdir(&dir_proxy).await.unwrap();
            let dir_instance_names: HashSet<_> = dir_contents.into_iter().map(|d| d.name).collect();
            assert_eq!(instance_names.len(), 4);
            assert_eq!(dir_instance_names, instance_names);
            instance_names
        };

        // Open one of the entries.
        {
            let instance_dir = fuchsia_fs::directory::open_directory(
                &dir_proxy,
                instance_names.iter().next().unwrap(),
                fuchsia_fs::PERM_READABLE,
            )
            .await
            .unwrap();

            // Make sure we're reading the expected directory.
            let instance_dir_contents =
                fuchsia_fs::directory::readdir(&instance_dir).await.unwrap();
            assert!(instance_dir_contents.iter().find(|d| d.name == "member").is_some());
        }

        // Add entries from the children again. This should be a no-op since all of them are
        // already there and we prevent duplicates.
        let dir_contents = {
            let previous_entries: HashSet<_> =
                dir.entries().into_iter().map(|e| (e.name.clone(), e.source_id.clone())).collect();
            dir.add_entries_from_children().await.unwrap();
            let entries: HashSet<_> =
                dir.entries().into_iter().map(|e| (e.name.clone(), e.source_id.clone())).collect();
            assert_eq!(entries, previous_entries);
            let dir_contents = fuchsia_fs::directory::readdir(&dir_proxy).await.unwrap();
            let dir_instance_names: HashSet<_> =
                dir_contents.iter().map(|d| d.name.clone()).collect();
            assert_eq!(dir_instance_names, instance_names);
            dir_contents
        };

        // Test that removal of instances works.
        {
            bar_component.stop().await.unwrap();
            let dir_contents = wait_for_dir_content_change(&dir_proxy, dir_contents).await;
            assert_eq!(dir_contents.len(), 3);

            test.start_instance_and_wait_start(bar_component.moniker()).await.unwrap();
            let dir_contents = wait_for_dir_content_change(&dir_proxy, dir_contents).await;
            assert_eq!(dir_contents.len(), 4);
        }
    }

    #[fuchsia::test]
    async fn test_anonymized_service_directory_component_started() {
        let (test, dir_arc) = create_anonymized_service_test_realm(false).await;

        let dir_proxy = vfs::directory::serve_read_only(dir_arc.dir_entry(), ExecutionScope::new());

        let entries = fuchsia_fs::directory::readdir(&dir_proxy).await.unwrap();
        let instance_names: HashSet<String> = entries.iter().map(|d| d.name.clone()).collect();
        // should be no entries in a non initialized collection service dir.
        assert_eq!(instance_names.len(), 0);

        let root = test.model.root();
        let foo_component =
            root.find_and_maybe_resolve(&["coll1:foo"].try_into().unwrap()).await.unwrap();

        // Test that starting an instance results in the collection service directory adding the
        // relevant instances.
        foo_component.ensure_started(&StartReason::Eager).await.unwrap();
        let entries = wait_for_dir_content_change(&dir_proxy, entries).await;
        assert_eq!(entries.len(), 1);

        let baz_component =
            root.find_and_maybe_resolve(&["coll2:baz"].try_into().unwrap()).await.unwrap();

        // Test with second collection
        baz_component.ensure_started(&StartReason::Eager).await.unwrap();
        let entries = wait_for_dir_content_change(&dir_proxy, entries).await;
        assert_eq!(entries.len(), 3);

        let static_a_component =
            root.find_and_maybe_resolve(&["static_a"].try_into().unwrap()).await.unwrap();

        // Test with static child
        static_a_component.ensure_started(&StartReason::Eager).await.unwrap();
        let entries = wait_for_dir_content_change(&dir_proxy, entries).await;
        assert_eq!(entries.len(), 4);
    }

    #[fuchsia::test]
    async fn test_anonymized_service_directory_component_stopped() {
        let (test, dir_arc) = create_anonymized_service_test_realm(true).await;

        let dir_proxy = vfs::directory::serve_read_only(dir_arc.dir_entry(), ExecutionScope::new());

        // List the entries of the directory served by `open`.
        let entries = fuchsia_fs::directory::readdir(&dir_proxy).await.unwrap();
        let instance_names: HashSet<String> = entries.iter().map(|d| d.name.clone()).collect();
        assert_eq!(instance_names.len(), 6);

        let root = test.model.root();
        let foo_component =
            root.find_and_maybe_resolve(&["coll1:foo"].try_into().unwrap()).await.unwrap();

        // Test that removal of instances works
        foo_component.stop().await.unwrap();
        let entries = wait_for_dir_content_change(&dir_proxy, entries).await;
        assert_eq!(entries.len(), 5);

        let baz_component =
            root.find_and_maybe_resolve(&["coll2:baz"].try_into().unwrap()).await.unwrap();

        // Test with second collection
        baz_component.stop().await.unwrap();
        let entries = wait_for_dir_content_change(&dir_proxy, entries).await;
        assert_eq!(entries.len(), 3);

        let static_a_component =
            root.find_and_maybe_resolve(&["static_a"].try_into().unwrap()).await.unwrap();

        // Test with static child
        static_a_component.stop().await.unwrap();
        let entries = wait_for_dir_content_change(&dir_proxy, entries).await;
        assert_eq!(entries.len(), 2);
    }

    #[fuchsia::test]
    async fn test_anonymized_service_directory_failed_to_route_child() {
        let components = create_test_component_decls();

        let mock_single_instance = pseudo_directory! {
            "default" => pseudo_directory! {
                "member" => pseudo_directory! {}
            }
        };

        let test = RoutingTestBuilder::new("root", components)
            .add_outgoing_path(
                "foo",
                "/svc/my.service.Service".parse().unwrap(),
                mock_single_instance.clone(),
            )
            .add_outgoing_path(
                "bar",
                "/svc/my.service.Service".parse().unwrap(),
                mock_single_instance,
            )
            .build()
            .await;

        let root = test.model.root();
        test.create_dynamic_child(&Moniker::root(), "coll1", ChildBuilder::new().name("foo")).await;
        test.create_dynamic_child(&Moniker::root(), "coll1", ChildBuilder::new().name("bar")).await;
        let foo_component =
            root.find_and_maybe_resolve(&["coll1:foo"].try_into().unwrap()).await.unwrap();

        let provider = MockAnonymizedCapabilityProvider {
            instances: Arc::new(Mutex::new(hashmap! {
                AggregateInstance::Child("coll1:foo".try_into().unwrap()) => foo_component.as_weak(),
                // "bar" not added to induce a routing failure on route_instance
            })),
        };

        let route = AnonymizedServiceRoute {
            source_moniker: Moniker::root(),
            members: vec![AggregateMember::Collection("coll1".parse().unwrap())],
            service_name: "my.service.Service".parse().unwrap(),
        };

        let dir =
            Arc::new(AnonymizedAggregateServiceDir::new(root.as_weak(), route, Box::new(provider)));

        dir.add_entries_from_children().await.unwrap();
        // Entries from foo should be available even though we can't route to bar
        let dir_proxy = vfs::directory::serve_read_only(dir.dir_entry(), ExecutionScope::new());

        // List the entries of the directory served by `open`.
        let entries = fuchsia_fs::directory::readdir(&dir_proxy).await.unwrap();
        let instance_names: HashSet<String> = entries.into_iter().map(|d| d.name).collect();
        assert_eq!(instance_names.len(), 1);
        for instance in instance_names {
            assert!(is_instance_id(&instance), "{}", instance);
        }
    }

    #[fuchsia::test]
    async fn test_anonymized_service_directory_readdir() {
        let components = create_test_component_decls();

        let mock_instance_foo = pseudo_directory! {
            "default" => pseudo_directory! {}
        };

        let mock_instance_bar = pseudo_directory! {
            "default" => pseudo_directory! {},
            "one" => pseudo_directory! {},
        };

        let mock_instance_static_a = pseudo_directory! {
            "default" => pseudo_directory! {}
        };

        let test = RoutingTestBuilder::new("root", components)
            .add_outgoing_path("foo", "/svc/my.service.Service".parse().unwrap(), mock_instance_foo)
            .add_outgoing_path("bar", "/svc/my.service.Service".parse().unwrap(), mock_instance_bar)
            .add_outgoing_path(
                "static_a",
                "/svc/my.service.Service".parse().unwrap(),
                mock_instance_static_a,
            )
            .build()
            .await;

        let root = test.model.root();
        test.create_dynamic_child(&Moniker::root(), "coll1", ChildBuilder::new().name("foo")).await;
        test.create_dynamic_child(&Moniker::root(), "coll2", ChildBuilder::new().name("bar")).await;
        let foo_component =
            root.find_and_maybe_resolve(&["coll1:foo"].try_into().unwrap()).await.unwrap();
        let bar_component =
            root.find_and_maybe_resolve(&["coll2:bar"].try_into().unwrap()).await.unwrap();
        let static_a_component =
            root.find_and_maybe_resolve(&["static_a"].try_into().unwrap()).await.unwrap();

        let provider = MockAnonymizedCapabilityProvider {
            instances: Arc::new(Mutex::new(hashmap! {
                AggregateInstance::Child("coll1:foo".try_into().unwrap()) => foo_component.as_weak(),
                AggregateInstance::Child("coll2:bar".try_into().unwrap()) => bar_component.as_weak(),
                AggregateInstance::Child("static_a".try_into().unwrap()) => static_a_component.as_weak(),
            })),
        };

        let route = AnonymizedServiceRoute {
            source_moniker: Moniker::root(),
            members: vec![
                AggregateMember::Collection("coll1".parse().unwrap()),
                AggregateMember::Collection("coll2".parse().unwrap()),
            ],
            service_name: "my.service.Service".parse().unwrap(),
        };

        let dir =
            Arc::new(AnonymizedAggregateServiceDir::new(root.as_weak(), route, Box::new(provider)));

        dir.add_entries_from_children().await.unwrap();

        let dir_proxy = vfs::directory::serve_read_only(dir.dir_entry(), ExecutionScope::new());

        let entries = fuchsia_fs::directory::readdir(&dir_proxy).await.unwrap();

        let instance_names: HashSet<String> = entries.into_iter().map(|d| d.name).collect();
        assert_eq!(instance_names.len(), 4);
        for instance in instance_names {
            assert!(is_instance_id(&instance), "{}", instance);
        }
    }

    proptest! {
        #[test]
        fn service_instance_id(seed in 0..u64::MAX) {
            let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
            let instance = generate_hex_name(&mut rng);
            assert!(is_instance_id(instance.as_str()), "{}", instance);

            // Verify it's random
            let instance2 = generate_hex_name(&mut rng);
            assert!(is_instance_id(instance2.as_str()), "{}", instance2);
            assert_ne!(instance, instance2);
        }
    }

    fn is_instance_id(id: &str) -> bool {
        id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit())
    }
}
