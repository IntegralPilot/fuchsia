// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use directed_graph::DirectedGraph;
use fidl_fuchsia_component_decl as fdecl;
use flyweights::FlyStr;
use std::fmt;

/// A node in the DependencyGraph. The first string describes the type of node and the second
/// string is the name of the node.
#[derive(Clone, Hash, Ord, Debug, PartialOrd, PartialEq, Eq)]
pub enum DependencyNode {
    Self_,
    Child(FlyStr, Option<FlyStr>),
    Collection(FlyStr),
    Environment(FlyStr),
    Capability(FlyStr),
}

impl fmt::Display for DependencyNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DependencyNode::Self_ => write!(f, "self"),
            DependencyNode::Child(name, None) => write!(f, "child {}", name),
            DependencyNode::Child(name, Some(collection)) => {
                write!(f, "child {}:{}", collection, name)
            }
            DependencyNode::Collection(name) => write!(f, "collection {}", name),
            DependencyNode::Environment(name) => write!(f, "environment {}", name),
            DependencyNode::Capability(name) => write!(f, "capability {}", name),
        }
    }
}

fn ref_to_dependency_node(ref_: Option<&fdecl::Ref>) -> Option<DependencyNode> {
    match ref_? {
        fdecl::Ref::Self_(_) => Some(DependencyNode::Self_),
        fdecl::Ref::Child(fdecl::ChildRef { name, collection }) => {
            Some(DependencyNode::Child(name.into(), collection.as_ref().map(|s| s.into())))
        }
        fdecl::Ref::Collection(fdecl::CollectionRef { name }) => {
            Some(DependencyNode::Collection(name.into()))
        }
        fdecl::Ref::Capability(fdecl::CapabilityRef { name }) => {
            Some(DependencyNode::Capability(name.into()))
        }
        fdecl::Ref::Framework(_)
        | fdecl::Ref::Parent(_)
        | fdecl::Ref::Debug(_)
        | fdecl::Ref::VoidType(_) => None,
        #[cfg(fuchsia_api_level_at_least = "HEAD")]
        fdecl::Ref::Environment(_) => None,
        _ => None,
    }
}

// Generates the edges of the graph that are from a components `uses`.
fn add_dependencies_from_uses(
    strong_dependencies: &mut DirectedGraph<DependencyNode>,
    decl: &fdecl::Component,
    dynamic_children: &[(&str, &str)],
) {
    if let Some(uses) = decl.uses.as_ref() {
        for use_ in uses.iter() {
            #[allow(unused_variables)]
            let (dependency_type, source, source_name, dict) = match use_ {
                fdecl::Use::Service(u) => {
                    (u.dependency_type, &u.source, &u.source_name, u.source_dictionary.as_ref())
                }
                fdecl::Use::Protocol(u) => {
                    (u.dependency_type, &u.source, &u.source_name, u.source_dictionary.as_ref())
                }
                fdecl::Use::Directory(u) => {
                    (u.dependency_type, &u.source, &u.source_name, u.source_dictionary.as_ref())
                }
                fdecl::Use::EventStream(u) => (
                    Some(fdecl::DependencyType::Strong),
                    &u.source,
                    &u.source_name,
                    None::<&String>,
                ),
                #[cfg(fuchsia_api_level_at_least = "HEAD")]
                fdecl::Use::Runner(u) => (
                    Some(fdecl::DependencyType::Strong),
                    &u.source,
                    &u.source_name,
                    u.source_dictionary.as_ref(),
                ),
                #[cfg(fuchsia_api_level_at_least = "HEAD")]
                fdecl::Use::Config(u) => (
                    Some(fdecl::DependencyType::Strong),
                    &u.source,
                    &u.source_name,
                    u.source_dictionary.as_ref(),
                ),
                // Storage can only be used from parent, which we don't track.
                fdecl::Use::Storage(_) => continue,
                _ => continue,
            };
            if dependency_type != Some(fdecl::DependencyType::Strong) {
                continue;
            }

            let dependency_nodes = match &source {
                Some(fdecl::Ref::Child(fdecl::ChildRef { name, collection })) => {
                    vec![DependencyNode::Child(name.into(), collection.as_ref().map(|s| s.into()))]
                }
                Some(fdecl::Ref::Self_(_)) => {
                    if dict.as_ref().is_some() {
                        if let Some(source_name) = source_name.as_ref() {
                            vec![DependencyNode::Capability(source_name.into())]
                        } else {
                            vec![]
                        }
                    } else {
                        vec![]
                    }
                }
                Some(fdecl::Ref::Collection(fdecl::CollectionRef { name })) => {
                    let mut nodes = vec![];
                    for child_name in dynamic_children_in_collection(dynamic_children, &name) {
                        nodes.push(DependencyNode::Child(child_name, Some(name.into())));
                    }
                    nodes
                }
                _ => vec![],
            };

            for source_node in dependency_nodes {
                strong_dependencies.add_edge(source_node, DependencyNode::Self_);
            }
        }
    }
}

