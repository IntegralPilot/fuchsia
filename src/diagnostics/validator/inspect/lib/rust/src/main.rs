// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Error, format_err};
use fidl::endpoints::create_request_stream;
use fidl_diagnostics_validate::*;
use fidl_fuchsia_inspect::TreeMarker;
use fuchsia_async as fasync;
use fuchsia_component::server::ServiceFs;
/// Rust Puppet, receiving commands to drive the Rust Inspect library.
///
/// This code doesn't check for illegal commands such as deleting a node
/// that doesn't exist. Illegal commands should be (and can be) filtered
/// within the Validator program by running the command sequence against the
/// local ("data::Data") implementation before sending them to the puppets.
use fuchsia_inspect::Property as UsablePropertyTrait;
use fuchsia_inspect::hierarchy::*;
use fuchsia_inspect::*;
use futures::prelude::*;
use inspect_runtime::{TreeServerSendPreference, service};
use log::{error, info, warn};
use std::collections::HashMap;

#[derive(Debug)]
enum Property {
    // The names StringProperty, IntLinearHistogramProperty, etc. are built by macros such as
    // create_linear_histogram_property_fn! in src/lib/diagnostics/inspect/rust/src/lib.rs.
    // You won't find them by searching the codebase; instead search that file for macro_rules!
    // and go from there.
    // They're documented in https://fuchsia-docs.firebaseapp.com/rust/fuchsia_inspect/index.html.
    String(StringProperty),
    Int(IntProperty),
    Uint(UintProperty),
    Double(DoubleProperty),
    Bytes(BytesProperty),
    Bool(BoolProperty),
    IntArray(IntArrayProperty),
    UintArray(UintArrayProperty),
    DoubleArray(DoubleArrayProperty),
    StringArray(StringArrayProperty),
    IntLinearHistogram(IntLinearHistogramProperty),
    UintLinearHistogram(UintLinearHistogramProperty),
    DoubleLinearHistogram(DoubleLinearHistogramProperty),
    IntExponentialHistogram(IntExponentialHistogramProperty),
    UintExponentialHistogram(UintExponentialHistogramProperty),
    DoubleExponentialHistogram(DoubleExponentialHistogramProperty),
}

struct Actor {
    inspector: Inspector,
    nodes: HashMap<u32, Node>,
    properties: HashMap<u32, Property>,
    lazy_children: HashMap<u32, LazyNode>,
}

/// Handles publishing and unpublishing an inspect tree.
struct Publisher {
    inspector: Option<Inspector>,
    publish_task: Option<inspect_runtime::PublishedInspectController>,
}

impl Publisher {
    fn new() -> Self {
        Self { inspector: None, publish_task: None }
    }

    fn publish(&mut self, inspector: Inspector) {
        self.inspector = Some(inspector.clone());
        self.publish_task =
            inspect_runtime::publish(&inspector, inspect_runtime::PublishOptions::default());
    }
}

impl Actor {
    fn new(inspector: Inspector) -> Actor {
        Actor {
            inspector,
            nodes: HashMap::new(),
            properties: HashMap::new(),
            lazy_children: HashMap::new(),
        }
    }

