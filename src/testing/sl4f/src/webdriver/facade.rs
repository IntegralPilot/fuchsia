// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::webdriver::types::{EnableDevToolsResult, GetDevToolsPortsResult};
use anyhow::{Error, format_err};
use fidl::endpoints::{ServerEnd, create_request_stream};
use fidl_fuchsia_web::{
    DevToolsListenerMarker, DevToolsListenerRequest, DevToolsListenerRequestStream,
    DevToolsPerContextListenerMarker, DevToolsPerContextListenerRequest,
    DevToolsPerContextListenerRequestStream,
};
use fuchsia_async as fasync;
use fuchsia_sync::Mutex;
use futures::channel::mpsc;
use futures::prelude::*;
use log::*;
use std::collections::HashSet;
use std::ops::DerefMut;

/// Facade providing access to WebDriver debug services.  Supports enabling
/// DevTools ports on Chrome contexts, and retrieving the set of open ports.
/// Open ports can be used by Chromedriver to manipulate contexts for testing.
#[derive(Debug)]
pub struct WebdriverFacade {
    /// Internal facade, instantiated when facade is initialized with
    /// `enable_dev_tools`.
    internal: Mutex<Option<WebdriverFacadeInternal>>,
}

impl WebdriverFacade {
    /// Create a new `WebdriverFacade`
    pub fn new() -> WebdriverFacade {
        WebdriverFacade { internal: Mutex::new(None) }
    }

    /// Configure WebDriver to start any future contexts in debug mode.  This
    /// allows contexts to be controlled remotely through ChromeDriver.
    pub async fn enable_dev_tools(&self) -> Result<EnableDevToolsResult, Error> {
        if self.internal.lock().is_some() {
            return Err(format_err!("DevTools already enabled."));
        }
        let initialized_internal = WebdriverFacadeInternal::new().await?;

        let mut internal = self.internal.lock();

        if internal.is_none() {
            *internal = Some(initialized_internal);
            Ok(EnableDevToolsResult::Success)
        } else {
            Err(format_err!("DevTools already enabled."))
        }
    }

    /// Returns a list of open DevTools ports.  Returns an error if DevTools
    /// have not been enabled using `enable_dev_tools`.
    pub async fn get_dev_tools_ports(&self) -> Result<GetDevToolsPortsResult, Error> {
        let mut internal = self.internal.lock();
        match internal.deref_mut() {
            Some(facade) => Ok(GetDevToolsPortsResult::new(facade.get_ports())),
            None => Err(format_err!("DevTools are not enabled.")),
        }
    }
}

/// Internal struct providing updated list of open DevTools ports using
/// WebDriver Debug service.
#[derive(Debug)]
struct WebdriverFacadeInternal {
    /// Set of currently open DevTools ports.
    dev_tools_ports: HashSet<u16>,
    /// Receiving end for port update channel.
    port_update_receiver: mpsc::UnboundedReceiver<PortUpdateMessage>,
}

impl WebdriverFacadeInternal {
    /// Create a new `WebdriverFacadeInternal`.  Can fail if connecting to the
    /// debug service fails.
    pub async fn new() -> Result<WebdriverFacadeInternal, Error> {
        let port_update_receiver = Self::get_port_event_receiver().await?;
        Ok(WebdriverFacadeInternal { dev_tools_ports: HashSet::new(), port_update_receiver })
    }

    /// Returns a copy of the available ports.
    pub fn get_ports(&mut self) -> Vec<u16> {
        self.update_port_set();
        Vec::from_iter(self.dev_tools_ports.iter().cloned())
    }

    /// Consumes messages produced by context listeners to update set of open ports.
    fn update_port_set(&mut self) {
        while let Ok(Some(update)) = self.port_update_receiver.try_next() {
            match update {
                PortUpdateMessage::PortOpened(port) => self.dev_tools_ports.insert(port),
                PortUpdateMessage::PortClosed(port) => self.dev_tools_ports.remove(&port),
            };
        }
    }

    /// Setup a channel to receive port open/close channels and return the
    /// receiving end.  Assumes Webdriver is already running.
    async fn get_port_event_receiver() -> Result<mpsc::UnboundedReceiver<PortUpdateMessage>, Error>
    {
        let (port_update_sender, port_update_receiver) = mpsc::unbounded();

        let debug = Self::spawn_dev_tools_listener_at_path(
            "/svc/fuchsia.web.Debug",
            port_update_sender.clone(),
        );
        let debug_context_provider = Self::spawn_dev_tools_listener_at_path(
            "/svc/fuchsia.web.Debug-context_provider",
            port_update_sender,
        );

        // Wait for initializing the two providers to complete, and continue so long as one or other is functional.
        let debug_result = debug.await;
        let debug_context_provider_result = debug_context_provider.await;
        debug_result.or(debug_context_provider_result)?;

        Ok(port_update_receiver)
    }