fn add_dependencies_from_capabilities(
    strong_dependencies: &mut DirectedGraph<DependencyNode>,
    decl: &fdecl::Component,
) {
    if let Some(capabilities) = decl.capabilities.as_ref() {
        for cap in capabilities {
            match cap {
                fdecl::Capability::Dictionary(dictionary) => {
                    if dictionary.source_path.as_ref().is_some() {
                        if let Some(name) = dictionary.name.as_ref() {
                            // If `source_path` is set that means the dictionary is provided by the program,
                            // which implies a dependency from `self` to the dictionary declaration.
                            strong_dependencies.add_edge(
                                DependencyNode::Self_,
                                DependencyNode::Capability(name.into()),
                            );
                        }
                    }
                }
                fdecl::Capability::Storage(storage) => {
                    if let (Some(name), Some(_backing_dir)) =
                        (storage.name.as_ref(), storage.backing_dir.as_ref())
                    {
                        if let Some(source_node) = ref_to_dependency_node(storage.source.as_ref()) {
                            strong_dependencies
                                .add_edge(source_node, DependencyNode::Capability(name.into()));
                        }
                    }
                }
                _ => continue,
            }
        }
    }
}

fn add_dependencies_from_environments(
    strong_dependencies: &mut DirectedGraph<DependencyNode>,
    decl: &fdecl::Component,
) {
    if let Some(environment) = decl.environments.as_ref() {
        for environment in environment {
            if let Some(name) = &environment.name {
                let target = DependencyNode::Environment(name.into());
                if let Some(debugs) = environment.debug_capabilities.as_ref() {
                    for debug in debugs {
                        if let fdecl::DebugRegistration::Protocol(o) = debug {
                            if let Some(source_node) = ref_to_dependency_node(o.source.as_ref()) {
                                strong_dependencies.add_edge(source_node, target.clone());
                            }
                        }
                    }
                }
                if let Some(runners) = environment.runners.as_ref() {
                    for runner in runners {
                        if let Some(source_node) = ref_to_dependency_node(runner.source.as_ref()) {
                            strong_dependencies.add_edge(source_node, target.clone());
                        }
                    }
                }
                if let Some(resolvers) = environment.resolvers.as_ref() {
                    for resolver in resolvers {
                        if let Some(source_node) = ref_to_dependency_node(resolver.source.as_ref())
                        {
                            strong_dependencies.add_edge(source_node, target.clone());
                        }
                    }
                }
            }
        }
    }
}

fn add_dependencies_from_children(
    strong_dependencies: &mut DirectedGraph<DependencyNode>,
    decl: &fdecl::Component,
) {
    if let Some(children) = decl.children.as_ref() {
        for child in children {
            if let Some(name) = child.name.as_ref() {
                if let Some(env) = child.environment.as_ref() {
                    let source = DependencyNode::Environment(env.into());
                    let target = DependencyNode::Child(name.into(), None);
                    strong_dependencies.add_edge(source, target);
                }
            }
        }
    }
}

fn add_dependencies_from_collections(
    strong_dependencies: &mut DirectedGraph<DependencyNode>,
    decl: &fdecl::Component,
    dynamic_children: &[(&str, &str)],
) {
    if let Some(collections) = decl.collections.as_ref() {
        for collection in collections {
            if let Some(env) = collection.environment.as_ref() {
                if let Some(name) = collection.name.as_ref() {
                    let source = DependencyNode::Environment(env.into());
                    let target = DependencyNode::Collection(name.into());
                    strong_dependencies.add_edge(source.clone(), target);

                    for child_name in dynamic_children_in_collection(dynamic_children, &name) {
                        strong_dependencies.add_edge(
                            source.clone(),
                            DependencyNode::Child(child_name, Some(name.into())),
                        );
                    }
                }
            }
        }
    }
}

