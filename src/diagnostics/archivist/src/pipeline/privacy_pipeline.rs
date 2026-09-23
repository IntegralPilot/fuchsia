// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
use crate::accessor::{ArchiveAccessorServer, ArchiveAccessorTranslator, ArchiveAccessorWriter};
use crate::configs;
use crate::diagnostics::AccessorStats;
use crate::error::Error;
use crate::pipeline::StaticHierarchyAllowlist;
use fidl::endpoints::{DiscoverableProtocolMarker, ProtocolMarker, ServerEnd};
use fidl_fuchsia_component_sandbox as fsandbox;
use fidl_fuchsia_diagnostics as fdiagnostics;
use fidl_fuchsia_diagnostics_host as fdiagnostics_host;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use fuchsia_fs::directory;
use fuchsia_inspect as inspect;
use fuchsia_sync::RwLock;
use futures::TryStreamExt;
use log::{debug, warn};
use moniker::ExtendedMoniker;
use std::borrow::Cow;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

const ALL_PIPELINE_NAME: &str = "all";

struct PipelineParameters {
    has_config: bool,
    name: Cow<'static, str>,
    empty_behavior: configs::EmptyBehavior,
}

#[derive(Copy, Clone)]
pub struct PipelinesDictionaryId(u64);

impl Deref for PipelinesDictionaryId {
    type Target = u64;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Overlay that mediates connections between servers and the central
/// data repository. The overlay is provided static configurations that
/// make it unique to a specific pipeline, and uses those static configurations
/// to offer filtered access to the central repository.
pub struct Pipeline {
    /// The name of the pipeline.
    name: Cow<'static, str>,

    /// Contains information about the configuration of the pipeline.
    _pipeline_node: Option<inspect::Node>,

    /// Contains information about the accessor requests done for this pipeline.
    stats: AccessorStats,

    /// The statically declared allowlist for data exfiltration on this pipeline.
    static_allowlist: RwLock<StaticHierarchyAllowlist>,
}

impl Pipeline {
    fn new(
        parameters: PipelineParameters,
        pipelines_path: &Path,
        parent_node: &inspect::Node,
        accessor_stats_node: &inspect::Node,
    ) -> Self {
        let mut _pipeline_node = None;
        let path = format!("{}/{}", pipelines_path.display(), parameters.name);
        let mut static_selectors = None;
        if parameters.has_config {
            let node = parent_node.create_child(parameters.name.as_ref());
            let mut config =
                configs::PipelineConfig::from_directory(path, parameters.empty_behavior);
            config.record_to_inspect(&node);
            _pipeline_node = Some(node);
            if !config.disable_filtering {
                static_selectors = config.take_inspect_selectors();
            }
        }
        let stats = AccessorStats::new(accessor_stats_node.create_child(parameters.name.as_ref()));
        Pipeline {
            _pipeline_node,
            stats,
            name: parameters.name,
            static_allowlist: RwLock::new(StaticHierarchyAllowlist::new(static_selectors)),
        }
    }

    fn protocol_name(&self) -> Cow<'_, str> {
        self.protocol_name_inner::<fdiagnostics::ArchiveAccessorMarker>()
    }

    fn host_protocol_name(&self) -> Cow<'_, str> {
        self.protocol_name_inner::<fdiagnostics_host::ArchiveAccessorMarker>()
    }

    fn protocol_name_inner<P: DiscoverableProtocolMarker>(&self) -> Cow<'_, str> {
        if self.name.as_ref() == ALL_PIPELINE_NAME {
            Cow::Borrowed(P::PROTOCOL_NAME)
        } else {
            Cow::Owned(format!("{}.{}", P::PROTOCOL_NAME, self.name))
        }
    }

    pub fn accessor_stats(&self) -> &AccessorStats {
        &self.stats
    }

    pub fn remove_component(&self, moniker: &ExtendedMoniker) {
        self.static_allowlist.write().remove_component(moniker);
    }

    pub fn add_component(&self, moniker: &ExtendedMoniker) -> Result<(), Error> {
        self.static_allowlist.write().add_component(moniker.clone())
    }

    pub fn static_hierarchy_allowlist(&self) -> StaticHierarchyAllowlist {
        // TODO(https://fxbug.dev/42159044): can we avoid cloning here? This clone is not super expensive
        // as it'll be just cloning arcs, but we could be more efficient here.
        // Due to lock semantics we can't just return a reference at the moment as it leads to
        // an ABBA lock between inspect insertion into the repo and inspect reading.
        self.static_allowlist.read().clone()
    }
}

#[cfg(test)]
impl Pipeline {
    pub fn for_test(static_selectors: Option<Vec<fdiagnostics::Selector>>) -> Self {
        Pipeline {
            _pipeline_node: None,
            name: Cow::Borrowed("test"),
            stats: AccessorStats::new(Default::default()),
            static_allowlist: RwLock::new(StaticHierarchyAllowlist::new(static_selectors)),
        }
    }
}

pub struct PipelineManager {
    pipelines: Vec<Arc<Pipeline>>,
    _pipelines_node: inspect::Node,
    _accessor_stats_node: inspect::Node,
    scope: Option<fasync::Scope>,
}

