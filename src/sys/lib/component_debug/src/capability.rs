// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::realm::{
    GetAllInstancesError, GetDeclarationError, get_all_instances, get_resolved_declaration,
};
use cm_rust::offer::OfferDecl;
use cm_rust::{CapabilityDecl, ComponentDecl, ExposeDecl, SourceName, UseDecl};
use flex_fuchsia_sys2 as fsys;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use moniker::Moniker;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FindInstancesError {
    #[error("failed to get all instances: {0}")]
    GetAllInstancesError(#[from] GetAllInstancesError),

    #[error("failed to get manifest for {moniker}: {err}")]
    GetDeclarationError {
        moniker: Moniker,
        #[source]
        err: GetDeclarationError,
    },
}

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[cfg_attr(
    feature = "serde",
    derive(Deserialize, Serialize),
    serde(tag = "type", rename_all = "snake_case")
)]
pub enum RouteSegment {
    /// The capability was used by a component instance in its manifest.
    UseBy { moniker: Moniker, capability: UseDecl },

    /// The capability was offered by a component instance in its manifest.
    OfferBy { moniker: Moniker, capability: OfferDecl },

    /// The capability was exposed by a component instance in its manifest.
    ExposeBy { moniker: Moniker, capability: ExposeDecl },

    /// The capability was declared by a component instance in its manifest.
    DeclareBy { moniker: Moniker, capability: CapabilityDecl },
}

/// Find components that reference a capability matching the given |query|.
pub async fn get_all_route_segments(
    query: String,
    realm_query: &fsys::RealmQueryProxy,
) -> Result<Vec<RouteSegment>, FindInstancesError> {
    let instances = get_all_instances(realm_query).await?;
    let query = query.as_str();
    let mut results = FuturesUnordered::new();

    for instance in instances {
        results.push(async move {
            let result = get_resolved_declaration(&instance.moniker, realm_query);
            match result.await {
                Ok(decl) => {
                    let component_segments = get_segments(&instance.moniker, decl, &query);
                    Ok(component_segments)
                }
                // If the instance is not yet resolved, then we can't get its resolved declaration.
                // If the component doesn't exist, then it's been destroyed since the
                // `get_all_instances` call and we can't get its resolved declaration. Both of these
                // things are expected, so ignore these errors.
                Err(
                    GetDeclarationError::InstanceNotResolved(_)
                    | GetDeclarationError::InstanceNotFound(_),
                ) => Ok(vec![]),
                Err(err) => Err(FindInstancesError::GetDeclarationError {
                    moniker: instance.moniker.clone(),
                    err,
                }),
            }
        });
    }

    let mut segments = Vec::with_capacity(results.len());
    while let Some(result) = results.next().await {
        let mut component_segments = result?;
        segments.append(&mut component_segments);
    }

    Ok(segments)
}

/// Determine if a capability matching the |query| is declared, exposed, used or offered by
/// this component.
fn get_segments(moniker: &Moniker, manifest: ComponentDecl, query: &str) -> Vec<RouteSegment> {
    let mut segments = vec![];

    for capability in manifest.capabilities {
        if capability.name().to_string().contains(query) {
            segments.push(RouteSegment::DeclareBy { moniker: moniker.clone(), capability });
        }
    }

    for expose in manifest.exposes {
        if expose.source_name().to_string().contains(query) {
            segments.push(RouteSegment::ExposeBy { moniker: moniker.clone(), capability: expose });
        }
    }

    for use_ in manifest.uses {
        if use_.source_name().to_string().contains(query) {
            segments.push(RouteSegment::UseBy { moniker: moniker.clone(), capability: use_ });
        }
    }

    for offer in manifest.offers {
        if offer.source_name().to_string().contains(query) {
            segments.push(RouteSegment::OfferBy { moniker: moniker.clone(), capability: offer });
        }
    }

    segments
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::*;
    use cm_rust::offer::{OfferProtocolDecl, OfferSource, OfferTarget};
    use cm_rust::*;
    use cm_rust_testing::*;
    use std::collections::HashMap;

    fn create_realm_query() -> fsys::RealmQueryProxy {
        serve_realm_query(
            vec![fsys::Instance {
                moniker: Some("./my_foo".to_string()),
                url: Some("fuchsia-pkg://fuchsia.com/foo#meta/foo.cm".to_string()),
                instance_id: None,
                resolved_info: Some(fsys::ResolvedInfo {
                    resolved_url: Some("fuchsia-pkg://fuchsia.com/foo#meta/foo.cm".to_string()),
                    execution_info: None,
                    ..Default::default()
                }),
                ..Default::default()
            }],
            HashMap::from([(
                "./my_foo".to_string(),
                ComponentDeclBuilder::new()
                    .child(
                        ChildBuilder::new()
                            .name("my_bar")
                            .url("fuchsia-pkg://fuchsia.com/bar#meta/bar.cm"),
                    )
                    .protocol_default("fuchsia.foo.bar")
                    .use_(UseBuilder::protocol().name("fuchsia.foo.bar"))
                    .expose(
                        ExposeBuilder::protocol()
                            .name("fuchsia.foo.bar")
                            .source(ExposeSource::Self_),
                    )
                    .offer(
                        OfferBuilder::protocol()
                            .name("fuchsia.foo.bar")
                            .source(OfferSource::Self_)
                            .target_static_child("my_bar"),
                    )
                    .build()
                    .native_into_fidl(),
            )]),
            HashMap::new(),
            HashMap::new(),
        )
    }

    #[fuchsia::test]
    async fn segments() {
        let realm_query = create_realm_query();

        let segments =
            get_all_route_segments("fuchsia.foo.bar".to_string(), &realm_query).await.unwrap();

        assert_eq!(segments.len(), 4);

        let mut found_use = false;
        let mut found_offer = false;
        let mut found_expose = false;
        let mut found_declaration = false;

        for segment in segments {
            match segment {
                RouteSegment::UseBy { moniker, capability } => {
                    found_use = true;
                    assert_eq!(moniker, "/my_foo".try_into().unwrap());
                    assert_eq!(
                        capability,
                        UseDecl::Protocol(UseProtocolDecl {
                            source: UseSource::Parent,
                            source_name: "fuchsia.foo.bar".parse().unwrap(),
                            source_dictionary: Default::default(),
                            target_path: Some("/svc/fuchsia.foo.bar".parse().unwrap()),
                            dependency_type: DependencyType::Strong,
                            numbered_handle: None,
                            availability: Availability::Required
                        })
                    );
                }
                RouteSegment::OfferBy { moniker, capability } => {
                    found_offer = true;
                    assert_eq!(moniker, "/my_foo".try_into().unwrap());
                    assert_eq!(
                        capability,
                        OfferDecl::Protocol(OfferProtocolDecl {
                            source: OfferSource::Self_,
                            source_name: "fuchsia.foo.bar".parse().unwrap(),
                            source_dictionary: Default::default(),
                            target: OfferTarget::Child(ChildRef {
                                name: "my_bar".parse().unwrap(),
                                collection: None,
                            }),
                            target_name: "fuchsia.foo.bar".parse().unwrap(),
                            dependency_type: DependencyType::Strong,
                            availability: Availability::Required
                        })
                    );
                }
                RouteSegment::ExposeBy { moniker, capability } => {
                    found_expose = true;
                    assert_eq!(moniker, "/my_foo".try_into().unwrap());
                    assert_eq!(
                        capability,
                        ExposeDecl::Protocol(ExposeProtocolDecl {
                            source: ExposeSource::Self_,
                            source_name: "fuchsia.foo.bar".parse().unwrap(),
                            source_dictionary: Default::default(),
                            target: ExposeTarget::Parent,
                            target_name: "fuchsia.foo.bar".parse().unwrap(),
                            availability: Availability::Required
                        })
                    );
                }
                RouteSegment::DeclareBy { moniker, capability } => {
                    found_declaration = true;
                    assert_eq!(moniker, "/my_foo".try_into().unwrap());
                    assert_eq!(
                        capability,
                        CapabilityDecl::Protocol(ProtocolDecl {
                            name: "fuchsia.foo.bar".parse().unwrap(),
                            source_path: Some("/svc/fuchsia.foo.bar".parse().unwrap()),
                            delivery: Default::default(),
                        })
                    );
                }
            }
        }

        assert!(found_use);
        assert!(found_expose);
        assert!(found_offer);
        assert!(found_declaration);
    }

    #[cfg(feature = "serde")]
    #[fuchsia::test]
    async fn test_route_segment_serialization() {
        use serde_json::json;

        let segment = RouteSegment::DeclareBy {
            moniker: "/my_foo".try_into().unwrap(),
            capability: CapabilityDecl::Protocol(ProtocolDecl {
                name: "fuchsia.foo.bar".parse().unwrap(),
                source_path: Some("/svc/fuchsia.foo.bar".parse().unwrap()),
                delivery: Default::default(),
            }),
        };

        let serialized = serde_json::to_value(&segment).expect("failed to serialize RouteSegment");

        assert_eq!(
            serialized,
            json!({
                "type": "declare_by",
                "moniker": "/my_foo",
                "capability": {
                    "type": "protocol",
                    "name": "fuchsia.foo.bar",
                    "source_path": "/svc/fuchsia.foo.bar",
                    "delivery": "immediate"
                }
            })
        );
    }
}