    /// Spawn an instance of `DevToolsListener` that forwards port open/close
    /// events from the debug request channel at `protocol_path` to the supplied
    /// mpsc channel.
    async fn spawn_dev_tools_listener_at_path(
        protocol_path: &str,
        port_update_sender: mpsc::UnboundedSender<PortUpdateMessage>,
    ) -> Result<(), Error> {
        // Connect to the specified protocol path.
        let debug_proxy = fuchsia_component::client::connect_to_protocol_at_path::<
            fidl_fuchsia_web::DebugMarker,
        >(protocol_path)?;

        // Create a DevToolsListener and channel, and enable DevTools.
        let (dev_tools_client, dev_tools_stream) =
            create_request_stream::<DevToolsListenerMarker>();
        debug_proxy.enable_dev_tools(dev_tools_client).await?;

        // Spawn a task to process the DevToolsListener updates asynchronously.
        fasync::Task::spawn(async move {
            let dev_tools_listener = DevToolsListener::new(port_update_sender);
            dev_tools_listener
                .handle_requests_from_stream(dev_tools_stream)
                .await
                .unwrap_or_else(|_| print!("Error handling DevToolsListener channel!"));
        })
        .detach();

        Ok(())
    }
}

/// Message passed from a `DevToolsPerContextListener` to
/// `WebdriverFacade` to notify it of a port opening or closing
#[derive(Debug)]
enum PortUpdateMessage {
    /// Sent when a port is opened.
    PortOpened(u16),
    /// Sent when a port is closed.
    PortClosed(u16),
}

/// An implementation of `fuchsia.web.DevToolsListener` that instantiates
/// `DevToolsPerContextListener` when a context is created.
struct DevToolsListener {
    /// Sender end of port update channel.
    port_update_sender: mpsc::UnboundedSender<PortUpdateMessage>,
}

impl DevToolsListener {
    /// Create a new `DevToolsListener`
    fn new(port_update_sender: mpsc::UnboundedSender<PortUpdateMessage>) -> Self {
        DevToolsListener { port_update_sender }
    }

    /// Handle requests made to `DevToolsListener`.
    pub async fn handle_requests_from_stream(
        &self,
        mut stream: DevToolsListenerRequestStream,
    ) -> Result<(), Error> {
        while let Some(request) = stream.try_next().await? {
            let DevToolsListenerRequest::OnContextDevToolsAvailable { listener, .. } = request;
            self.on_context_created(listener)?;
        }
        Ok(())
    }

    /// Handles OnContextDevToolsAvailable.  Spawns an instance of
    /// `DevToolsPerContextListener` to handle the new Chrome context.
    fn on_context_created(
        &self,
        listener: ServerEnd<DevToolsPerContextListenerMarker>,
    ) -> Result<(), Error> {
        info!("Chrome context created");
        let listener_request_stream = listener.into_stream();
        let port_update_sender = mpsc::UnboundedSender::clone(&self.port_update_sender);
        fasync::Task::spawn(async move {
            let mut per_context_listener = DevToolsPerContextListener::new(port_update_sender);
            per_context_listener
                .handle_requests_from_stream(listener_request_stream)
                .await
                .unwrap_or_else(|_| warn!("Error handling DevToolsListener channel!"));
        })
        .detach();
        Ok(())
    }
}

/// An implementation of `fuchsia.web.DevToolsPerContextListener` that forwards
/// port open/close events to an mpsc channel.
struct DevToolsPerContextListener {
    /// Sender end of port update channel.
    port_update_sender: mpsc::UnboundedSender<PortUpdateMessage>,
}

impl DevToolsPerContextListener {
    /// Create a new `DevToolsPerContextListener`
    fn new(port_update_sender: mpsc::UnboundedSender<PortUpdateMessage>) -> Self {
        DevToolsPerContextListener { port_update_sender }
    }

    /// Handle requests made to `DevToolsPerContextListener`.  The HTTP port
    /// becomes available when OnHttpPortOpen is called, and becomes
    /// unavailable when the stream closes.
    pub async fn handle_requests_from_stream(
        &mut self,
        mut stream: DevToolsPerContextListenerRequestStream,
    ) -> Result<(), Error> {
        let mut context_port = None;

        while let Ok(Some(request)) = stream.try_next().await {
            let DevToolsPerContextListenerRequest::OnHttpPortOpen { port, .. } = request;
            context_port.replace(port);
            self.on_port_open(port)?;
        }

        // Port is closed after stream ends.
        if let Some(port) = context_port {
            self.on_port_closed(port)?;
        }
        Ok(())
    }

    /// Send a port open event.
    fn on_port_open(&mut self, port: u16) -> Result<(), Error> {
        info!("DevTools port {:?} opened", port);
        self.port_update_sender
            .unbounded_send(PortUpdateMessage::PortOpened(port))
            .map_err(|_| format_err!("Error sending port open message"))
    }

    /// Send a port close event.
    fn on_port_closed(&mut self, port: u16) -> Result<(), Error> {
        info!("DevTools port {:?} closed", port);
        self.port_update_sender
            .unbounded_send(PortUpdateMessage::PortClosed(port))
            .map_err(|_| format_err!("Error sending port closed message"))
    }
}
