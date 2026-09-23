// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

//! # Inspect Runtime
//!
//! This library contains the necessary functions to serve inspect from a component.

use fidl::AsHandleRef;
use fidl::endpoints::ClientEnd;
use fuchsia_component_client::connect_to_protocol;
use fuchsia_inspect::Inspector;
use log::error;
use pin_project::pin_project;
use std::future::Future;
use std::pin::{Pin, pin};
use std::task::{Context, Poll};
use {fidl_fuchsia_inspect as finspect, fuchsia_async as fasync};

#[cfg(fuchsia_api_level_at_least = "HEAD")]
pub use finspect::EscrowToken;

pub mod service;

/// A setting for the fuchsia.inspect.Tree server that indicates how the server should send
/// the Inspector's VMO. For fallible methods of sending, a fallback is also set.
#[derive(Clone)]
pub enum TreeServerSendPreference {
    /// Frozen denotes sending a copy-on-write VMO.
    /// `on_failure` refers to failure behavior, as not all VMOs
    /// can be frozen. In particular, freezing a VMO requires writing to it,
    /// so if an Inspector is created with a read-only VMO, freezing will fail.
    ///
    /// Failure behavior should be one of Live or DeepCopy.
    ///
    /// Frozen { on_failure: Live } is the default value of TreeServerSendPreference.
    Frozen { on_failure: Box<TreeServerSendPreference> },

    /// Live denotes sending a live handle to the VMO.
    ///
    /// A client might want this behavior if they have time sensitive writes
    /// to the VMO, because copy-on-write behavior causes the initial write
    /// to a page to be around 1% slower.
    Live,

    /// DeepCopy will send a private copy of the VMO. This should probably
    /// not be a client's first choice, as Frozen(DeepCopy) will provide the
    /// same semantic behavior while possibly avoiding an expensive copy.
    ///
    /// A client might want this behavior if they have time sensitive writes
    /// to the VMO, because copy-on-write behavior causes the initial write
    /// to a page to be around 1% slower.
    DeepCopy,
}

impl TreeServerSendPreference {
    /// Create a new [`TreeServerSendPreference`] that sends a frozen/copy-on-write VMO of the tree,
    /// falling back to the specified `failure_mode` if a frozen VMO cannot be provided.
    ///
    /// # Arguments
    ///
    /// * `failure_mode` - Fallback behavior to use if freezing the Inspect VMO fails.
    ///
    pub fn frozen_or(failure_mode: TreeServerSendPreference) -> Self {
        TreeServerSendPreference::Frozen { on_failure: Box::new(failure_mode) }
    }
}

impl Default for TreeServerSendPreference {
    fn default() -> Self {
        TreeServerSendPreference::frozen_or(TreeServerSendPreference::Live)
    }
}

/// Optional settings for serving `fuchsia.inspect.Tree`
#[derive(Default)]
pub struct PublishOptions {
    /// This specifies how the VMO should be sent over the `fuchsia.inspect.Tree` server.
    ///
    /// Default behavior is
    /// `TreeServerSendPreference::Frozen { on_failure: TreeServerSendPreference::Live }`.
    pub(crate) vmo_preference: TreeServerSendPreference,

    /// An name value which will show up in the metadata of snapshots
    /// taken from this `fuchsia.inspect.Tree` server. Defaults to
    /// fuchsia.inspect#DEFAULT_TREE_NAME.
    pub(crate) tree_name: Option<String>,

    /// Channel over which the InspectSink protocol will be used.
    pub(crate) inspect_sink_client: Option<ClientEnd<finspect::InspectSinkMarker>>,

    /// Scope on which the server will be spawned.
    pub(crate) custom_scope: Option<fasync::ScopeHandle>,

    /// If provided, `publish` will use this tree instead of creating a new one.
    pub(crate) tree: Option<TreeServerHandle>,
}

impl PublishOptions {
    /// This specifies how the VMO should be sent over the `fuchsia.inspect.Tree` server.
    ///
    /// Default behavior is
    /// `TreeServerSendPreference::Frozen { on_failure: TreeServerSendPreference::Live }`.
    pub fn send_vmo_preference(mut self, preference: TreeServerSendPreference) -> Self {
        self.vmo_preference = preference;
        self
    }

    /// This sets an optional name value which will show up in the metadata of snapshots
    /// taken from this `fuchsia.inspect.Tree` server.
    ///
    /// Default behavior is an empty string.
    pub fn inspect_tree_name(mut self, name: impl Into<String>) -> Self {
        self.tree_name = Some(name.into());
        self
    }

