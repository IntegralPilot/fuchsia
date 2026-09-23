// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use component_events::events::{EventStream, ExitStatus, Stopped};
use component_events::matcher::EventMatcher;
use fidl::endpoints::DiscoverableProtocolMarker;
use fidl_fuchsia_component::{CreateChildArgs, RealmProxy};
use fidl_fuchsia_component_decl::{Child, CollectionRef, StartupMode};
use fidl_fuchsia_starnix_psi::PsiProviderMarker;
use fuchsia_component_test::{
    Capability, ChildOptions, RealmBuilder, RealmBuilderParams, RealmInstance, Ref, Route,
};
use fuchsia_runtime::{HandleInfo, HandleType};
use futures::io::BufReader;
use futures::{AsyncBufReadExt, AsyncWriteExt};
use itertools::Itertools;
use log::info;
use std::sync::Arc;
use {fidl_fuchsia_process as fprocess, fuchsia_async as fasync};

use crate::fake_psi_provider::FakePsiProvider;

#[derive(Clone, Copy, Debug)]
pub struct PuppetFileDescriptor(usize);

pub struct PuppetInstance {
    realm: RealmInstance,
    events: EventStream,

    /// See the `ControlSocket` class in `puppet.cc` for documentation on the format of messages
    /// transferred through this socket.
    ctl_channel: BufReader<fasync::Socket>,
}

impl PuppetInstance {
    pub async fn new(fake_psi_provider: Option<Arc<FakePsiProvider>>) -> PuppetInstance {
        let events = EventStream::open().await.unwrap();

        let builder = RealmBuilder::with_params(
            RealmBuilderParams::new().from_relative_url("#meta/test_realm.cm"),
        )
        .await
        .unwrap();

        if let Some(fake_psi_provider) = fake_psi_provider {
            let psi_provider_ref = builder
                .add_local_child(
                    "fake_psi_provider",
                    move |handles| Box::pin(fake_psi_provider.clone().serve(handles)),
                    ChildOptions::new(),
                )
                .await
                .unwrap();

            builder
                .add_route(
                    Route::new()
                        .capability(Capability::protocol_by_name(PsiProviderMarker::PROTOCOL_NAME))
                        .from(&psi_provider_ref)
                        .to(Ref::child("kernel")),
                )
                .await
                .unwrap();
        }

        info!("building realm and starting eager container");
        let realm = builder.build().await.unwrap();

        // Create a control channel to be connected to the puppet.
        let (ctl_puppet_side, ctl_local_side) = zx::Socket::create_stream();

        info!("kernel and container init have requested thread roles, starting puppet");
        let test_realm: RealmProxy = realm.root.connect_to_protocol_at_exposed_dir().unwrap();
        test_realm
            .create_child(
                &CollectionRef { name: "puppets".to_string() },
                &Child {
                    name: Some("puppet".to_string()),
                    url: Some("#meta/puppet.cm".to_string()),
                    startup: Some(StartupMode::Lazy),
                    ..Default::default()
                },
                CreateChildArgs {
                    numbered_handles: Some(vec![fprocess::HandleInfo {
                        id: HandleInfo::new(HandleType::FileDescriptor, 3).as_raw(),
                        handle: ctl_puppet_side.into(),
                    }]),
                    ..Default::default()
                },
            )
            .await
            .unwrap()
            .unwrap();

        let mut puppet = PuppetInstance {
            realm,
            events,
            ctl_channel: BufReader::new(fasync::Socket::from_socket(ctl_local_side)),
        };

        // Synchronization point: wait until the puppet reports readiness.
        let initial_message = puppet.read_message().await;
        assert_eq!(initial_message, vec!["READY".to_string()]);

        puppet
    }

    pub async fn check_exists(&mut self, path: &str) -> bool {
        self.write_message(&["CHECK_EXISTS", path]).await;
        let reply = self.read_message().await;
        reply[0] == "YES"
    }

    pub async fn open(&mut self, path: &str) -> PuppetFileDescriptor {
        self.write_message(&["OPEN", path]).await;
        let reply = self.read_message().await;
        PuppetFileDescriptor(reply[0].parse().unwrap())
    }

    pub async fn close(&mut self, fd: PuppetFileDescriptor) {
        self.write_message(&["CLOSE", &fd.0.to_string()]).await;
    }

    pub async fn read_to_end(&mut self, fd: PuppetFileDescriptor) -> String {
        self.write_message(&["READ_TO_END", &fd.0.to_string()]).await;
        let reply = self.read_message().await;
        reply[0].clone()
    }

    pub async fn write_all(&mut self, fd: PuppetFileDescriptor, data: &str) {
        self.write_message(&["WRITE_ALL", &fd.0.to_string(), data]).await;
        let _ = self.read_message().await;
    }

    /// Calls poll() with events=POLLPRI on the file descriptor, returning true
    /// if it delivered a POLLPRI event or false if it timed out.
    pub async fn poll_pollpri(&mut self, fd: PuppetFileDescriptor, timeout_ms: usize) -> bool {
        self.write_message(&["POLL_POLLPRI", &fd.0.to_string(), &timeout_ms.to_string()]).await;
        let reply = self.read_message().await;
        reply[0] == "EVENT"
    }

    /// Creates an epollfd and registers the PSI file descriptor with events=EPOLLPRI.
    pub async fn epoll_create_and_add_epollpri(
        &mut self,
        fd: PuppetFileDescriptor,
    ) -> PuppetFileDescriptor {
        self.write_message(&["EPOLL_CREATE_AND_ADD_EPOLLPRI", &fd.0.to_string()]).await;
        let reply = self.read_message().await;
        PuppetFileDescriptor(reply[0].parse().unwrap())
    }

    /// Calls epoll_wait() on an epollfd that was created by epoll_create_and_add_epollpri.
    /// Returns true if the PSI file descriptor delivered a POLLPRI event or false if it timed out.
    pub async fn epoll_wait_epollpri(
        &mut self,
        epollfd: PuppetFileDescriptor,
        timeout_ms: usize,
    ) -> bool {
        self.write_message(&[
            "EPOLL_WAIT_EPOLLPRI",
            &epollfd.0.to_string(),
            &timeout_ms.to_string(),
        ])
        .await;
        let reply = self.read_message().await;
        reply[0] == "EVENT"
    }

    async fn write_message(&mut self, msg: &[&str]) {
        let data = format!("{};", msg.iter().map(|s| hex::encode(s.as_bytes())).join(","));
        self.ctl_channel.write_all(data.as_bytes()).await.unwrap();
    }

    async fn read_message(&mut self) -> Vec<String> {
        let mut buf = Vec::new();
        self.ctl_channel.read_until(b';', &mut buf).await.unwrap();
        let buf = buf.strip_suffix(b";").unwrap();
        buf.split(|c| *c == b',')
            .map(|s| String::from_utf8(hex::decode(s).unwrap()).unwrap())
            .collect()
    }

    pub async fn check_exit_clean(mut self) {
        info!("waiting for puppet to exit");
        self.write_message(&["EXIT"]).await;

        let puppet_stopped = EventMatcher::ok()
            .moniker_regex("realm_builder:.+/puppets:puppet")
            .wait::<Stopped>(&mut self.events)
            .await
            .unwrap();
        assert_eq!(
            puppet_stopped.result().unwrap().status,
            ExitStatus::Clean,
            "puppet must exit cleanly"
        );

        self.realm.destroy().await.unwrap();
    }
}