    fn act(&mut self, action: Action) -> Result<(), Error> {
        match action {
            Action::CreateNode(CreateNode { parent, id, name }) => {
                self.nodes.insert(id, self.find_parent(parent)?.create_child(name));
            }
            Action::DeleteNode(DeleteNode { id }) => {
                self.nodes.remove(&id);
            }
            Action::CreateNumericProperty(CreateNumericProperty { parent, id, name, value }) => {
                self.properties.insert(
                    id,
                    match value {
                        Value::IntT(n) => {
                            Property::Int(self.find_parent(parent)?.create_int(name, n))
                        }
                        Value::UintT(n) => {
                            Property::Uint(self.find_parent(parent)?.create_uint(name, n))
                        }
                        Value::DoubleT(n) => {
                            Property::Double(self.find_parent(parent)?.create_double(name, n))
                        }
                        unknown => return Err(format_err!("Unknown number type {:?}", unknown)),
                    },
                );
            }
            Action::CreateBytesProperty(CreateBytesProperty { parent, id, name, value }) => {
                self.properties.insert(
                    id,
                    Property::Bytes(self.find_parent(parent)?.create_bytes(name, value)),
                );
            }
            Action::CreateStringProperty(CreateStringProperty { parent, id, name, value }) => {
                self.properties.insert(
                    id,
                    Property::String(self.find_parent(parent)?.create_string(name, value)),
                );
            }
            Action::CreateBoolProperty(CreateBoolProperty { parent, id, name, value }) => {
                self.properties
                    .insert(id, Property::Bool(self.find_parent(parent)?.create_bool(name, value)));
            }
            Action::DeleteProperty(DeleteProperty { id }) => {
                self.properties.remove(&id);
            }
            Action::SetNumber(SetNumber { id, value }) => {
                match (self.find_property(id)?, value) {
                    (Property::Int(p), Value::IntT(v)) => p.set(v),
                    (Property::Uint(p), Value::UintT(v)) => p.set(v),
                    (Property::Double(p), Value::DoubleT(v)) => p.set(v),
                    unexpected => {
                        return Err(format_err!("Illegal types {:?} for SetNumber", unexpected));
                    }
                };
            }
            Action::AddNumber(AddNumber { id, value }) => {
                match (self.find_property(id)?, value) {
                    (Property::Int(p), Value::IntT(v)) => {
                        p.add(v);
                    }
                    (Property::Uint(p), Value::UintT(v)) => {
                        p.add(v);
                    }
                    (Property::Double(p), Value::DoubleT(v)) => {
                        p.add(v);
                    }
                    unexpected => {
                        return Err(format_err!("Illegal types {:?} for AddNumber", unexpected));
                    }
                };
            }
            Action::SubtractNumber(SubtractNumber { id, value }) => {
                match (self.find_property(id)?, value) {
                    (Property::Int(p), Value::IntT(v)) => {
                        p.subtract(v);
                    }
                    (Property::Uint(p), Value::UintT(v)) => {
                        p.subtract(v);
                    }
                    (Property::Double(p), Value::DoubleT(v)) => {
                        p.subtract(v);
                    }
                    unexpected => {
                        return Err(format_err!(
                            "Illegal types {:?} for SubtractNumber",
                            unexpected
                        ));
                    }
                };
            }
            Action::SetString(SetString { id, value }) => match self.find_property(id)? {
                Property::String(p) => p.set(&value),
                unexpected => {
                    return Err(format_err!("Illegal property {:?} for SetString", unexpected));
                }
            },
            Action::SetBytes(SetBytes { id, value }) => match self.find_property(id)? {
                Property::Bytes(p) => p.set(&value),
                unexpected => {
                    return Err(format_err!("Illegal property {:?} for SetBytes", unexpected));
                }
            },
            Action::SetBool(SetBool { id, value }) => match self.find_property(id)? {
                Property::Bool(p) => p.set(value),
                unexpected => {
                    return Err(format_err!("Illegal property {:?} for SetBool", unexpected));
                }
            },
            Action::CreateArrayProperty(CreateArrayProperty {
                parent,
                id,
                name,
                slots,
                value_type,
            }) => {
                self.properties.insert(
                    id,
                    match value_type {
                        ValueType::Int => Property::IntArray(
                            self.find_parent(parent)?.create_int_array(name, slots as usize),
                        ),
                        ValueType::Uint => Property::UintArray(
                            self.find_parent(parent)?.create_uint_array(name, slots as usize),
                        ),
                        ValueType::Double => Property::DoubleArray(
                            self.find_parent(parent)?.create_double_array(name, slots as usize),
                        ),
                        ValueType::String => Property::StringArray({
                            let array: StringArrayProperty =
                                self.find_parent(parent)?.create_string_array(name, slots as usize);
                            array
                        }),
                        unknown => return Err(format_err!("Unknown value type {unknown:?}")),
                    },
                );
            }
            Action::ArraySet(ArraySet { id, index, value }) => {
                match (self.find_property(id)?, value) {
                    (Property::IntArray(p), Value::IntT(v)) => p.set(index as usize, v),
                    (Property::UintArray(p), Value::UintT(v)) => p.set(index as usize, v),
                    (Property::DoubleArray(p), Value::DoubleT(v)) => p.set(index as usize, v),
                    (Property::StringArray(p), Value::StringT(v)) => {
                        p.set(index as usize, v);
                    }
                    unexpected => {
                        return Err(format_err!("Illegal types {:?} for ArraySet", unexpected));
                    }
                };
            }
            Action::ArrayAdd(ArrayAdd { id, index, value }) => {
                match (self.find_property(id)?, value) {
                    (Property::IntArray(p), Value::IntT(v)) => p.add(index as usize, v),
                    (Property::UintArray(p), Value::UintT(v)) => p.add(index as usize, v),
                    (Property::DoubleArray(p), Value::DoubleT(v)) => p.add(index as usize, v),
                    unexpected => {
                        return Err(format_err!("Illegal types {:?} for ArrayAdd", unexpected));
                    }
                };
            }
            Action::ArraySubtract(ArraySubtract { id, index, value }) => {
                match (self.find_property(id)?, value) {
                    (Property::IntArray(p), Value::IntT(v)) => p.subtract(index as usize, v),
                    (Property::UintArray(p), Value::UintT(v)) => p.subtract(index as usize, v),
                    (Property::DoubleArray(p), Value::DoubleT(v)) => p.subtract(index as usize, v),
                    unexpected => {
                        return Err(format_err!(
                            "Illegal types {:?} for ArraySubtract",
                            unexpected
                        ));
                    }
                };
            }
            Action::CreateLinearHistogram(CreateLinearHistogram {
                parent,
                id,
                name,
                floor,
                step_size,
                buckets,
            }) => {
                let buckets = buckets as usize;
                self.properties.insert(
                    id,
                    match (floor, step_size) {
                        (Value::IntT(floor), Value::IntT(step_size)) => {
                            Property::IntLinearHistogram(
                                self.find_parent(parent)?.create_int_linear_histogram(
                                    name,
                                    LinearHistogramParams { floor, step_size, buckets },
                                ),
                            )
                        }
                        (Value::UintT(floor), Value::UintT(step_size)) => {
                            Property::UintLinearHistogram(
                                self.find_parent(parent)?.create_uint_linear_histogram(
                                    name,
                                    LinearHistogramParams { floor, step_size, buckets },
                                ),
                            )
                        }
                        (Value::DoubleT(floor), Value::DoubleT(step_size)) => {
                            Property::DoubleLinearHistogram(
                                self.find_parent(parent)?.create_double_linear_histogram(
                                    name,
                                    LinearHistogramParams { floor, step_size, buckets },
                                ),
                            )
                        }
                        unexpected => {
                            return Err(format_err!(
                                "Illegal types {:?} for CreateLinearHistogram",
                                unexpected
                            ));
                        }
                    },
                );
            }
            Action::CreateExponentialHistogram(CreateExponentialHistogram {
                parent,
                id,
                name,
                floor,
                initial_step,
                step_multiplier,
                buckets,
            }) => {
                let buckets = buckets as usize;
                self.properties.insert(
                    id,
                    match (floor, initial_step, step_multiplier) {
                        (
                            Value::IntT(floor),
                            Value::IntT(initial_step),
                            Value::IntT(step_multiplier),
                        ) => Property::IntExponentialHistogram(
                            self.find_parent(parent)?.create_int_exponential_histogram(
                                name,
                                ExponentialHistogramParams {
                                    floor,
                                    initial_step,
                                    step_multiplier,
                                    buckets,
                                },
                            ),
                        ),
                        (
                            Value::UintT(floor),
                            Value::UintT(initial_step),
                            Value::UintT(step_multiplier),
                        ) => Property::UintExponentialHistogram(
                            self.find_parent(parent)?.create_uint_exponential_histogram(
                                name,
                                ExponentialHistogramParams {
                                    floor,
                                    initial_step,
                                    step_multiplier,
                                    buckets,
                                },
                            ),
                        ),
                        (
                            Value::DoubleT(floor),
                            Value::DoubleT(initial_step),
                            Value::DoubleT(step_multiplier),
                        ) => Property::DoubleExponentialHistogram(
                            self.find_parent(parent)?.create_double_exponential_histogram(
                                name,
                                ExponentialHistogramParams {
                                    floor,
                                    initial_step,
                                    step_multiplier,
                                    buckets,
                                },
                            ),
                        ),
                        unexpected => {
                            return Err(format_err!(
                                "Illegal types {:?} for CreateExponentialHistogram",
                                unexpected
                            ));
                        }
                    },
                );
            }
            Action::Insert(Insert { id, value }) => {
                match (self.find_property(id)?, value) {
                    (Property::IntLinearHistogram(p), Value::IntT(v)) => p.insert(v),
                    (Property::UintLinearHistogram(p), Value::UintT(v)) => p.insert(v),
                    (Property::DoubleLinearHistogram(p), Value::DoubleT(v)) => p.insert(v),
                    (Property::IntExponentialHistogram(p), Value::IntT(v)) => p.insert(v),
                    (Property::UintExponentialHistogram(p), Value::UintT(v)) => p.insert(v),
                    (Property::DoubleExponentialHistogram(p), Value::DoubleT(v)) => p.insert(v),
                    unexpected => {
                        return Err(format_err!("Illegal types {:?} for Insert", unexpected));
                    }
                };
            }
            Action::InsertMultiple(InsertMultiple { id, value, count }) => {
                match (self.find_property(id)?, value) {
                    (Property::IntLinearHistogram(p), Value::IntT(v)) => {
                        p.insert_multiple(v, count as usize)
                    }
                    (Property::UintLinearHistogram(p), Value::UintT(v)) => {
                        p.insert_multiple(v, count as usize)
                    }
                    (Property::DoubleLinearHistogram(p), Value::DoubleT(v)) => {
                        p.insert_multiple(v, count as usize)
                    }
                    (Property::IntExponentialHistogram(p), Value::IntT(v)) => {
                        p.insert_multiple(v, count as usize)
                    }
                    (Property::UintExponentialHistogram(p), Value::UintT(v)) => {
                        p.insert_multiple(v, count as usize)
                    }
                    (Property::DoubleExponentialHistogram(p), Value::DoubleT(v)) => {
                        p.insert_multiple(v, count as usize)
                    }
                    unexpected => {
                        return Err(format_err!(
                            "Illegal types {:?} for InsertMultiple",
                            unexpected
                        ));
                    }
                };
            }
            unexpected => {
                // "Illegal" is the appropriate response here, not "Unimplemented".
                // Known-Unimplemented actions should be matched explicitly.
                return Err(format_err!("Unexpected action {:?}", unexpected));
            }
        };
        Ok(())
    }