    /// Sets a custom fuchsia_async::Scope to use for serving Inspect.
    pub fn custom_scope(mut self, scope: fasync::ScopeHandle) -> Self {
        self.custom_scope = Some(scope);
        self
    }

    /// This allows the client to provide the InspectSink client channel.
    pub fn on_inspect_sink_client(
        mut self,
        client: ClientEnd<finspect::InspectSinkMarker>,
    ) -> Self {
        self.inspect_sink_client = Some(client);
        self
    }

    /// Use the provided [`TreeServerHandle`] instead of creating a new one. Skips the
    /// call to InspectSink.Publish, but still spawns a new Tree server to
    /// handle incoming requests.
    pub fn on_tree_server(mut self, tree: TreeServerHandle) -> Self {
        self.tree = Some(tree);
        self
    }
}

/// Spawns a server handling `fuchsia.inspect.Tree` requests and a handle
/// to the `fuchsia.inspect.Tree` is published using `fuchsia.inspect.InspectSink`.
///
/// Whenever the client wishes to stop publishing Inspect, the Controller may be dropped.
///
/// `None` will be returned on FIDL failures. This includes:
/// * Failing to convert a FIDL endpoint for `fuchsia.inspect.Tree`'s `TreeMarker` into a stream
/// * Failing to connect to the `InspectSink` protocol
/// * Failing to send the connection over the wire
#[must_use]
pub fn publish(
    inspector: &Inspector,
    options: PublishOptions,
) -> Option<PublishedInspectController> {
    let PublishOptions { vmo_preference, tree_name, inspect_sink_client, custom_scope, tree } =
        options;
    let scope = custom_scope
        .map(|handle| handle.new_child_with_name("inspect_runtime::publish"))
        .unwrap_or_else(|| fasync::Scope::new_with_name("inspect_runtime::publish"));

    if let Some(TreeServerHandle { client_koid: client, stream }) = tree {
        service::spawn_tree_server_with_stream(inspector.clone(), vmo_preference, stream, &scope);
        return Some(PublishedInspectController::new(inspector.clone(), scope, client));
    }

    let tree = service::spawn_tree_server(inspector.clone(), vmo_preference, &scope);

    let inspect_sink = inspect_sink_client.map(|client| client.into_proxy()).or_else(|| {
        connect_to_protocol::<finspect::InspectSinkMarker>()
            .map_err(|err| error!(err:%; "failed to spawn the fuchsia.inspect.Tree server"))
            .ok()
    })?;

    // unwrap: safe since we have a valid tree handle coming from the server we spawn.
    let tree_koid = tree.as_handle_ref().koid().unwrap();
    if let Err(err) = inspect_sink.publish(finspect::InspectSinkPublishRequest {
        tree: Some(tree),
        name: tree_name,
        ..finspect::InspectSinkPublishRequest::default()
    }) {
        error!(err:%; "failed to spawn the fuchsia.inspect.Tree server");
        return None;
    }

    Some(PublishedInspectController::new(inspector.clone(), scope, tree_koid))
}

/// Options for fetching a VMO that was previously escrowed.
#[derive(Debug, Default)]
pub struct FetchEscrowOptions {
    /// Channel over which the InspectSink protocol will be used.
    pub(crate) inspect_sink_client: Option<ClientEnd<finspect::InspectSinkMarker>>,

    /// If true, the escrowed Inspect tree will be replaced with a new one, and a handle
    /// to the new tree will be returned in [`FetchEscrowResult`].
    pub(crate) should_replace_with_tree: bool,
}

impl FetchEscrowOptions {
    /// Creates new default options for fetching an escrowed VMO.
    pub fn new() -> Self {
        Self::default()
    }

    /// This allows the client to provide the InspectSink client channel.
    pub fn on_inspect_sink_client(
        mut self,
        client: ClientEnd<finspect::InspectSinkMarker>,
    ) -> Self {
        self.inspect_sink_client = Some(client);
        self
    }

    /// If true, the escrowed Inspect tree will be replaced with a new one, and a handle
    /// to the new tree will be returned in [`FetchEscrowResult`].
    pub fn replace_with_tree(mut self) -> Self {
        self.should_replace_with_tree = true;
        self
    }
}

/// The result of fetching an escrowed VMO.
pub struct FetchEscrowResult {
    /// The VMO containing the escrowed Inspect data.
    pub vmo: zx::Vmo,
    /// A handle to the new Inspect Tree if one was requested.
    pub server: Option<TreeServerHandle>,
}

