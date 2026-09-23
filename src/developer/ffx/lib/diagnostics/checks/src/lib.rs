// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use discovery::TargetHandle;
use ffx_config::EnvironmentContext;
use ffx_diagnostics::{Check, CheckExt, Notifier};
use safe_string::TermSafe;
use std::time::Duration;
use termio::Colors;

mod check_fastboot;
mod check_target_specifier;
mod connect_rcs;
mod connect_ssh;
mod discovery_stream;
mod resolve_target;
mod verify_ssh_keys;

pub use discovery_stream::{DiagnosticsResolver, NotifierMessage, SingleTargetResolver};

use check_fastboot::check_fastboot_device;
use check_target_specifier::GetTargetSpecifier;
use connect_rcs::ConnectRemoteControlProxy;
use connect_ssh::{ConnectSsh, DefaultSshConnectorProvider};
use resolve_target::ResolveTarget;
use verify_ssh_keys::{DefaultKeyVerifier, VerifySshKeys};

pub async fn ffx_diagnostics_analytics<N>(notifier: &mut N) -> fho::Result<()>
where
    N: Notifier + std::marker::Unpin,
{
    if ffx_diagnostics_analytics::is_analytics_enabled().await {
        notifier.info("Analytics enabled.")?;
    } else {
        notifier.info("Analytics NOT enabled. Skipping.")?;
    }
    Ok(())
}

pub async fn run_diagnostics_with_handle<N>(
    env_context: &EnvironmentContext,
    target_handle: TargetHandle,
    notifier: &mut N,
    product_timeout: Duration,
) -> fho::Result<()>
where
    N: Notifier + std::marker::Unpin,
{
    ffx_diagnostics_analytics(notifier).await?;
    run_diagnostics_with_handle_inner(env_context, target_handle, notifier, product_timeout).await
}

pub async fn run_diagnostics<N>(
    env: &EnvironmentContext,
    notifier: &mut N,
    product_timeout: Duration,
) -> fho::Result<()>
where
    N: Notifier + std::marker::Unpin,
{
    ffx_diagnostics_analytics(notifier).await?;
    let (target, notifier) = GetTargetSpecifier::new(&env)
        .check_with_notifier((), notifier)
        .and_then_check(ResolveTarget::<N>::new(&env))
        .await?;
    run_diagnostics_with_handle_inner(&env, target, notifier, product_timeout).await?;
    Ok(())
}

/// Helper function so that both `run_diagnostics_with_handle` and `run_diagnostics`, as top level
/// functions, can invoke `ffx_diagnostics_analytics` exactly once.
async fn run_diagnostics_with_handle_inner<N>(
    env_context: &EnvironmentContext,
    target_handle: TargetHandle,
    notifier: &mut N,
    product_timeout: Duration,
) -> fho::Result<()>
where
    N: Notifier + std::marker::Unpin,
{
    match target_handle.state {
        discovery::TargetState::Product { .. } => {
            check_product_device(env_context, notifier, target_handle, product_timeout).await?
        }
        discovery::TargetState::Fastboot(_) => {
            check_fastboot_device(env_context, notifier, target_handle).await?
        }
        discovery::TargetState::Unknown => {
            fho::return_user_error!("Device is in an unknown state. No way to check status.")
        }
        discovery::TargetState::Zedboot => {
            fho::return_user_error!("Zedboot is not currently supported for this command.")
        }
    }
    notifier.on_success("All checks passed.")?;
    Ok(())
}

async fn check_product_device<N>(
    env_context: &EnvironmentContext,
    notifier: &mut N,
    device: TargetHandle,
    timeout: Duration,
) -> fho::Result<()>
where
    N: Notifier + std::marker::Unpin,
{
    // Depending on the number of targets resolved and their types,
    // this could go one of several ways. It may also be nice to mention where the devices
    // originated. This does not check VSock devices.
    let conn_provider = DefaultSshConnectorProvider;
    let key_verifier = DefaultKeyVerifier;
    let (info, notifier) = VerifySshKeys::new(env_context, &key_verifier)
        .check_with_notifier(device, notifier)
        .and_then_check(ConnectSsh::new(env_context, &conn_provider))
        .and_then_check(ConnectRemoteControlProxy::new(timeout))
        .await
        .map_err(|e: anyhow::Error| fho::Error::User(e.into()))?;
    let info_bits = [
        info.name.as_ref().map(|n| format!("name: {}", TermSafe::from_str_escaped(n))),
        info.model.as_ref().map(|m| format!("model: {}", TermSafe::from_str_escaped(m))),
        info.manufacturer
            .as_ref()
            .map(|m| format!("manufacturer: {}", TermSafe::from_str_escaped(m))),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    let colors = Colors::current();
    notifier.on_success(format!(
        "Got device info: {}{}{}",
        colors.green,
        info_bits.join(" "),
        colors.reset
    ))?;
    Ok(())
}