fn find_offer_node(
    offer: &fdecl::Offer,
    source: Option<&fdecl::Ref>,
    source_name: &Option<String>,
    _dictionary: Option<&String>,
) -> Option<DependencyNode> {
    if source.is_none() {
        return None;
    }

    match source? {
        fdecl::Ref::Child(fdecl::ChildRef { name, collection }) => {
            Some(DependencyNode::Child(name.into(), collection.as_ref().map(|s| s.into())))
        }
        fdecl::Ref::Self_(_) if _dictionary.is_some() => {
            let root_dict = _dictionary.unwrap().split('/').next().unwrap();
            return Some(DependencyNode::Capability(root_dict.into()));
        }
        fdecl::Ref::Self_(_) => {
            if let Some(source_name) = source_name {
                if matches!(offer, fdecl::Offer::Dictionary(_)) {
                    return Some(DependencyNode::Capability(source_name.into()));
                }
                if matches!(offer, fdecl::Offer::Storage(_)) {
                    return Some(DependencyNode::Capability(source_name.into()));
                }
            }

            Some(DependencyNode::Self_)
        }
        fdecl::Ref::Collection(fdecl::CollectionRef { name }) => {
            Some(DependencyNode::Collection(name.into()))
        }
        fdecl::Ref::Capability(fdecl::CapabilityRef { name }) => {
            Some(DependencyNode::Capability(name.into()))
        }
        fdecl::Ref::Parent(_) | fdecl::Ref::Framework(_) | fdecl::Ref::VoidType(_) => None,
        _ => None,
    }
}

fn dynamic_children_in_collection(
    dynamic_children: &[(&str, &str)],
    collection: &str,
) -> Vec<FlyStr> {
    dynamic_children
        .iter()
        .filter_map(|(n, c)| if *c == collection { Some((*n).into()) } else { None })
        .collect()
}

fn add_offer_edges(
    source_node: Option<DependencyNode>,
    target_node: Option<DependencyNode>,
    strong_dependencies: &mut DirectedGraph<DependencyNode>,
    dynamic_children: &[(&str, &str)],
) {
    if source_node.is_none() {
        return;
    }

    let source = source_node.unwrap();

    if let DependencyNode::Collection(name) = &source {
        for child_name in dynamic_children_in_collection(dynamic_children, &name) {
            strong_dependencies.add_edge(
                DependencyNode::Child(child_name, Some(name.clone())),
                DependencyNode::Collection(name.clone()),
            );
        }
    }

    if target_node.is_none() {
        return;
    }

    let target = target_node.unwrap();

    strong_dependencies.add_edge(source.clone(), target.clone());

    if let DependencyNode::Collection(name) = target {
        for child_name in dynamic_children_in_collection(dynamic_children, &name) {
            strong_dependencies
                .add_edge(source.clone(), DependencyNode::Child(child_name, Some(name.clone())));
        }
    }
}

// Populates a dependency graph of a component's `offers` (not including dynamic offers)
fn add_dependencies_from_offers(
    strong_dependencies: &mut DirectedGraph<DependencyNode>,
    decl: &fdecl::Component,
    dynamic_children: &[(&str, &str)],
) {
    for offer in decl.offers.as_ref().map(|o| &*o as &[fdecl::Offer]).unwrap_or(&[]) {
        add_dependencies_from_offer(strong_dependencies, offer, dynamic_children);
    }
}

pub fn add_dependencies_from_offer(
    strong_dependencies: &mut DirectedGraph<DependencyNode>,
    offer: &fdecl::Offer,
    dynamic_children: &[(&str, &str)],
) {
    let (source_node, target_node) = get_dependency_from_offer(offer);
    add_offer_edges(source_node, target_node, strong_dependencies, dynamic_children);
}

