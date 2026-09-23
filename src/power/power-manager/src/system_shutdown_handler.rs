// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::PowerManagerError;
use crate::message::{Message, MessageReturn};
use crate::node::Node;
use anyhow::{Error, format_err};
use async_trait::async_trait;
use fidl_fuchsia_hardware_power_statecontrol as fpowercontrol;
use fuchsia_inspect::{self as inspect, Property};
use log::*;
use serde_derive::Deserialize;
use serde_json as json;
use std::rc::Rc;
/// Node: SystemShutdownHandler
///
/// Summary: Provides a mechanism for the Power Manager to shut down the system due to either
/// extreme temperature or other reasons.
///
/// Handles Messages:
///     - SystemShutdown
///
/// FIDL dependencies:
///     - fuchsia.hardware.power.statecontrol.Admin: the node uses this protocol in case the
///       temperature exceeds a threshold. If the call fails, Power Manager will force a shutdown
///       by terminating itself.

/// A builder for constructing the SystemShutdownHandler node.
pub struct SystemShutdownHandlerBuilder<'a> {
    shutdown_shim_proxy: Option<fpowercontrol::AdminProxy>,
    force_shutdown_func: Box<dyn Fn()>,
    inspect_root: Option<&'a inspect::Node>,
    poweroff_for_shutdown: bool,
}

impl<'a> SystemShutdownHandlerBuilder<'a> {
    pub fn new() -> Self {
        Self {
            shutdown_shim_proxy: None,
            force_shutdown_func: Box::new(force_shutdown),
            inspect_root: None,
            poweroff_for_shutdown: false,
        }
    }

    pub fn new_from_json(json_data: json::Value) -> Self {
        #[derive(Deserialize, Default)]
        struct Config {
            #[serde(default)]
            poweroff_for_shutdown: bool,
        }

        #[derive(Deserialize)]
        struct JsonData {
            config: Option<Config>,
        }

        let data: JsonData = json::from_value(json_data).unwrap();
        Self::new()
            .with_poweroff_for_shutdown(data.config.unwrap_or_default().poweroff_for_shutdown)
    }

    pub fn with_poweroff_for_shutdown(mut self, poweroff_for_shutdown: bool) -> Self {
        self.poweroff_for_shutdown = poweroff_for_shutdown;
        self
    }

    #[cfg(test)]
    pub fn with_shutdown_shim_proxy(mut self, proxy: fpowercontrol::AdminProxy) -> Self {
        self.shutdown_shim_proxy = Some(proxy);
        self
    }

    #[cfg(test)]
    pub fn with_force_shutdown_function(
        mut self,
        force_shutdown: Box<impl Fn() + 'static>,
    ) -> Self {
        self.force_shutdown_func = force_shutdown;
        self
    }

    #[cfg(test)]
    pub fn with_inspect_root(mut self, root: &'a inspect::Node) -> Self {
        self.inspect_root = Some(root);
        self
    }

    pub fn build(self) -> Result<Rc<SystemShutdownHandler>, Error> {
        // Optionally use the default inspect root node
        let inspect_root =
            self.inspect_root.unwrap_or_else(|| inspect::component::inspector().root());

        // Connect to the shutdown-shim's Admin service if a proxy wasn't specified
        let shutdown_shim_proxy = if let Some(proxy) = self.shutdown_shim_proxy {
            proxy
        } else {
            fuchsia_component::client::connect_to_protocol::<fpowercontrol::AdminMarker>()?
        };

        let node = Rc::new(SystemShutdownHandler {
            force_shutdown_func: self.force_shutdown_func,
            shutdown_shim_proxy,
            inspect: InspectData::new(inspect_root, "SystemShutdownHandler".to_string()),
            poweroff_for_shutdown: self.poweroff_for_shutdown,
        });

        Ok(node)
    }
}

pub struct SystemShutdownHandler {
    /// Function to force a system shutdown.
    force_shutdown_func: Box<dyn Fn()>,

    /// Proxy handle to communicate with the Shutdown-shim's Admin protocol.
    shutdown_shim_proxy: fpowercontrol::AdminProxy,

    /// Struct for managing Component Inspection data
    inspect: InspectData,

    /// If true, will poweroff during shutdown instead of reboot
    poweroff_for_shutdown: bool,
}

