// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fdomain_fuchsia_developer_remotecontrol::{
    IdentifyHostResponse, RemoteControlProxy, RemoteControlRequest,
};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

pub struct FakeRcsConfig {
    pub components: Vec<String>,
    pub identify_host_response: Option<IdentifyHostResponse>,
    pub capability_handlers: HashMap<String, Box<dyn Fn(fdomain_client::Channel) + 'static>>,
    pub identify_host_handler: Option<
        Rc<
            dyn Fn(fdomain_fuchsia_developer_remotecontrol::RemoteControlIdentifyHostResponder)
                + 'static,
        >,
    >,
}

impl Default for FakeRcsConfig {
    fn default() -> Self {
        Self {
            components: Vec::new(),
            identify_host_response: None,
            capability_handlers: HashMap::new(),
            identify_host_handler: None,
        }
    }
}

/// Setup a fake RCS that can handle IdentifyHost and RealmQuery, and custom capabilities.
pub fn setup_fake_rcs(
    client: Arc<fdomain_client::Client>,
    config: FakeRcsConfig,
) -> RemoteControlProxy {
    let mut mock_realm_query_builder = iquery_test_support::MockRealmQueryBuilder::prefilled();
    for c in &config.components {
        mock_realm_query_builder =
            mock_realm_query_builder.when(c.as_str()).moniker(c.as_str()).add();
    }

    let mock_realm_query = Rc::new(mock_realm_query_builder.build());
    let identify_host_response =
        config.identify_host_response.unwrap_or_else(|| IdentifyHostResponse {
            nodename: Some(String::from("fake_fuchsia_device")),
            ..Default::default()
        });

    let capability_handlers = config.capability_handlers;
    let identify_host_handler = config.identify_host_handler;

    let proxy = target_holders::fake_proxy::<RemoteControlProxy>(client, move |req| {
        let querier = Rc::clone(&mock_realm_query);
        let identify_host_response = identify_host_response.clone();
        let identify_host_handler = identify_host_handler.clone();
        match req {
            RemoteControlRequest::ConnectCapability {
                moniker,
                capability_set,
                capability_name,
                server_channel,
                responder,
            } => {
                if let Some(handler) = capability_handlers.get(&capability_name) {
                    handler(server_channel);
                    responder.send(Ok(())).unwrap();
                } else if capability_name == "svc/fuchsia.sys2.RealmQuery.root" {
                    assert_eq!(moniker, "toolbox");
                    assert_eq!(capability_set, rcs::OpenDirType::NamespaceDir);
                    let querier = Rc::clone(&querier);
                    fuchsia_async::Task::local(
                        querier.serve_f(fdomain_client::fidl::ServerEnd::new(server_channel)),
                    )
                    .detach();
                    responder.send(Ok(())).unwrap();
                } else {
                    unimplemented!("Capability {} not supported in fake RCS", capability_name);
                }
            }
            RemoteControlRequest::IdentifyHost { responder } => {
                if let Some(handler) = identify_host_handler.as_ref() {
                    handler(responder);
                } else {
                    responder.send(Ok(&identify_host_response)).unwrap();
                }
            }
            RemoteControlRequest::EchoString { value, responder } => {
                responder.send(value.as_ref()).expect("should send");
            }
            _ => unreachable!("Not implemented in fake RCS"),
        }
    });
    proxy
}
