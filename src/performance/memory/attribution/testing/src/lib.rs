// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_memory_attribution as fattribution;
use futures::stream::{BoxStream, SelectAll};
use futures::{FutureExt, Stream, StreamExt};
use pin_project::pin_project;
use std::collections::HashMap;
use std::pin::Pin;
use std::task::{Context, Poll};

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
pub struct PrincipalIdentifier(pub u64);

#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum Resource {
    KernelObject(zx::Koid),
    Vmar { process: zx::Koid, base: usize, len: usize },
}

/// A simple tree breakdown of resource usage useful for tests.
#[derive(Debug, Clone)]
pub struct Principal {
    /// Identifier of the principal.
    pub identifier: PrincipalIdentifier,

    /// Name of the principal.
    pub name: String,

    /// Resources used by this principal.
    pub resources: Vec<Resource>,

    /// Children of the principal.
    pub children: Vec<Principal>,
}

impl Principal {
    pub fn new(identifier: PrincipalIdentifier, name: String) -> Principal {
        Principal { identifier, name, resources: vec![], children: vec![] }
    }

    pub fn child_by_name(&self, name: &str) -> Option<&Principal> {
        self.children.iter().find(|c| c.name == name)
    }
}

/// Obtain which resources are used for various activities by an attribution provider.
///
/// If one of the children under the attribution provider has detailed attribution
/// information, this function will recursively visit those children and build a
/// tree of nodes.
///
/// Returns a stream of tree that are momentary snapshots of the memory state.
/// The tree will evolve over time as principals are added and removed.
pub fn attribute_memory(
    identifier: PrincipalIdentifier,
    name: String,
    attribution_provider: fattribution::ProviderProxy,
    introspector: fcomponent::IntrospectorProxy,
) -> BoxStream<'static, Principal> {
    futures::stream::unfold(
        StreamState::new(identifier, name, introspector, attribution_provider),
        get_next,
    )
    .boxed()
}

/// Wait for the next hanging-get message and recompute the tree.
async fn get_next(mut state: StreamState) -> Option<(Principal, StreamState)> {
    let mut node = state
        .node
        .clone()
        .unwrap_or_else(|| Principal::new(state.identifier.clone(), state.name.clone()));
    let mut children: HashMap<PrincipalIdentifier, Principal> =
        node.children.clone().into_iter().map(|n| (n.identifier.clone(), n)).collect();

    // Wait for new attribution information.
    match state.next().await {
        Some(event) => {
            match event {
                // New attribution information for this principal.
                Event::Node(attributions) => {
                    for attribution in attributions {
                        handle_update(attribution, &mut state, &mut children).await;
                    }
                }
                // New attribution information for a child principal.
                Event::Child(child) => {
                    children.insert(child.identifier.clone(), child);
                }
            }
        }
        None => return None,
    }

    node.children = children.into_values().collect();
    state.node = Some(node.clone());
    Some((node, state))
}

async fn handle_update(
    attribution: fattribution::AttributionUpdate,
    state: &mut StreamState,
    children: &mut HashMap<PrincipalIdentifier, Principal>,
) {
    match attribution {
        fattribution::AttributionUpdate::Add(new_principal) => {
            let identifier_id = PrincipalIdentifier(new_principal.identifier.unwrap());
            let principal_name =
                get_identifier_string(new_principal.description.unwrap(), &state.introspector)
                    .await;

            // Recursively attribute memory in this child principal if applicable.
            if let Some(client) = new_principal.detailed_attribution {
                state.child_update.push(
                    attribute_memory(
                        identifier_id.clone(),
                        principal_name.clone(),
                        client.into_proxy(),
                        state.introspector.clone(),
                    )
                    .boxed(),
                );
            }
            children.insert(identifier_id.clone(), Principal::new(identifier_id, principal_name));
        }
        fattribution::AttributionUpdate::Update(updated_principal) => {
            let identifier = PrincipalIdentifier(updated_principal.identifier.unwrap());

            let child = children.get_mut(&identifier).unwrap();
            let raw_resources = match updated_principal.resources.unwrap() {
                fattribution::Resources::Data(d) => d.resources,
                fattribution::Resources::Buffer(b) => {
                    let mapping = mapped_vmo::ImmutableMapping::create_from_vmo(&b, false).unwrap();
                    let resource_vector: fattribution::Data = fidl::unpersist(&mapping).unwrap();
                    resource_vector.resources
                }
                fattribution::ResourcesUnknown!() => {
                    unimplemented!()
                }
            };
            child.resources = raw_resources
                .into_iter()
                .filter_map(|r| match r {
                    fattribution::Resource::KernelObject(koid) => {
                        Some(Resource::KernelObject(zx::Koid::from_raw(koid)))
                    }
                    fattribution::Resource::ProcessMapped(vmar) => Some(Resource::Vmar {
                        process: zx::Koid::from_raw(vmar.process),
                        base: vmar.base as usize,
                        len: vmar.len as usize,
                    }),
                    _ => todo!("unimplemented"),
                })
                .collect();
        }
        fattribution::AttributionUpdate::Remove(identifier_ref) => {
            let identifier = PrincipalIdentifier(identifier_ref);
            children.remove(&identifier);
        }
        x @ _ => panic!("unimplemented {x:?}"),
    }
}