impl SystemShutdownHandler {
    /// Called only when there is a high temperature reboot request.
    /// If the function is called while a shutdown is already in
    /// progress, then an error is returned. This is the only scenario where the function will
    /// return. In all other cases, the function does not return.
    async fn handle_shutdown(&self, msg: &Message) -> Result<(), Error> {
        fuchsia_trace::instant!(
            c"power_manager",
            c"SystemShutdownHandler::handle_shutdown",
            fuchsia_trace::Scope::Thread,
            "msg" => format!("{:?}", msg).as_str()
        );

        self.inspect.log_shutdown_request(&msg);

        let result = if self.poweroff_for_shutdown {
            info!("System poweroff ({:?})", msg);
            self.shutdown_shim_proxy
                .shutdown(&fpowercontrol::ShutdownOptions {
                    action: Some(fpowercontrol::ShutdownAction::Poweroff),
                    reasons: Some(vec![fpowercontrol::ShutdownReason::HighTemperature]),
                    ..Default::default()
                })
                .await
        } else {
            info!("System reboot ({:?})", msg);
            self.shutdown_shim_proxy
                .shutdown(&fpowercontrol::ShutdownOptions {
                    action: Some(fpowercontrol::ShutdownAction::Reboot),
                    reasons: Some(vec![fpowercontrol::ShutdownReason::HighTemperature]),
                    ..Default::default()
                })
                .await
        };

        // If the result is an error, either by underlying API failure or by timeout, then force a
        // shutdown using the configured force_shutdown_func
        if result.is_err() {
            self.inspect.force_shutdown_attempted.set(true);
            (self.force_shutdown_func)();
        }

        Ok(())
    }

    /// Handle a SystemShutdown message which is a request to shut down the system.
    async fn handle_system_shutdown_message(
        &self,
        msg: &Message,
    ) -> Result<MessageReturn, PowerManagerError> {
        match self.handle_shutdown(msg).await {
            Ok(()) => Ok(MessageReturn::SystemShutdown),
            Err(e) => Err(PowerManagerError::GenericError(format_err!("{}", e))),
        }
    }
}

/// Forcibly shuts down the system. The function works by exiting the power_manager process. Since
/// the power_manager is marked as a critical process to the root job, once the power_manager exits
/// the root job will also exit, and the system will reboot.
pub fn force_shutdown() {
    info!("Force shutdown requested");
    std::process::exit(1);
}

#[async_trait(?Send)]
impl Node for SystemShutdownHandler {
    fn name(&self) -> String {
        "SystemShutdownHandler".to_string()
    }

    async fn handle_message(&self, msg: &Message) -> Result<MessageReturn, PowerManagerError> {
        match msg {
            Message::HighTemperatureShutdown => self.handle_system_shutdown_message(msg).await,
            _ => Err(PowerManagerError::Unsupported),
        }
    }
}

struct InspectData {
    // Nodes
    _root_node: inspect::Node,

    // Properties
    shutdown_request: inspect::StringProperty,
    force_shutdown_attempted: inspect::BoolProperty,
}

impl InspectData {
    fn new(parent: &inspect::Node, name: String) -> Self {
        let root_node = parent.create_child(name);
        Self {
            shutdown_request: root_node.create_string("shutdown_request", "None"),
            force_shutdown_attempted: root_node.create_bool("force_shutdown_attempted", false),
            _root_node: root_node,
        }
    }