pub fn get_dependency_from_offer(
    offer: &fdecl::Offer,
) -> (Option<DependencyNode>, Option<DependencyNode>) {
    let (source_node, target_node) = match offer {
        fdecl::Offer::Protocol(o) => {
            let source_node = find_offer_node(
                offer,
                o.source.as_ref(),
                &o.source_name,
                o.source_dictionary.as_ref(),
            );

            if let Some(fdecl::DependencyType::Strong) = o.dependency_type.as_ref() {
                let target_node = find_offer_node(offer, o.target.as_ref(), &None, None);

                (source_node, target_node)
            } else {
                return (None, None);
            }
        }
        #[cfg(fuchsia_api_level_at_least = "25")]
        fdecl::Offer::Dictionary(o) => {
            let source_node = find_offer_node(
                offer,
                o.source.as_ref(),
                &o.source_name,
                o.source_dictionary.as_ref(),
            );

            if let Some(fdecl::DependencyType::Strong) = o.dependency_type.as_ref() {
                let target_node = find_offer_node(offer, o.target.as_ref(), &None, None);

                (source_node, target_node)
            } else {
                return (None, None);
            }
        }
        fdecl::Offer::Directory(o) => {
            let source_node = find_offer_node(
                offer,
                o.source.as_ref(),
                &o.source_name,
                o.source_dictionary.as_ref(),
            );
            if let Some(fdecl::DependencyType::Strong) = o.dependency_type.as_ref() {
                let target_node = find_offer_node(offer, o.target.as_ref(), &None, None);

                (source_node, target_node)
            } else {
                return (None, None);
            }
        }
        fdecl::Offer::Service(o) => {
            let source_node = find_offer_node(
                offer,
                o.source.as_ref(),
                &o.source_name,
                o.source_dictionary.as_ref(),
            );

            #[cfg(fuchsia_api_level_at_least = "HEAD")]
            {
                if &fdecl::DependencyType::Strong
                    == o.dependency_type.as_ref().unwrap_or(&fdecl::DependencyType::Strong)
                {
                    let target_node = find_offer_node(offer, o.target.as_ref(), &None, None);

                    (source_node, target_node)
                } else {
                    return (None, None);
                }
            }

            #[cfg(fuchsia_api_level_less_than = "HEAD")]
            {
                let target_node = find_offer_node(offer, o.target.as_ref(), &None, None);

                (source_node, target_node)
            }
        }
        fdecl::Offer::Storage(o) => {
            let source_node = find_offer_node(offer, o.source.as_ref(), &o.source_name, None);

            let target_node = find_offer_node(offer, o.target.as_ref(), &None, None);

            (source_node, target_node)
        }
        fdecl::Offer::Runner(o) => {
            let source_node = find_offer_node(
                offer,
                o.source.as_ref(),
                &o.source_name,
                o.source_dictionary.as_ref(),
            );

            let target_node = find_offer_node(offer, o.target.as_ref(), &None, None);

            (source_node, target_node)
        }
        fdecl::Offer::Resolver(o) => {
            let source_node = find_offer_node(
                offer,
                o.source.as_ref(),
                &o.source_name,
                o.source_dictionary.as_ref(),
            );

            let target_node = find_offer_node(offer, o.target.as_ref(), &None, None);

            (source_node, target_node)
        }
        fdecl::Offer::Config(o) => {
            let source_node = find_offer_node(
                offer,
                o.source.as_ref(),
                &o.source_name,
                o.source_dictionary.as_ref(),
            );

            let target_node = find_offer_node(offer, o.target.as_ref(), &None, None);

            (source_node, target_node)
        }
        _ => return (None, None),
    };
    (source_node, target_node)
}

// Populates a dependency graph of the disjoint sets of graphs.
pub fn generate_dependency_graph(
    strong_dependencies: &mut DirectedGraph<DependencyNode>,
    decl: &fdecl::Component,
    dynamic_children: &[(&str, &str)],
) {
    add_dependencies_from_uses(strong_dependencies, decl, dynamic_children);
    add_dependencies_from_offers(strong_dependencies, decl, dynamic_children);
    add_dependencies_from_capabilities(strong_dependencies, decl);
    add_dependencies_from_environments(strong_dependencies, decl);
    add_dependencies_from_children(strong_dependencies, decl);
    add_dependencies_from_collections(strong_dependencies, decl, dynamic_children);
}