impl PipelineManager {
    pub async fn new(
        pipelines_path: PathBuf,
        pipelines_node: inspect::Node,
        accessor_stats_node: inspect::Node,
        scope: fasync::Scope,
    ) -> Self {
        let mut pipelines = vec![];
        if let Ok(dir) =
            directory::open_in_namespace(pipelines_path.to_str().unwrap(), fio::PERM_READABLE)
        {
            for entry in directory::readdir(&dir).await.expect("read dir") {
                if !matches!(entry.kind, directory::DirentKind::Directory) {
                    continue;
                }
                let empty_behavior = if entry.name == "feedback" || entry.name == "previous_boot" {
                    configs::EmptyBehavior::DoNotFilter
                } else {
                    configs::EmptyBehavior::Disable
                };
                let parameters = PipelineParameters {
                    has_config: true,
                    name: Cow::Owned(entry.name),
                    empty_behavior,
                };
                pipelines.push(Arc::new(Pipeline::new(
                    parameters,
                    &pipelines_path,
                    &pipelines_node,
                    &accessor_stats_node,
                )));
            }
        }
        pipelines.push(Arc::new(Pipeline::new(
            PipelineParameters {
                has_config: false,
                name: Cow::Borrowed(ALL_PIPELINE_NAME),
                empty_behavior: configs::EmptyBehavior::Disable,
            },
            &pipelines_path,
            &pipelines_node,
            &accessor_stats_node,
        )));
        Self {
            pipelines,
            _pipelines_node: pipelines_node,
            _accessor_stats_node: accessor_stats_node,
            scope: Some(scope),
        }
    }

    pub fn weak_pipelines(&self) -> Vec<Weak<Pipeline>> {
        self.pipelines.iter().map(Arc::downgrade).collect::<Vec<_>>()
    }

    pub async fn cancel(&mut self) {
        if let Some(scope) = self.scope.take() {
            scope.cancel().await;
        }
    }

    pub async fn serve_pipelines(
        &self,
        accessor_server: Arc<ArchiveAccessorServer>,
        id_gen: &sandbox::CapabilityIdGenerator,
        capability_store: &mut fsandbox::CapabilityStoreProxy,
    ) -> PipelinesDictionaryId {
        let accessors_dict_id = id_gen.next();
        capability_store.dictionary_create(accessors_dict_id).await.unwrap().unwrap();
        debug!("Will serve {} pipelines", self.pipelines.len());
        for pipeline in &self.pipelines {
            debug!("Installing spawning receivers for {}", pipeline.name);
            let accessor_pipeline = Arc::clone(pipeline);
            // Unwrap: safe, we must not cancel before serving.
            self.scope.as_ref().unwrap().spawn(handle_receiver_requests::<
                fdiagnostics::ArchiveAccessorMarker,
            >(
                get_receiver_stream(
                    pipeline.protocol_name(),
                    accessors_dict_id,
                    id_gen,
                    capability_store,
                )
                .await,
                Arc::clone(&accessor_server),
                Arc::clone(&accessor_pipeline),
            ));
            // Unwrap: safe, we must not cancel before serving.
            self.scope.as_ref().unwrap().spawn(handle_receiver_requests::<
                fdiagnostics_host::ArchiveAccessorMarker,
            >(
                get_receiver_stream(
                    pipeline.host_protocol_name(),
                    accessors_dict_id,
                    id_gen,
                    capability_store,
                )
                .await,
                Arc::clone(&accessor_server),
                Arc::clone(&accessor_pipeline),
            ));
        }

        PipelinesDictionaryId(accessors_dict_id)
    }
}

async fn get_receiver_stream(
    protocol_name: Cow<'_, str>,
    accessors_dict_id: u64,
    id_gen: &sandbox::CapabilityIdGenerator,
    capability_store: &mut fsandbox::CapabilityStoreProxy,
) -> fsandbox::ReceiverRequestStream {
    let (accessor_receiver_client, receiver_stream) =
        fidl::endpoints::create_request_stream::<fsandbox::ReceiverMarker>();
    let connector_id = id_gen.next();
    capability_store
        .connector_create(connector_id, accessor_receiver_client)
        .await
        .unwrap()
        .unwrap();
    debug!("Added {protocol_name} to the accessors dictionary.");
    capability_store
        .dictionary_insert(
            accessors_dict_id,
            &fsandbox::DictionaryItem { key: protocol_name.into_owned(), value: connector_id },
        )
        .await
        .unwrap()
        .unwrap();
    receiver_stream
}

async fn handle_receiver_requests<P>(
    mut receiver_stream: fsandbox::ReceiverRequestStream,
    accessor_server: Arc<ArchiveAccessorServer>,
    pipeline: Arc<Pipeline>,
) where
    P: ProtocolMarker,
    P::RequestStream: ArchiveAccessorTranslator + Send + 'static,
    <P::RequestStream as ArchiveAccessorTranslator>::InnerDataRequestChannel:
        ArchiveAccessorWriter + Send,
{
    while let Some(request) = receiver_stream.try_next().await.unwrap() {
        match request {
            fsandbox::ReceiverRequest::Receive { channel, control_handle: _ } => {
                debug!("Handling receive request for: {} -> {}", pipeline.name, P::DEBUG_NAME);
                let server_end = ServerEnd::<P>::new(channel);
                accessor_server.spawn_server::<P::RequestStream>(
                    Arc::clone(&pipeline),
                    server_end.into_stream(),
                );
            }
            fsandbox::ReceiverRequest::_UnknownMethod { method_type, ordinal, .. } => {
                warn!(method_type:?, ordinal; "Got unknown interaction on Receiver");
            }
        }
    }
}
