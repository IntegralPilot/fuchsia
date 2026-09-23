// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use super::PUPPET_MONIKER;
use super::data::{self, Data, LazyNode};
use super::metrics::Metrics;
use anyhow::{Error, format_err};
use fidl_diagnostics_validate as validate;
use fidl_fuchsia_inspect as fidl_inspect;
use fuchsia_component::client as fclient;
use serde::Serialize;
use zx::{self as zx, Vmo};

pub const VMO_SIZE: u64 = 4096;

#[derive(Debug)]
pub struct Config {
    pub diff_type: DiffType,
    pub printable_name: String,
    pub has_runner_node: bool,
    pub test_archive: bool,
}

/// When reporting a discrepancy between local and remote Data trees, should the output include:
/// - The full rendering of both trees?
/// - The condensed diff between the trees? (This may still be quite large.)
/// - Both full and condensed renderings?
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub enum DiffType {
    #[default]
    Full,
    Diff,
    Both,
}

impl From<Option<validate::DiffType>> for DiffType {
    fn from(original: Option<validate::DiffType>) -> Self {
        match original {
            Some(validate::DiffType::Diff) => Self::Diff,
            Some(validate::DiffType::Both) => Self::Both,
            _ => Self::Full,
        }
    }
}

pub struct Puppet {
    pub vmo: Vmo,
    // Need to remember the connection to avoid dropping the VMO
    connection: Connection,
    // A printable name for output to the user.
    pub config: Config,
}

impl Puppet {
    pub async fn apply(
        &mut self,
        action: &mut validate::Action,
    ) -> Result<validate::TestResult, Error> {
        Ok(self.connection.fidl.act(action).await?)
    }

    pub async fn apply_lazy(
        &mut self,
        lazy_action: &mut validate::LazyAction,
    ) -> Result<validate::TestResult, Error> {
        match &self.connection.root_link_channel {
            Some(_) => Ok(self.connection.fidl.act_lazy(lazy_action).await?),
            None => Ok(validate::TestResult::Unimplemented),
        }
    }

    pub async fn act_lazy_thread_local(
        &mut self,
        lazy_action: &mut validate::LazyAction,
    ) -> Result<validate::TestResult, Error> {
        Ok(self.connection.fidl.act_lazy_thread_local(lazy_action).await?)
    }

    pub async fn publish(&mut self) -> Result<validate::TestResult, Error> {
        Ok(self.connection.fidl.publish().await?)
    }

    pub async fn connect() -> Result<Self, Error> {
        Puppet::initialize_with_connection(Connection::connect().await?).await
    }

    pub(crate) async fn shutdown(self) {
        let lifecycle_controller =
            fclient::connect_to_protocol::<fidl_fuchsia_sys2::LifecycleControllerMarker>().unwrap();
        lifecycle_controller.stop_instance(&format!("./{PUPPET_MONIKER}")).await.unwrap().unwrap();
    }

    /// Get the printable name associated with this puppet/test
    pub fn printable_name(&self) -> &str {
        &self.config.printable_name
    }

    #[cfg(test)]
    pub async fn connect_local(local_fidl: validate::InspectPuppetProxy) -> Result<Puppet, Error> {
        let mut puppet = Puppet::initialize_with_connection(Connection::new(local_fidl)).await?;
        puppet.config.test_archive = false;
        Ok(puppet)
    }

    async fn initialize_with_connection(mut connection: Connection) -> Result<Puppet, Error> {
        Ok(Puppet {
            vmo: connection.initialize_vmo().await?,
            config: connection.get_config().await?,
            connection,
        })
    }

    pub async fn read_data(&self) -> Result<Data, Error> {
        Ok(match &self.connection.root_link_channel {
            None => data::Scanner::try_from(&self.vmo)?.data(),
            Some(root_link_channel) => {
                let vmo_tree = LazyNode::new(root_link_channel.clone()).await?;
                data::Scanner::try_from(vmo_tree)?.data()
            }
        })
    }