/// A handle to a `fuchsia.inspect.Tree` server.
pub struct TreeServerHandle {
    client_koid: zx::Koid,
    stream: finspect::TreeRequestStream,
}

/// Fetches a VMO that was previously escrowed.
///
/// This function connects to `fuchsia.inspect.InspectSink` and exchanges the provided
/// `escrow_token` for the VMO it represents.
///
/// If `FetchEscrowOptions::replace_with_tree` is set, a new `fuchsia.inspect.Tree` server
/// will be created to replace the one that was torn down when the VMO was originally escrowed.
/// A handle to this new tree will be returned.
#[cfg(fuchsia_api_level_at_least = "HEAD")]
pub async fn fetch_escrow(
    escrow_token: finspect::EscrowToken,
    options: FetchEscrowOptions,
) -> Result<FetchEscrowResult, anyhow::Error> {
    use anyhow::{Context as _, anyhow};

    let FetchEscrowOptions { inspect_sink_client, should_replace_with_tree } = options;

    let (tree, handle) = if should_replace_with_tree {
        let (client, stream) = fidl::endpoints::create_request_stream::<finspect::TreeMarker>();
        // unwrap: safe since we have a valid tree handle coming from above.
        let client_koid = client.as_handle_ref().koid().unwrap();
        (Some(client), Some(TreeServerHandle { client_koid, stream }))
    } else {
        (None, None)
    };

    let inspect_sink = match inspect_sink_client {
        Some(client) => client.into_proxy(),
        None => connect_to_protocol::<finspect::InspectSinkMarker>()?,
    };

    let vmo = inspect_sink
        .fetch_escrow(finspect::InspectSinkFetchEscrowRequest {
            token: Some(escrow_token),
            tree,
            ..Default::default()
        })
        .await
        .context("Failed to fetch escrow")?
        .vmo
        .ok_or_else(|| {
            anyhow!("VMO missing from response; perhaps the provided escrow_token is invalid")
        })?;

    Ok(FetchEscrowResult { vmo, server: handle })
}

#[pin_project]
pub struct PublishedInspectController {
    #[pin]
    scope: fasync::scope::Join,
    inspector: Inspector,
    tree_koid: zx::Koid,
}

#[cfg(fuchsia_api_level_at_least = "HEAD")]
#[derive(Default)]
pub struct EscrowOptions {
    name: Option<String>,
    inspect_sink: Option<finspect::InspectSinkProxy>,
}

#[cfg(fuchsia_api_level_at_least = "HEAD")]
impl EscrowOptions {
    /// Sets the name with which the Inspect handle will be escrowed.
    pub fn name(mut self, name: impl Into<String>) -> Self {
        self.name = Some(name.into());
        self
    }

    /// Sets the inspect sink channel to use for escrowing.
    pub fn inspect_sink(mut self, proxy: finspect::InspectSinkProxy) -> Self {
        self.inspect_sink = Some(proxy);
        self
    }
}