    /// Updates the `shutdown_request` property according to the provided request.
    fn log_shutdown_request(&self, request: &Message) {
        self.shutdown_request.set(format!("{:?}", request).as_str());
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use diagnostics_assertions::assert_data_tree;
    use fuchsia_async as fasync;
    use futures::TryStreamExt;
    use std::cell::Cell;

    /// Create a fake Admin service proxy that responds to Shutdown requests by calling
    /// the provided closure.
    fn setup_fake_admin_service(
        mut shutdown_function: impl FnMut(fpowercontrol::ShutdownOptions) + 'static,
    ) -> fpowercontrol::AdminProxy {
        let (proxy, mut stream) =
            fidl::endpoints::create_proxy_and_stream::<fpowercontrol::AdminMarker>();
        fasync::Task::local(async move {
            while let Ok(req) = stream.try_next().await {
                match req {
                    Some(fpowercontrol::AdminRequest::Shutdown { options, responder }) => {
                        shutdown_function(options);
                        let _ = responder.send(Ok(()));
                    }
                    e => panic!("Unexpected request: {:?}", e),
                }
            }
        })
        .detach();

        proxy
    }

    /// Tests for the presence and correctness of inspect data
    #[fuchsia::test]
    async fn test_inspect_data() {
        let inspector = inspect::Inspector::default();
        let node = SystemShutdownHandlerBuilder::new()
            .with_inspect_root(inspector.root())
            .with_force_shutdown_function(Box::new(|| {}))
            .build()
            .unwrap();

        // Issue a shutdown call that will fail which causes a force shutdown to be issued.
        // This gives us something interesting to verify in Inspect.
        let _ = node.handle_shutdown(&Message::HighTemperatureShutdown).await;

        assert_data_tree!(
            inspector,
            root: {
                SystemShutdownHandler: {
                    shutdown_request: "HighTemperatureShutdown",
                    force_shutdown_attempted: true
                }
            }
        );
    }

    /// Tests that the handle_shutdown function correctly sets the reboot reasons and calls
    /// Admin shutdown API.
    #[fuchsia::test]
    async fn test_shutdown() {
        // At the end of the test, verify the Admin server's received shutdown request with correct
        // reason.
        let shutdown_count = Rc::new(Cell::new(0));
        let shutdown_count_clone = shutdown_count.clone();

        let shutdown_action = Rc::new(Cell::new(None));
        let shutdown_action_clone = shutdown_action.clone();

        let shutdown_reasons = Rc::new(Cell::new(vec![]));
        let shutdown_reasons_clone = shutdown_reasons.clone();

        // Create the node with a special Admin proxy
        let node = SystemShutdownHandlerBuilder::new()
            .with_shutdown_shim_proxy(setup_fake_admin_service(move |options| {
                shutdown_count_clone.set(shutdown_count_clone.get() + 1);
                shutdown_action_clone.set(options.action);
                shutdown_reasons_clone.set(options.reasons.unwrap());
            }))
            .build()
            .unwrap();

        // Call handle_shutdown which results in a fidl call.
        let _ = node.handle_shutdown(&Message::HighTemperatureShutdown).await;

        // Verify the shutdown was called with correct reason.
        assert_eq!(shutdown_count.get(), 1);
        assert_eq!(shutdown_action.get(), Some(fpowercontrol::ShutdownAction::Reboot));
        assert_eq!(shutdown_reasons.take(), vec![fpowercontrol::ShutdownReason::HighTemperature]);
    }

    #[fuchsia::test]
    async fn test_high_temperature_poweroff() {
        // At the end of the test, verify the Admin server's received shutdown request
        let shutdown_count = Rc::new(Cell::new(0));
        let shutdown_count_clone = shutdown_count.clone();

        let shutdown_action = Rc::new(Cell::new(None));
        let shutdown_action_clone = shutdown_action.clone();

        let shutdown_reasons = Rc::new(Cell::new(vec![]));
        let shutdown_reasons_clone = shutdown_reasons.clone();

        // Create the node with a special Admin proxy
        let node = SystemShutdownHandlerBuilder::new()
            .with_shutdown_shim_proxy(setup_fake_admin_service(move |options| {
                shutdown_count_clone.set(shutdown_count_clone.get() + 1);
                shutdown_action_clone.set(options.action);
                shutdown_reasons_clone.set(options.reasons.unwrap());
            }))
            .with_poweroff_for_shutdown(true)
            .build()
            .unwrap();

        // Call handle_shutdown which results in a fidl call.
        let _ = node.handle_shutdown(&Message::HighTemperatureShutdown).await;

        // Verify the shutdown was called with correct reason.
        assert_eq!(shutdown_count.get(), 1);
        assert_eq!(shutdown_action.get(), Some(fpowercontrol::ShutdownAction::Poweroff));
        assert_eq!(shutdown_reasons.take(), vec![fpowercontrol::ShutdownReason::HighTemperature]);
    }

    /// Tests that if high temperature reboot request fails, the forced shutdown method is called.
    #[fuchsia::test]
    async fn test_force_shutdown() {
        let force_shutdown = Rc::new(Cell::new(false));
        let force_shutdown_clone = force_shutdown.clone();
        let force_shutdown_func = Box::new(move || {
            force_shutdown_clone.set(true);
        });

        let node = SystemShutdownHandlerBuilder::new()
            .with_force_shutdown_function(force_shutdown_func)
            .build()
            .unwrap();

        // Call the normal shutdown function. The call will fail because there is no routing to the
        // server in this unit test, and then the forced shutdown method will be called.
        let _ = node.handle_shutdown(&Message::HighTemperatureShutdown).await;
        assert_eq!(force_shutdown.get(), true);
    }
}
