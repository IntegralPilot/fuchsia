// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::{Incoming, Node};
use fuchsia_async::ScopeHandle;
use fuchsia_component::server::{ServiceFs, ServiceObjTrait};
use fuchsia_component_config::Config;
use fuchsia_inspect::Inspector;
use inspect_runtime::PublishOptions;
use log::error;
use namespace::Namespace;
use zx::Status;

use fdf::AsyncDispatcher;
use fidl_fuchsia_driver_framework::DriverStartArgs;

pub use fuchsia_inspect_contrib::content_publisher as inspect_publisher;

/// The context arguments passed to the driver in its start arguments.
#[non_exhaustive]
pub struct DriverContext {
    /// A reference to the root [`fdf::Dispatcher`] for this driver.
    pub root_dispatcher: AsyncDispatcher,
    /// The original [`DriverStartArgs`] passed in as start arguments, minus any parts that were
    /// used to construct other elements of [`Self`].
    pub start_args: DriverStartArgs,
    /// The incoming namespace constructed from [`DriverStartArgs::incoming`]. Since producing this
    /// consumes the incoming namespace from [`Self::start_args`], that will always be [`None`].
    pub incoming: Incoming,
}

impl DriverContext {
    /// Binds the node proxy client end from the start args into a [`NodeProxy`] that can be used
    /// to add child nodes. Dropping this proxy will result in the node being removed and the
    /// driver starting shutdown, so it should be bound and stored in your driver object in its
    /// [`crate::Driver::start`] method.
    ///
    /// After calling this, [`DriverStartArgs::node`] in [`Self::start_args`] will be `None`.
    ///
    /// Returns [`Status::INVALID_ARGS`] if the node client end is not present in the start
    /// arguments.
    pub fn take_node(&mut self) -> Result<Node, Status> {
        let node_client = self.start_args.node.take().ok_or(Status::INVALID_ARGS)?;
        Ok(Node::from(node_client.into_proxy()))
    }

    /// Returns the component config.
    ///
    /// After calling this, [`DriverStartArgs::config`] in [`Self::start_args`] will be `None`.
    ///
    /// Returns [`Status::INVALID_ARGS`] if the config is not present in the start arguments.
    pub fn take_config<C: Config>(&mut self) -> Result<C, Status> {
        let vmo = self.start_args.config.take().ok_or(Status::INVALID_ARGS)?;
        Ok(Config::from_vmo(&vmo).expect("Config VMO handle must be valid."))
    }

    /// Serves the given [`ServiceFs`] on the node's outgoing directory. This can only be called
    /// once, and after this the [`DriverStartArgs::outgoing_dir`] member will be [`None`].
    ///
    /// Logs an error and returns [`Status::INVALID_ARGS`] if the outgoing directory server end is
    /// not present in the start arguments, or [`Status::INTERNAL`] if serving the connection
    /// failed.
    pub fn serve_outgoing<O: ServiceObjTrait>(
        &mut self,
        outgoing_fs: &mut ServiceFs<O>,
    ) -> Result<(), Status> {
        let Some(outgoing_dir) = self.start_args.outgoing_dir.take() else {
            error!("Tried to serve on outgoing directory but it wasn't available");
            return Err(Status::INVALID_ARGS);
        };
        outgoing_fs.serve_connection(outgoing_dir).map_err(|err| {
            error!("Failed to serve outgoing directory: {err}");
            Status::INTERNAL
        })?;

        Ok(())
    }

    /// Spawns a server handling `fuchsia.inspect.Tree` requests and a handle
    /// to the `fuchsia.inspect.Tree` is published using `fuchsia.inspect.InspectSink`.
    ///
    /// Whenever the client wishes to stop publishing Inspect, the Controller may be dropped.
    pub fn publish_inspect(&self, inspector: &Inspector, scope: ScopeHandle) -> Result<(), Status> {
        let client = self.incoming.connect_protocol().map_err(|err| {
            error!("Error connecting to inspect : {err}");
            Status::INTERNAL
        })?;

        let task = inspect_runtime::publish(
            inspector,
            PublishOptions::default().on_inspect_sink_client(client),
        )
        .ok_or(Status::INTERNAL)?;

        scope.spawn_local(task);

        Ok(())
    }

    /// Returns an inspect content publisher that acts as a stream for requests for inspect data.
    pub fn inspect_content_publisher(&self) -> Result<inspect_publisher::ContentPublisher, Status> {
        let inspect_sink_client = self.incoming.connect_protocol().map_err(|err| {
            error!("Error connecting to inspect : {err}");
            Status::INTERNAL
        })?;

        let options = inspect_publisher::PublishOptions { inspect_sink_client };
        inspect_publisher::content_publisher(options).map_err(|err| {
            error!("Error creating inspect content publisher: {err}");
            Status::INTERNAL
        })
    }

    /// Returns the VMAR which the driver can use to map memory.
    ///
    /// If the driver was not provided with an explicit VMAR in its start arguments, the root VMAR
    /// is returned.
    pub fn vmar(&self) -> zx::Unowned<'_, zx::Vmar> {
        // NB: We can't use `map_or_else` here, because the compiler gets confused about lifetimes
        // when attempting to unify the types of the two function arguments.
        if let Some(vmar) = self.start_args.vmar.as_ref().map(zx::Unowned::new) {
            vmar
        } else {
            fuchsia_runtime::vmar_root_self()
        }
    }

    pub(crate) fn new(
        root_dispatcher: AsyncDispatcher,
        mut start_args: DriverStartArgs,
    ) -> Result<Self, Status> {
        let incoming_namespace: Namespace = start_args
            .incoming
            .take()
            .unwrap_or_default()
            .try_into()
            .map_err(|_| Status::INVALID_ARGS)?;
        let incoming = Incoming::from(incoming_namespace);
        Ok(DriverContext { root_dispatcher, start_args, incoming })
    }

    pub(crate) fn start_logging(&self, driver_name: &str) -> Result<(), Status> {
        let log_client = match self.incoming.connect_protocol() {
            Ok(log_client) => log_client,
            Err(err) => {
                eprintln!(
                    "Error connecting to log sink proxy at driver startup: {err}. Continuing without logging."
                );
                return Ok(());
            }
        };

        if let Err(e) = driver_diagnostics_log::initialize(
            driver_diagnostics_log::PublishOptions::default()
                .use_log_sink(log_client)
                .tags(&["driver", driver_name]),
        ) {
            eprintln!("Error initializing logging at driver startup: {e}");
        }
        Ok(())
    }
}