    fn act_lazy(&mut self, lazy_action: LazyAction) -> Result<(), Error> {
        match lazy_action {
            LazyAction::CreateLazyNode(CreateLazyNode {
                parent,
                id,
                name,
                disposition,
                actions,
            }) => {
                let parent = self.find_parent(parent)?;
                let lazy_child = Self::create_lazy_node(parent, name, disposition, actions)?;
                self.lazy_children.insert(id, lazy_child);
            }
            LazyAction::DeleteLazyNode(DeleteLazyNode { id }) => {
                self.lazy_children.remove(&id);
            }
            _ => {
                return Err(format_err!("Unknown lazy action {:?}", lazy_action));
            }
        }
        Ok(())
    }

    fn act_lazy_thread_local(&mut self, lazy_action: LazyAction) -> Result<(), Error> {
        match lazy_action {
            LazyAction::CreateLazyNode(CreateLazyNode {
                parent,
                id,
                name,
                disposition,
                actions,
            }) => {
                let parent = self.find_parent(parent)?;
                let lazy_child =
                    Self::create_thread_unsafe_lazy_node(parent, name, disposition, actions)?;
                self.lazy_children.insert(id, lazy_child);
            }
            LazyAction::DeleteLazyNode(DeleteLazyNode { id }) => {
                self.lazy_children.remove(&id);
            }
            _ => {
                return Err(format_err!("Unknown lazy action {:?}", lazy_action));
            }
        }
        Ok(())
    }