    pub fn metrics(&self) -> Result<Metrics, Error> {
        Ok(data::Scanner::try_from(&self.vmo)?.metrics())
    }
}

struct Connection {
    fidl: validate::InspectPuppetProxy,
    // Connection to Tree FIDL if Puppet supports it.
    // Puppets can add support by implementing InitializeTree method.
    root_link_channel: Option<fidl_inspect::TreeProxy>,
}

impl Connection {
    async fn connect() -> Result<Self, Error> {
        let puppet_fidl = fclient::connect_to_protocol::<validate::InspectPuppetMarker>().unwrap();
        Ok(Self::new(puppet_fidl))
    }

    async fn get_config(&self) -> Result<Config, Error> {
        let (printable_name, opts) = self.fidl.get_config().await?;
        Ok(Config {
            diff_type: opts.diff_type.into(),
            printable_name,
            has_runner_node: opts.has_runner_node.unwrap_or(false),
            test_archive: true,
        })
    }

    async fn fetch_link_channel(
        fidl: &validate::InspectPuppetProxy,
    ) -> Option<fidl_inspect::TreeProxy> {
        let params =
            validate::InitializationParams { vmo_size: Some(VMO_SIZE), ..Default::default() };
        let response = fidl.initialize_tree(&params).await;
        if let Ok((Some(tree_client_end), validate::TestResult::Ok)) = response {
            Some(tree_client_end.into_proxy())
        } else {
            None
        }
    }

    async fn get_vmo_handle(channel: &fidl_inspect::TreeProxy) -> Result<Vmo, Error> {
        let tree_content = channel.get_content().await?;
        let buffer =
            tree_content.buffer.ok_or_else(|| format_err!("Buffer doesn't contain VMO"))?;
        Ok(buffer.vmo)
    }

    fn new(fidl: validate::InspectPuppetProxy) -> Self {
        Self { fidl, root_link_channel: None }
    }

