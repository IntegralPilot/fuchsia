// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::time::Duration;

use fdomain_client::fidl::Proxy;
use ffx_command_error::Result;
use fho::{FhoEnvironment, TryFromEnv as _};

mod from_toolbox;
mod host_ssh_addr;
mod nodename;
mod remote_control_proxy;
mod ssh_addr;
mod target_info_query;
mod with_moniker;

pub use from_toolbox::toolbox;
pub use host_ssh_addr::HostAddrHolder;
pub use nodename::NodenameHolder;
pub use remote_control_proxy::{
    RemoteControlProxyHolder, fake_async_proxy, fake_proxy, open_moniker,
};
pub use ssh_addr::SshAddrHolder;
pub use target_info_query::TargetInfoQueryHolder;
pub use with_moniker::{
    ExposedDirectoryConnector, OptionalProtocolConnector, WithMoniker, exposed_dir,
    optional_moniker,
};

const DEFAULT_PROXY_TIMEOUT: Duration = Duration::from_secs(15);

/// A decorator for proxy types in [`crate::FfxTool`] implementations so you can
/// specify the moniker for the component exposing the proxy you're loading.
///
/// This is actually an alias to [`toolbox_or`], so it will also try
/// your tool's default toolbox first.
///
/// Example:
///
/// ```rust
/// #[derive(FfxTool)]
/// struct Tool {
///     #[with(fho::moniker("core/foo/thing"))]
///     foo_proxy: FooProxy,
/// }
/// ```
pub fn moniker<P: Proxy>(moniker: impl AsRef<str>) -> WithMoniker<P> {
    WithMoniker::new(moniker.as_ref())
}

pub(crate) async fn connect_to_rcs(env: &FhoEnvironment) -> Result<RemoteControlProxyHolder> {
    let retry_count = 1;
    let mut tries = 0;
    // TODO(b/287693891): Remove explicit retries/timeouts here so they can be
    // configurable instead.
    loop {
        tries += 1;
        let res = RemoteControlProxyHolder::try_from_env(env).await;
        if res.is_ok() || tries > retry_count {
            // Using `TryFromEnv` on `RemoteControlProxy` already contains user error information,
            // which will be propagated after exiting the loop.
            break Ok(res?);
        }
    }
}