    fn find_parent(&self, id: u32) -> Result<&'_ Node, Error> {
        if id == ROOT_ID {
            Ok(self.inspector.root())
        } else {
            self.nodes.get(&id).ok_or_else(|| format_err!("Node {} not found", id))
        }
    }

    fn find_property(&self, id: u32) -> Result<&Property, Error> {
        self.properties.get(&id).ok_or_else(|| format_err!("Property {} not found", id))
    }

    fn create_lazy_node(
        parent: &Node,
        name: String,
        disposition: LinkDisposition,
        actions: Vec<Action>,
    ) -> Result<LazyNode, Error> {
        let mut actor = Actor::new(Inspector::default());
        for action in actions.into_iter() {
            if let Err(err) = actor.act(action) {
                return Err(format_err!("Failed to perform action on lazy node: {:?}", err));
            }
        }

        let callback = move || {
            let _ = &actor;
            let clone = actor.inspector.clone();
            async move { Ok(clone) }.boxed()
        };

        Ok(match disposition {
            LinkDisposition::Child => parent.create_lazy_child(&name, callback),
            LinkDisposition::Inline => parent.create_lazy_values(&name, callback),
        })
    }

    fn create_thread_unsafe_lazy_node(
        parent: &Node,
        name: String,
        disposition: LinkDisposition,
        actions: Vec<Action>,
    ) -> Result<LazyNode, Error> {
        let mut actor = Actor::new(Inspector::default());
        for action in actions.into_iter() {
            if let Err(err) = actor.act(action) {
                return Err(format_err!("Failed to perform action on lazy node: {:?}", err));
            }
        }

        let not_send = std::rc::Rc::new(());
        let callback = move || {
            let x = not_send.clone();
            let _ = &actor;
            let clone = actor.inspector.clone();
            async move {
                let _ = x.clone();
                Ok(clone)
            }
            .boxed_local()
        };

        Ok(match disposition {
            LinkDisposition::Child => parent.create_lazy_child_with_thread_local(&name, callback),
            LinkDisposition::Inline => parent.create_lazy_values_with_thread_local(&name, callback),
        })
    }
}