#[cfg(fuchsia_api_level_at_least = "HEAD")]
#[derive(Debug, thiserror::Error)]
pub enum EscrowError {
    #[error("Failed to spawn the fuchsia.inspect.Tree server: {0}")]
    SpawnTreeServer(#[from] anyhow::Error),
    #[error("Failed to get a frozen vmo, aborting escrow: {0}")]
    GetFrozenVmo(#[from] fuchsia_inspect::Error),
    #[error("Failed to escrow inspect data: {0}")]
    Escrow(#[from] fidl::Error),
}

impl PublishedInspectController {
    fn new(inspector: Inspector, scope: fasync::Scope, tree_koid: zx::Koid) -> Self {
        Self { inspector, scope: scope.join(), tree_koid }
    }

    /// Escrows a frozen copy of the VMO of the associated Inspector replacing the current live
    /// handle in the server.
    /// This will not capture lazy nodes or properties.
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    pub async fn escrow_frozen(self, opts: EscrowOptions) -> Result<EscrowToken, EscrowError> {
        let inspect_sink = match opts.inspect_sink {
            Some(proxy) => proxy,
            None => match connect_to_protocol::<finspect::InspectSinkMarker>() {
                Ok(inspect_sink) => inspect_sink,
                Err(err) => {
                    return Err(EscrowError::SpawnTreeServer(err));
                }
            },
        };
        let (ep0, ep1) = zx::EventPair::create();
        let vmo = match self.inspector.frozen_vmo_copy() {
            Ok(vmo) => vmo,
            Err(err) => {
                return Err(EscrowError::GetFrozenVmo(err));
            }
        };
        if let Err(err) = inspect_sink.escrow(finspect::InspectSinkEscrowRequest {
            vmo: Some(vmo),
            name: opts.name,
            token: Some(EscrowToken { token: ep0 }),
            tree: Some(self.tree_koid.raw_koid()),
            ..Default::default()
        }) {
            return Err(EscrowError::Escrow(err));
        }
        self.scope.await;
        Ok(EscrowToken { token: ep1 })
    }

    /// Cancels the running published controller.
    ///
    /// The future resolves when no more serving tasks are running.
    pub async fn cancel(self) {
        let Self { scope, inspector: _, tree_koid: _ } = self;
        let scope = pin!(scope);
        scope.cancel().await;
    }
}

impl Future for PublishedInspectController {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        this.scope.poll(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use component_events::events::{EventStream, Started};
    use component_events::matcher::EventMatcher;
    use diagnostics_assertions::assert_json_diff;
    use diagnostics_hierarchy::DiagnosticsHierarchy;
    use diagnostics_reader::ArchiveReader;
    use fidl::endpoints::RequestStream;
    use fidl_fuchsia_inspect::{InspectSinkRequest, InspectSinkRequestStream};
    use fuchsia_component_test::ScopedInstance;
    use fuchsia_inspect::InspectorConfig;
    use fuchsia_inspect::reader::snapshot::Snapshot;
    use fuchsia_inspect::reader::{PartialNodeHierarchy, read};

    use futures::{FutureExt, StreamExt};

    const TEST_PUBLISH_COMPONENT_URL: &str = "#meta/inspect_test_component.cm";

    #[fuchsia::test]
    async fn new_no_op() {
        let inspector = Inspector::new(InspectorConfig::default().no_op());
        assert!(!inspector.is_valid());

        // Ensure publish doesn't crash on a No-Op inspector.
        // The idea is that in this context, publish will hang if the server is running
        // correctly. That is, if there is an error condition, it will be immediate.
        assert_matches!(
            publish(&inspector, PublishOptions::default()).unwrap().now_or_never(),
            None
        );
    }

    #[fuchsia::test]
    async fn connect_to_service() -> Result<(), anyhow::Error> {
        let mut event_stream = EventStream::open().await.unwrap();

        let app = ScopedInstance::new_with_name(
            "interesting_name".into(),
            "coll".to_string(),
            TEST_PUBLISH_COMPONENT_URL.to_string(),
        )
        .await
        .expect("failed to create test component");

        let started_stream = EventMatcher::ok()
            .moniker_regex(app.child_name().to_owned())
            .wait::<Started>(&mut event_stream);

        app.connect_to_binder().expect("failed to connect to Binder protocol");

        started_stream.await.expect("failed to observe Started event");

        let hierarchy = ArchiveReader::inspect()
            .add_selector("coll\\:interesting_name:[name=tree-0]root")
            .snapshot()
            .await?
            .into_iter()
            .next()
            .and_then(|result| result.payload)
            .expect("one Inspect hierarchy");

        assert_json_diff!(hierarchy, root: {
            "tree-0": 0u64,
            int: 3i64,
            "lazy-node": {
                a: "test",
                child: {
                    double: 3.25,
                },
            }
        });

        Ok(())
    }

    #[fuchsia::test]
    async fn publish_new_no_op() {
        let inspector = Inspector::new(InspectorConfig::default().no_op());
        assert!(!inspector.is_valid());

        // Ensure publish doesn't crash on a No-Op inspector
        let _task = publish(&inspector, PublishOptions::default());
    }

    #[fuchsia::test]
    async fn publish_on_provided_channel() {
        let (client, server) = zx::Channel::create();
        let inspector = Inspector::default();
        inspector.root().record_string("hello", "world");
        let _inspect_sink_server_task = publish(
            &inspector,
            PublishOptions::default()
                .on_inspect_sink_client(ClientEnd::<finspect::InspectSinkMarker>::new(client)),
        );
        let mut request_stream =
            InspectSinkRequestStream::from_channel(fidl::AsyncChannel::from_channel(server));

        let tree = request_stream.next().await.unwrap();

        assert_matches!(tree, Ok(InspectSinkRequest::Publish {
            payload: finspect::InspectSinkPublishRequest { tree: Some(tree), .. }, ..}) => {
                let hierarchy = read(&tree.into_proxy()).await.unwrap();
                assert_json_diff!(hierarchy, root: {
                    hello: "world"
                });
            }
        );

        assert!(request_stream.next().await.is_none());
    }

    #[fuchsia::test]
    async fn cancel_published_controller() {
        let (client, server) = zx::Channel::create();
        let inspector = Inspector::default();
        inspector.root().record_string("hello", "world");
        let controller = publish(
            &inspector,
            PublishOptions::default()
                .on_inspect_sink_client(ClientEnd::<finspect::InspectSinkMarker>::new(client)),
        )
        .expect("create controller");
        let mut request_stream =
            InspectSinkRequestStream::from_channel(fidl::AsyncChannel::from_channel(server));

        let tree = request_stream.next().await.unwrap();

        let tree = assert_matches!(tree, Ok(InspectSinkRequest::Publish {
            payload: finspect::InspectSinkPublishRequest { tree: Some(tree), .. }, ..}) => tree
        );

        assert!(request_stream.next().await.is_none());

        controller.cancel().await;
        fidl::AsyncChannel::from_channel(tree.into_channel())
            .on_closed()
            .await
            .expect("wait closed");
    }

    #[fuchsia::test]
    async fn controller_supports_escrowing_a_copy() {
        let inspector = Inspector::default();
        inspector.root().record_string("hello", "world");

        let (client, mut request_stream) = fidl::endpoints::create_request_stream();
        let controller =
            publish(&inspector, PublishOptions::default().on_inspect_sink_client(client))
                .expect("got controller");

        let request = request_stream.next().await.unwrap();
        let tree_koid = match request {
            Ok(InspectSinkRequest::Publish {
                payload: finspect::InspectSinkPublishRequest { tree: Some(tree), .. },
                ..
            }) => tree.as_handle_ref().basic_info().unwrap().koid,
            other => {
                panic!("unexpected request: {other:?}");
            }
        };
        let (proxy, mut request_stream) =
            fidl::endpoints::create_proxy_and_stream::<finspect::InspectSinkMarker>();
        let (client_token, request) = futures::future::join(
            controller.escrow_frozen(EscrowOptions {
                name: Some("test".into()),
                inspect_sink: Some(proxy),
            }),
            request_stream.next(),
        )
        .await;
        match request {
            Some(Ok(InspectSinkRequest::Escrow {
                payload:
                    finspect::InspectSinkEscrowRequest {
                        vmo: Some(vmo),
                        name: Some(name),
                        token: Some(EscrowToken { token }),
                        tree: Some(tree),
                        ..
                    },
                ..
            })) => {
                assert_eq!(name, "test");
                assert_eq!(tree, tree_koid.raw_koid());

                // An update to the inspector isn't reflected here, since it was  CoW.
                inspector.root().record_string("hey", "not there");

                let snapshot = Snapshot::try_from(&vmo).expect("valid vmo");
                let hierarchy: DiagnosticsHierarchy =
                    PartialNodeHierarchy::try_from(snapshot).expect("valid snapshot").into();
                assert_json_diff!(hierarchy, root: {
                    hello: "world"
                });
                assert_eq!(
                    client_token.unwrap().token.as_handle_ref().basic_info().unwrap().koid,
                    token.as_handle_ref().basic_info().unwrap().related_koid
                );
            }
            other => {
                panic!("unexpected request: {other:?}");
            }
        };
    }

    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    #[fuchsia::test]
    async fn fetch_escrow_works() {
        let (client, mut request_stream) =
            fidl::endpoints::create_request_stream::<finspect::InspectSinkMarker>();
        let (_local_token, remote_token) = zx::EventPair::create();
        let token = EscrowToken { token: remote_token };
        let expected_koid = token.token.as_handle_ref().basic_info().unwrap().koid;

        let publisher_fut =
            fetch_escrow(token, FetchEscrowOptions::new().on_inspect_sink_client(client));

        let server_fut = async {
            let (payload, responder) = assert_matches!(
                request_stream.next().await,
                Some(Ok(InspectSinkRequest::FetchEscrow { payload, responder })) => (payload, responder)
            );
            let received_token = payload.token.unwrap();
            assert_eq!(
                received_token.token.as_handle_ref().basic_info().unwrap().koid,
                expected_koid
            );
            responder
                .send(finspect::InspectSinkFetchEscrowResponse {
                    vmo: Some(zx::Vmo::create(0).unwrap()),
                    ..Default::default()
                })
                .unwrap();
        };

        let (result, _) = futures::join!(publisher_fut, server_fut);
        assert!(result.is_ok());
    }
}