    async fn initialize_vmo(&mut self) -> Result<Vmo, Error> {
        self.root_link_channel = Self::fetch_link_channel(&self.fidl).await;
        match &self.root_link_channel {
            Some(root_link_channel) => Self::get_vmo_handle(root_link_channel).await,
            None => {
                let params = validate::InitializationParams {
                    vmo_size: Some(VMO_SIZE),
                    ..Default::default()
                };
                let handle: Option<zx::NullableHandle>;
                let out = self.fidl.initialize(&params).await?;
                if let (Some(out_handle), _) = out {
                    handle = Some(out_handle);
                } else {
                    return Err(format_err!("Didn't get a VMO handle"));
                }
                match handle {
                    Some(unwrapped_handle) => Ok(Vmo::from(unwrapped_handle)),
                    None => Err(format_err!("Failed to unwrap handle")),
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::create_node;
    use anyhow::Context as _;
    use fidl::endpoints::{RequestStream, ServerEnd, create_proxy};
    use fidl_diagnostics_validate::{
        Action, CreateNode, CreateNumericProperty, InspectPuppetMarker, InspectPuppetRequest,
        InspectPuppetRequestStream, Options, ROOT_ID, TestResult, Value,
    };
    use fuchsia_async as fasync;
    use fuchsia_inspect::{Inspector, InspectorConfig, IntProperty, Node};
    use futures::prelude::*;
    use log::info;
    use std::collections::HashMap;

    #[fuchsia::test]
    async fn test_fidl_loopback() -> Result<(), Error> {
        let mut puppet = local_incomplete_puppet().await?;
        assert_eq!(puppet.vmo.get_size().unwrap(), VMO_SIZE);
        let tree = puppet.read_data().await?;
        assert_eq!(tree.to_string(), "root ->".to_string());
        let mut data = Data::new();
        tree.compare(&data, DiffType::Full)?;
        let mut action = create_node!(parent: ROOT_ID, id: 1, name: "child");
        puppet.apply(&mut action).await?;
        data.apply(&action)?;
        let tree = data::Scanner::try_from(&puppet.vmo)?.data();
        assert_eq!(tree.to_string(), "root ->\n> child ->".to_string());
        tree.compare(&data, DiffType::Full)?;
        Ok(())
    }

    // This is a partial implementation.
    // All it can do is initialize, and then create nodes and int properties (which it
    // will hold forever). Trying to create a uint property will return Unimplemented.
    // Other actions will give various kinds of incorrect results.
    pub(crate) async fn local_incomplete_puppet() -> Result<Puppet, Error> {
        let (client_end, server_end) = create_proxy();
        spawn_local_puppet(server_end).await;
        Puppet::connect_local(client_end).await
    }

    async fn spawn_local_puppet(server_end: ServerEnd<InspectPuppetMarker>) {
        fasync::Task::spawn(
            async move {
                // Inspector must be remembered so its VMO persists
                let mut inspector_maybe: Option<Inspector> = None;
                let mut nodes: HashMap<u32, Node> = HashMap::new();
                let mut properties: HashMap<u32, IntProperty> = HashMap::new();
                let server_chan = fasync::Channel::from_channel(server_end.into_channel());
                let mut stream = InspectPuppetRequestStream::from_channel(server_chan);
                while let Some(event) = stream.try_next().await? {
                    match event {
                        InspectPuppetRequest::GetConfig { responder } => {
                            responder.send("*Local*", Options::default()).ok();
                        }
                        InspectPuppetRequest::Initialize { params, responder } => {
                            let inspector = match params.vmo_size {
                                Some(size) => {
                                    Inspector::new(InspectorConfig::default().size(size as usize))
                                }
                                None => Inspector::default(),
                            };
                            responder
                                .send(
                                    inspector.duplicate_vmo().map(|v| v.into_handle()),
                                    TestResult::Ok,
                                )
                                .context("responding to initialize")?;
                            inspector_maybe = Some(inspector);
                        }
                        InspectPuppetRequest::Act { action, responder } => match action {
                            Action::CreateNode(CreateNode { parent, id, name }) => {
                                if let Some(ref inspector) = inspector_maybe {
                                    let parent_node = if parent == ROOT_ID {
                                        inspector.root()
                                    } else {
                                        nodes.get(&parent).unwrap()
                                    };
                                    let new_child = parent_node.create_child(name);
                                    nodes.insert(id, new_child);
                                }
                                responder.send(TestResult::Ok)?;
                            }
                            Action::CreateNumericProperty(CreateNumericProperty {
                                parent,
                                id,
                                name,
                                value: Value::IntT(value),
                            }) => {
                                inspector_maybe.as_ref().map(|i| {
                                    let parent_node = if parent == 0 {
                                        i.root()
                                    } else {
                                        nodes.get(&parent).unwrap()
                                    };
                                    properties.insert(id, parent_node.create_int(name, value))
                                });
                                responder.send(TestResult::Ok)?;
                            }
                            Action::CreateNumericProperty(CreateNumericProperty {
                                value: Value::UintT(_),
                                ..
                            }) => {
                                responder.send(TestResult::Unimplemented)?;
                            }

                            _ => responder.send(TestResult::Illegal)?,
                        },
                        InspectPuppetRequest::InitializeTree { params: _, responder } => {
                            responder.send(None, TestResult::Unimplemented)?;
                        }
                        InspectPuppetRequest::ActLazy { lazy_action: _, responder } => {
                            responder.send(TestResult::Unimplemented)?;
                        }
                        InspectPuppetRequest::Publish { responder } => {
                            responder.send(TestResult::Unimplemented)?;
                        }
                        InspectPuppetRequest::ActLazyThreadLocal { responder, .. } => {
                            responder.send(TestResult::Unimplemented)?;
                        }
                        InspectPuppetRequest::_UnknownMethod { .. } => {}
                    }
                }
                Ok(())
            }
            .unwrap_or_else(|e: anyhow::Error| info!("error running validate interface: {:?}", e)),
        )
        .detach();
    }
}