fn new_inspector(params: &InitializationParams) -> Inspector {
    match params.vmo_size {
        Some(size) => Inspector::new(InspectorConfig::default().size(size as usize)),
        None => Inspector::default(),
    }
}

async fn run_driver_service(
    mut stream: InspectPuppetRequestStream,
    mut publisher: Publisher,
) -> Result<(), Error> {
    let mut actor_maybe: Option<Actor> = None;
    let scope = fasync::Scope::new_with_name("validator puppet service");
    while let Some(event) = stream.try_next().await? {
        match event {
            InspectPuppetRequest::GetConfig { responder, .. } => {
                responder.send("rust-puppet", Options::default()).ok();
            }
            InspectPuppetRequest::Initialize { params, responder } => {
                if actor_maybe.is_some() {
                    responder.send(None, TestResult::Illegal).context("Double initialize call")?;
                } else {
                    let actor = Actor::new(new_inspector(&params));
                    responder
                        .send(
                            actor.inspector.duplicate_vmo().map(|v| v.into_handle()),
                            TestResult::Ok,
                        )
                        .context("responding to initialize")?;
                    actor_maybe = Some(actor);
                }
            }
            InspectPuppetRequest::InitializeTree { params, responder } => {
                let actor = Actor::new(new_inspector(&params));
                let (tree, request_stream) = create_request_stream::<TreeMarker>();
                service::spawn_tree_server_with_stream(
                    actor.inspector.clone(),
                    TreeServerSendPreference::default(),
                    request_stream,
                    &scope,
                );
                responder.send(Some(tree), TestResult::Ok)?;
                actor_maybe = Some(actor);
            }
            InspectPuppetRequest::Act { action, responder } => {
                let result = if let Some(a) = &mut actor_maybe {
                    match a.act(action) {
                        Ok(()) => TestResult::Ok,
                        Err(error) => {
                            warn!("Act saw illegal condition {:?}", error);
                            TestResult::Illegal
                        }
                    }
                } else {
                    TestResult::Illegal
                };
                responder.send(result)?;
            }
            InspectPuppetRequest::ActLazy { lazy_action, responder } => {
                let result = if let Some(a) = &mut actor_maybe {
                    match a.act_lazy(lazy_action) {
                        Ok(()) => TestResult::Ok,
                        Err(error) => {
                            warn!("ActLazy saw illegal condition {:?}", error);
                            TestResult::Illegal
                        }
                    }
                } else {
                    TestResult::Illegal
                };
                responder.send(result)?;
            }
            InspectPuppetRequest::ActLazyThreadLocal { lazy_action, responder } => {
                let result = if let Some(a) = &mut actor_maybe {
                    match a.act_lazy_thread_local(lazy_action) {
                        Ok(()) => TestResult::Ok,
                        Err(error) => {
                            warn!("ActLazyThreadLocal saw illegal condition {:?}", error);
                            TestResult::Illegal
                        }
                    }
                } else {
                    TestResult::Illegal
                };
                responder.send(result)?;
            }
            InspectPuppetRequest::Publish { responder } => match &actor_maybe {
                Some(actor) => {
                    publisher.publish(actor.inspector.clone());
                    responder.send(TestResult::Ok)?;
                }
                None => {
                    responder.send(TestResult::Illegal)?;
                }
            },
            InspectPuppetRequest::_UnknownMethod { .. } => {}
        }
    }
    Ok(())
}

enum IncomingService {
    InspectPuppet(InspectPuppetRequestStream),
    // ... more services here
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    info!("Puppet starting");

    let mut fs = ServiceFs::new_local();
    fs.dir("svc").add_fidl_service(IncomingService::InspectPuppet);

    fs.take_and_serve_directory_handle()?;

    // Set concurrent > 1, otherwise additional requests hang on the completion of the InspectPuppet
    // service.
    const MAX_CONCURRENT: usize = 4;
    let fut = fs.for_each_concurrent(MAX_CONCURRENT, |IncomingService::InspectPuppet(stream)| {
        run_driver_service(stream, Publisher::new())
            .unwrap_or_else(|e| error!("ERROR in puppet's main: {:?}", e))
    });

    fut.await;
    Ok(())
}