async fn get_identifier_string(
    description: fattribution::Description,
    introspector: &fcomponent::IntrospectorProxy,
) -> String {
    match description {
        fattribution::Description::Component(c) => introspector
            .get_moniker(c)
            .await
            .expect("Inspector call failed")
            .expect("Inspector::GetMoniker call failed"),
        fattribution::Description::Part(sc) => sc.clone(),
        fattribution::DescriptionUnknown!() => todo!(),
    }
}

/// [`StreamState`] holds attribution information for a given tree of principals
/// rooted at the one identified by `name`.
///
/// It implements a [`Stream`] and will yield the next update to the tree when
/// any of the hanging-gets from principals in this tree returns.
///
/// [`get_next`] will poll this stream to process the update, such as adding a new
/// child principal.
#[pin_project]
struct StreamState {
    /// The identifier of the principal at the root of the tree.
    identifier: PrincipalIdentifier,

    /// The name of the principal at the root of the tree.
    name: String,

    /// A capability used to unseal component instance tokens back to monikers.
    introspector: fcomponent::IntrospectorProxy,

    /// The tree of principals rooted at `node`.
    node: Option<Principal>,

    /// A stream of `AttributionUpdate` events for the current principal.
    ///
    /// If the stream finished, it will be set to `None`.
    hanging_get_update: Option<BoxStream<'static, Vec<fattribution::AttributionUpdate>>>,

    /// A stream of child principal updates. Each `Principal` element should
    /// replace the existing child principal if there already is a child with
    /// the same name. [`SelectAll`] is used to merge the updates from all children
    /// into a single stream.
    #[pin]
    child_update: SelectAll<BoxStream<'static, Principal>>,
}

impl StreamState {
    fn new(
        identifier: PrincipalIdentifier,
        name: String,
        introspector: fcomponent::IntrospectorProxy,
        attribution_provider: fattribution::ProviderProxy,
    ) -> Self {
        Self {
            identifier,
            name: name.clone(),
            introspector,
            node: None,
            hanging_get_update: Some(Box::pin(hanging_get_stream(name, attribution_provider))),
            child_update: SelectAll::new(),
        }
    }
}

fn hanging_get_stream(
    name: String,
    proxy: fattribution::ProviderProxy,
) -> impl Stream<Item = Vec<fattribution::AttributionUpdate>> + 'static {
    futures::stream::unfold(proxy, move |proxy| {
        let name = name.clone();
        proxy.get().map(move |get_result| {
            let attributions = match get_result {
                Ok(application_result) => application_result
                    .unwrap_or_else(|e| {
                        panic!("Failed call to AttributionResponse for {name}: {e:?}")
                    })
                    .attributions
                    .unwrap_or_else(|| panic!("Failed memory attribution for {name}")),
                Err(fidl::Error::ClientChannelClosed {
                    epitaph: fidl::Epitaph::PeerClosed,
                    ..
                }) => {
                    // If the hanging-get failed due to peer closed, consider there are no more
                    // updates to this principal. The closing of this hanging-get races with the
                    // parent principal notifying with the `AttributionUpdate::Remove` message, so
                    // it is possible to observe a peer-closed here first and then get a
                    // `AttributionUpdate::Remove` for this principal.
                    return None;
                }
                Err(e) => {
                    panic!("Failed to get AttributionResponse for {name}: {e:?}");
                }
            };
            Some((attributions, proxy))
        })
    })
}

enum Event {
    Node(Vec<fattribution::AttributionUpdate>),
    Child(Principal),
}

impl Stream for StreamState {
    type Item = Event;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this.child_update.poll_next_unpin(cx) {
            Poll::Ready(Some(node)) => {
                return Poll::Ready(Some(Event::Child(node)));
            }
            Poll::Ready(None) => {}
            Poll::Pending => {}
        }
        match this.hanging_get_update.as_mut() {
            Some(hanging_get_update) => match hanging_get_update.poll_next_unpin(cx) {
                Poll::Ready(Some(attributions)) => {
                    return Poll::Ready(Some(Event::Node(attributions)));
                }
                Poll::Ready(None) => {
                    this.hanging_get_update = None;
                    // Return None to signal that this Principal is done.
                    return Poll::Ready(None);
                }
                Poll::Pending => {}
            },
            None => {}
        }
        return Poll::Pending;
    }
}
