// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use async_trait::async_trait;
use ffx_core::{FfxInjectorError, Injector, downcast_injector_error};
use ffx_target::TargetProxy;
use fidl_fuchsia_developer_ffx::{DaemonProxy, VersionInfo};
use fidl_fuchsia_developer_remotecontrol::RemoteControlProxy;
use std::future::Future;
use std::pin::Pin;

pub struct FakeInjector {
    pub daemon_factory_force_autostart_closure:
        Box<dyn Fn() -> Pin<Box<dyn Future<Output = Result<DaemonProxy>>>>>,
    pub daemon_factory_closure: Box<dyn Fn() -> Pin<Box<dyn Future<Output = Result<DaemonProxy>>>>>,
    pub try_daemon_closure:
        Box<dyn Fn() -> Pin<Box<dyn Future<Output = Result<Option<DaemonProxy>>>>>>,
    pub remote_factory_closure:
        Box<dyn Fn() -> Pin<Box<dyn Future<Output = Result<RemoteControlProxy>>>>>,
    pub remote_factory_closure_f: Box<
        dyn Fn() -> Pin<
            Box<
                dyn Future<
                    Output = Result<fdomain_fuchsia_developer_remotecontrol::RemoteControlProxy>,
                >,
            >,
        >,
    >,
    pub target_factory_closure: Box<dyn Fn() -> Pin<Box<dyn Future<Output = Result<TargetProxy>>>>>,
    pub is_experiment_closure: Box<dyn Fn(&str) -> Pin<Box<dyn Future<Output = bool>>>>,
    pub build_info_closure:
        Box<dyn Fn() -> Pin<Box<dyn Future<Output = anyhow::Result<VersionInfo>>>>>,
}

impl Default for FakeInjector {
    fn default() -> Self {
        Self {
            daemon_factory_closure: Box::new(|| Box::pin(async { unimplemented!() })),
            daemon_factory_force_autostart_closure: Box::new(|| {
                Box::pin(async { unimplemented!() })
            }),
            try_daemon_closure: Box::new(|| Box::pin(async { unimplemented!() })),
            remote_factory_closure: Box::new(|| Box::pin(async { unimplemented!() })),
            remote_factory_closure_f: Box::new(|| Box::pin(async { unimplemented!() })),
            target_factory_closure: Box::new(|| Box::pin(async { unimplemented!() })),
            is_experiment_closure: Box::new(|_| Box::pin(async { unimplemented!() })),
            build_info_closure: Box::new(|| Box::pin(async { unimplemented!() })),
        }
    }
}

#[async_trait(?Send)]
impl Injector for FakeInjector {
    async fn daemon_factory_force_autostart(&self) -> Result<DaemonProxy, FfxInjectorError> {
        downcast_injector_error((self.daemon_factory_force_autostart_closure)().await)
    }

    async fn daemon_factory(&self) -> Result<DaemonProxy, FfxInjectorError> {
        downcast_injector_error((self.daemon_factory_closure)().await)
    }

    async fn remote_factory(&self) -> Result<RemoteControlProxy> {
        (self.remote_factory_closure)().await
    }

    async fn remote_factory_fdomain(
        &self,
    ) -> Result<fdomain_fuchsia_developer_remotecontrol::RemoteControlProxy> {
        (self.remote_factory_closure_f)().await
    }

    async fn target_factory(&self) -> Result<TargetProxy> {
        (self.target_factory_closure)().await
    }
}
