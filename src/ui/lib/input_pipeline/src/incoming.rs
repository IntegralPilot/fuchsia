// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::Transport;
use anyhow::Context;
use fidl_fuchsia_io as fio;
use fuchsia_component::client::Connect;
use fuchsia_component::directory::{AsRefDirectory, Directory};

#[cfg(feature = "dso")]
pub use dso::*;

#[cfg(not(feature = "dso"))]
pub use elf::*;

mod dso {
    #![cfg(feature = "dso")]

    use super::*;
    use crate::DriverTransport;

    #[derive(Clone)]
    pub struct Incoming(std::sync::Arc<fdf_component::Incoming>);

    impl Incoming {
        pub fn new(incoming: std::sync::Arc<fdf_component::Incoming>) -> Self {
            Self(incoming)
        }

        pub fn open_service<S: fidl::endpoints::ServiceMarker>(
            &self,
            marker: S,
        ) -> Result<fuchsia_component::client::Service<S>, anyhow::Error> {
            fuchsia_component::client::Service::open_from_dir_prefix(&*self.0, "svc", marker)
                .context("Service::open_from_dir_prefix")
        }

        pub fn connect_protocol<T: Connect>(&self) -> Result<T, anyhow::Error> {
            self.0.connect_protocol().context("connect_protocol")
        }

        pub fn connect_protocol_next<P: fidl_next::Discoverable>(
            &self,
        ) -> Result<fidl_next::ClientEnd<P, Transport>, anyhow::Error> {
            self.0.connect_protocol_libasync_next().context("connect_protocol_next")
        }

        pub fn connect_protocol_next_at<P: fidl_next::Discoverable>(
            dir: &impl AsRefDirectory,
            path: &str,
        ) -> Result<fidl_next::ClientEnd<P, Transport>, anyhow::Error> {
            fdf_component::Incoming::connect_protocol_libasync_next_at(dir, path)
                .context("connect_protocol_next_at")
        }

        pub fn connect_protocol_driver_transport<P: fidl_next::Discoverable>(
            &self,
        ) -> Result<fidl_next::ClientEnd<P, DriverTransport>, zx::Status> {
            self.0.connect_protocol_driver_transport::<P, _>(fdf::CurrentDispatcher)
        }

        pub fn connect_protocol_driver_transport_at<P: fidl_next::Discoverable>(
            dir: &impl AsRefDirectory,
            path: &str,
        ) -> Result<fidl_next::ClientEnd<P, DriverTransport>, zx::Status> {
            fdf_component::Incoming::connect_protocol_driver_transport_at::<P, _>(
                dir,
                path,
                fdf::CurrentDispatcher,
            )
        }
    }

    impl Directory for Incoming {
        fn open(
            &self,
            path: &str,
            flags: fio::Flags,
            server_end: zx::Channel,
        ) -> Result<(), anyhow::Error> {
            self.0.open(path, flags, server_end)
        }
    }
}

mod elf {
    #![cfg(not(feature = "dso"))]

    use super::*;
    use fuchsia_component::client::connect;

    #[derive(Clone)]
    pub struct Incoming;

    impl Incoming {
        pub fn new() -> Self {
            Self {}
        }

        pub fn open_service<S: fidl::endpoints::ServiceMarker>(
            &self,
            marker: S,
        ) -> Result<fuchsia_component::client::Service<S>, anyhow::Error> {
            fuchsia_component::client::Service::open(marker).context("Service::open")
        }

        pub fn connect_protocol<T: Connect>(&self) -> Result<T, anyhow::Error> {
            connect::connect_to_protocol::<T>()
        }

        pub fn connect_protocol_next<P: fidl_next::Discoverable>(
            &self,
        ) -> Result<fidl_next::ClientEnd<P, Transport>, anyhow::Error> {
            let (client_end, server_end) = zx::Channel::create();
            fdio::service_connect(&format!("/svc/{}", P::PROTOCOL_NAME), server_end)
                .context("connect_protocol_next")?;
            Ok(fidl_next::ClientEnd::<P, zx::Channel>::from_untyped(client_end))
        }

        pub fn connect_protocol_next_at<P: fidl_next::Discoverable>(
            dir: &impl AsRefDirectory,
            path: &str,
        ) -> Result<fidl_next::ClientEnd<P, Transport>, anyhow::Error> {
            let (client_end, server_end) = zx::Channel::create();
            dir.as_ref_directory().open(path, fio::Flags::PROTOCOL_SERVICE, server_end)?;
            Ok(fidl_next::ClientEnd::<P, zx::Channel>::from_untyped(client_end))
        }
    }

    impl Directory for Incoming {
        fn open(
            &self,
            path: &str,
            flags: fio::Flags,
            server_end: zx::Channel,
        ) -> Result<(), anyhow::Error> {
            let path = path.trim_start_matches('/');
            let absolute_path = if path.starts_with("svc/")
                || path == "svc"
                || path.starts_with("pkg/")
                || path == "pkg"
                || path.starts_with("dev/")
                || path == "dev"
                || path.starts_with("tmp/")
                || path == "tmp"
                || path.starts_with("hub/")
                || path == "hub"
                || path.starts_with("data/")
                || path == "data"
                || path.starts_with("cache/")
                || path == "cache"
                || path.starts_with("config/")
                || path == "config"
                || path.starts_with("incoming/")
                || path == "incoming"
            {
                format!("/{}", path)
            } else if path.is_empty() {
                "/".to_string()
            } else {
                // If it does not start with any recognized root namespace entry,
                // assume it is a service/protocol under "/svc/".
                format!("/svc/{}", path)
            };

            let namespace =
                fdio::Namespace::installed().context("failed to get installed namespace")?;
            namespace.open(&absolute_path, flags, server_end).context("Namespace::open")
        }
    }
}

impl AsRefDirectory for Incoming {
    fn as_ref_directory(&self) -> &dyn fuchsia_component::directory::Directory {
        self
    }
}
