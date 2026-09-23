// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![allow(clippy::let_unit_value)]

use ::fidl::endpoints::RequestStream as _;
use anyhow::{Context as _, Error, anyhow};
use fidl_fuchsia_component_sandbox as fsandbox;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_paver as fpaver;
use fidl_fuchsia_process_lifecycle as flifecycle;
use fidl_fuchsia_update as fupdate;
use fidl_fuchsia_update_verify::HealthVerificationMarker;
use fuchsia_async as fasync;
use fuchsia_component::client::connect_to_protocol;
use fuchsia_component::escrow::EscrowOperation;
use fuchsia_component::server::ServiceFs;
use fuchsia_inspect::health::Reporter as _;
use fuchsia_inspect::{self as finspect};
use futures::channel::oneshot;
use futures::future::{FutureExt as _, TryFutureExt as _};
use futures::stream::StreamExt as _;
use log::{error, info, warn};
use std::sync::Arc;

mod fidl;
mod metadata;
mod reboot;

// The feedback component persists the reboot reason. It obtains the reboot reason by registering a
// watcher with the power manager, so we delay reboot slightly to give the feedback component a
// chance to obtain and persist the reboot reason.
const MINIMUM_REBOOT_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

#[fuchsia::main(logging_tags = ["system-update-committer"])]
pub fn main() -> Result<(), Error> {
    info!("starting system-update-committer");

    let mut executor = fasync::LocalExecutorBuilder::new().build();
    let () = executor.run_singlethreaded(main_async()).map_err(|err| {
        // Use anyhow to print the error chain.
        let err = anyhow!(err);
        error!("error running system-update-committer: {:#}", err);
        err
    })?;

    info!("stopping system-update-committer");
    Ok(())
}

async fn main_async() -> Result<(), Error> {
    match fuchsia_runtime::take_startup_handle(fuchsia_runtime::HandleInfo::new(
        fuchsia_runtime::HandleType::EscrowedDictionary,
        0,
    )) {
        Some(dictionary) => {
            resume_from_escrow(fsandbox::DictionaryRef { token: dictionary.into() })
                .await
                .context("resume_from_idle_stop")
        }
        None => fresh_run().await.context("first_run"),
    }
}

struct EscrowState {
    // Keep the internal pair alive so clients can't observe EVENTPAIR_PEER_CLOSED.
    p_internal: ::fidl::NullableHandle,
    p_external: ::fidl::NullableHandle,
    // Inspect fails open so in rare cases it may not be available.
    frozen_inspect: Option<::fidl::NullableHandle>,
}

// It should not be possible for this function to be called more than once per boot in prod because:
//   1. it is only called when the component is not supplied with escrowed state
//   2. CM guarantees that escrowed state will always be supplied when restarting a component
//   3. the component always* escrows state when exiting cleanly
//   4. the component is `on_terminate: "reboot"` and so unclean exits should trigger reboot
//
//   * If the component receives a fuchsia.process.lifecycle/LifeCycle.Stop message it will exit
//     cleanly without escrowing state (see https://fxbug.dev/332341289), but because the component
//     is not a dynamic component this should only be possible by manually destroying it (e.g. via
//     `ffx component stop ...`), which never occurs in prod [0].
//
// Regardless, it should be safe to call this function multiple times per boot because if the
// component has exited cleanly then commit must not be necessary (either it was already unnecessary
// when it was called the first time or the first call successfully committed) and calling this
// function when commit is not necessary is safe.
//
// Returns success if the component is idle or asked to stop.
//
// [0] https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0110_reboot_for_critical_components?hl=en#detecting_termination_of_reboot-on-terminate_components
async fn fresh_run() -> Result<(), Error> {
    let reboot_deadline = std::time::Instant::now() + MINIMUM_REBOOT_WAIT;

    let inspector = finspect::Inspector::default();
    let inspect_controller =
        inspect_runtime::publish(&inspector, inspect_runtime::PublishOptions::default());

    let config = system_update_committer_config::Config::take_from_startup_handle();
    let idle_timeout = if config.stop_on_idle_timeout_millis >= 0 {
        zx::MonotonicDuration::from_millis(config.stop_on_idle_timeout_millis)
    } else {
        zx::MonotonicDuration::INFINITE
    };
    let commit_timeout = if config.commit_timeout_seconds >= 0 {
        zx::MonotonicDuration::from_seconds(config.commit_timeout_seconds)
    } else {
        zx::MonotonicDuration::INFINITE
    };
    inspector
        .root()
        .record_child("structured_config", |config_node| config.record_inspect(config_node));

    let verification_node = inspector.root().create_child("verification");
    let commit_node = metadata::CommitInspect::new(inspector.root().create_child("commit"));
    let mut health_node = finspect::health::Node::new(inspector.root());
    let verification_node_ref = &verification_node;
    let commit_node_ref = &commit_node;
    let health_node_ref = &mut health_node;

    let paver =
        connect_to_protocol::<fpaver::PaverMarker>().context("while connecting to paver")?;
    let (boot_manager, boot_manager_server_end) = ::fidl::endpoints::create_proxy();
    paver
        .find_boot_manager(boot_manager_server_end)
        .context("transport error while calling find_boot_manager()")?;
    let reboot_proxy =
        connect_to_protocol::<fidl_fuchsia_hardware_power_statecontrol::AdminMarker>()
            .context("while connecting to power state control")?;

    let health_verification = connect_to_protocol::<HealthVerificationMarker>()
        .context("while connecting to health verification")?;

    let (p_internal, p_external) = zx::EventPair::create();
    let p_internal_clone =
        p_internal.duplicate_handle(zx::Rights::BASIC).context("while duplicating p_internal")?;

    let (unblocker, blocker) = oneshot::channel();

    // Handle putting boot metadata in happy state, rebooting on failure (if necessary), and
    // reporting health to the inspect health node.
    let commit_fut = async move {
        match metadata::put_metadata_in_happy_state(
            &boot_manager,
            &p_internal,
            unblocker,
            &health_verification,
            commit_timeout,
            verification_node_ref,
            commit_node_ref,
        )
        .await
        {
            Err(e) => {
                let msg = format!(
                    "Failed to commit system. Rebooting at {:?} given error {:#} and {:?}",
                    reboot_deadline,
                    anyhow!(e),
                    config
                );
                health_node_ref.set_unhealthy(&msg);
                warn!("{msg}");
                reboot::wait_and_reboot(fasync::Timer::new(reboot_deadline), &reboot_proxy).await;
            }
            Ok(commit_result) => {
                info!("{}", commit_result.log_msg());
                health_node_ref.set_ok();
            }
        }
    }
    .fuse();
    let mut commit_fut = std::pin::pin!(commit_fut);

    let p_external_clone =
        p_external.duplicate_handle(zx::Rights::BASIC).context("while duplicating p_external")?;
    let fidl_server = Arc::new(fidl::FuchsiaUpdateFidlServer::new(
        p_external_clone,
        blocker.map_err(|e| e.to_string()),
        idle_timeout,
    ));
    let mut fs = ServiceFs::new_local();
    fs.take_and_serve_directory_handle()
        .context("while taking directory handle")?
        .dir("svc")
        .add_fidl_service(Services::CommitStatusProvider);
    let fs = fs.until_stalled(idle_timeout);
    let active_guard = fs.try_active_guard().unwrap();

    let mut service_fut = async move {
        let out_dir = fuchsia_sync::Mutex::new(None);
        let () = fs
            .for_each_concurrent(None, |item| async {
                use fuchsia_component::server::Item;
                match item {
                    Item::Request(Services::CommitStatusProvider(stream), _active_guard) => {
                        let () = fidl_server
                            .clone()
                            .handle_commit_status_provider_stream(stream)
                            .await
                            .unwrap_or_else(|e| {
                                warn!("handling CommitStatusProviderStream {e:#}");
                            });
                    }
                    Item::Stalled(outgoing_dir) => {
                        *out_dir.lock() = Some(outgoing_dir);
                    }
                }
            })
            .await;
        let out_dir = out_dir
            .lock()
            .take()
            .expect("StallableServiceFs should return the out dir before ending");
        Ok({
            let frozen_inspect = if let Some(inspect_controller) = inspect_controller {
                Some(
                    inspect_controller
                        .escrow_frozen(inspect_runtime::EscrowOptions::default())
                        .await
                        .context("freezing inspect")?
                        .token
                        .into_handle(),
                )
            } else {
                None
            };
            (
                EscrowState {
                    p_internal: p_internal_clone.into(),
                    p_external: p_external.into(),
                    frozen_inspect,
                },
                out_dir.into(),
            )
        })
    }
    .boxed_local()
    .fuse();
    let service_fut = futures::select! {
        () = commit_fut => {
            // Keep serving the out dir until the device is committed to avoid the confusing
            // situation in which the component is running but new clients are being ignored.
            drop(active_guard);
            service_fut
        },
        _ = service_fut => {
            panic!("fidl service fut completed before commit fut. this should be impossible \
            because of the active guard");
        }
    };

    run_until_idle_or_component_stopped(service_fut).await
}

// Returns success if the component is idle or asked to stop.
async fn resume_from_escrow(escrowed_state: fsandbox::DictionaryRef) -> Result<(), Error> {
    let EscrowState { p_internal, p_external, frozen_inspect } =
        EscrowState::load(escrowed_state).await.context("loading escrowed state")?;

    let config = system_update_committer_config::Config::take_from_startup_handle();
    let idle_timeout = if config.stop_on_idle_timeout_millis >= 0 {
        zx::MonotonicDuration::from_millis(config.stop_on_idle_timeout_millis)
    } else {
        zx::MonotonicDuration::INFINITE
    };

    let p_external_clone =
        p_external.duplicate_handle(zx::Rights::BASIC).context("while duplicating p_external")?;
    let fidl_server = Arc::new(fidl::FuchsiaUpdateFidlServer::new(
        p_external_clone.into(),
        futures::future::ready(Ok(())),
        idle_timeout,
    ));
    let mut fs = ServiceFs::new_local();
    fs.take_and_serve_directory_handle()
        .context("while taking directory handle")?
        .dir("svc")
        .add_fidl_service(Services::CommitStatusProvider);
    let fs = fs.until_stalled(idle_timeout);

    let service_fut = async move {
        let out_dir = fuchsia_sync::Mutex::new(None);
        let () = fs
            .for_each_concurrent(None, |item| async {
                use fuchsia_component::server::Item;
                match item {
                    Item::Request(Services::CommitStatusProvider(stream), _active_guard) => {
                        let () = fidl_server
                            .clone()
                            .handle_commit_status_provider_stream(stream)
                            .await
                            .unwrap_or_else(|e| {
                                warn!("handling CommitStatusProviderStream {e:#}");
                            });
                    }
                    Item::Stalled(outgoing_dir) => {
                        *out_dir.lock() = Some(outgoing_dir);
                    }
                }
            })
            .await;
        let out_dir = out_dir
            .lock()
            .take()
            .expect("StallableServiceFs should return the out dir before ending");
        Ok((EscrowState { p_internal, p_external, frozen_inspect }, out_dir.into()))
    }
    .boxed_local()
    .fuse();
    run_until_idle_or_component_stopped(service_fut).await
}

// Runs service_fut until it completes or a Lifecycle.Stop message is received, then escrows state
// if possible.
#[allow(clippy::type_complexity)]
async fn run_until_idle_or_component_stopped(
    mut service_fut: futures::future::Fuse<
        futures::future::LocalBoxFuture<
            '_,
            Result<(EscrowState, ::fidl::endpoints::ServerEnd<fio::DirectoryMarker>), Error>,
        >,
    >,
) -> Result<(), Error> {
    // We have ignored fuchsia.process/Lifecycle.Stop messages until now so that `fresh_run` can try
    // to commit the system without interruption.
    // Components have 5 seconds [0] to respond to Stop messages before being terminated by CM. We
    // use the entire period to maximize the chance we commit the system if it is pending and passes
    // the checks to avoid unnecessarily spending one of the seven boot attempts before rollback.
    // [0] https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/component_manager/src/model/environment.rs;l=31;drc=2f83da829133fd5432e7d9a3aeb4f46750f8572e
    let lifecycle = fuchsia_runtime::take_startup_handle(fuchsia_runtime::HandleInfo::new(
        fuchsia_runtime::HandleType::Lifecycle,
        0,
    ))
    .context("taking lifecycle handle")?;
    let lifecycle: ::fidl::endpoints::ServerEnd<flifecycle::LifecycleMarker> = lifecycle.into();
    let (mut lifecycle_request_stream, lifecycle_controller) =
        lifecycle.into_stream_and_control_handle();
    let escrow_operation = EscrowOperation::new_with_control_handle(lifecycle_controller);

    futures::select! {
        res = service_fut => {
            let (state, out_dir) = res?;
            let escrowed_dictionary = state.store().await.context("escrowing state")?;
            escrow_operation.with_fsandbox_dictionary(escrowed_dictionary);
            escrow_operation.run(out_dir).context("failed to run escrow operation")?;
            Ok(())
        },
        req = lifecycle_request_stream.next() => {
            match req.ok_or_else(|| anyhow::anyhow!("LifecycleRequest stream closed unexpectedly"))?
            .context("error reading from LifecycleRequest stream")?
                {
                flifecycle::LifecycleRequest::Stop{ control_handle} => {
                    // TODO(https://fxbug.dev/332341289) Exit cleanly by escrowing state including
                    // any client connections.
                    info!(
                        "received flifecycle::LifecycleRequest::Stop. Any client connections will \
                         be closed. This should only happen during shutdown."
                    );
                    // The shutdown request is acknowledged by closing the lifecycle channel which
                    // causes CM to kill the component.
                    // Intentionally leak the channel (which is held by `escrow_operation`) so that
                    // it will be closed when the OS cleans up the process, allowing the rest of
                    // the component's own cleanup to occur.

                    // Drop these so the Arc<Channel> in the request stream can be unwrapped.
                    drop((control_handle, escrow_operation));
                    // Leak the internal channel instead of the RequestStream because the Fuchsia
                    // executor will panic if it is dropped while registered receivers are still
                    // alive.
                    let (inner, _terminated): (_, bool) = lifecycle_request_stream.into_inner();
                    let inner = std::sync::Arc::try_unwrap(inner).map_err(
                        |_: std::sync::Arc<_>| {
                            anyhow::anyhow!("failed to extract lifecycle channel from Arc")
                        },
                    )?;
                    let inner: zx::Channel = inner.into_channel().into_zx_channel();
                    std::mem::forget(inner);
                    Ok(())
                }
            }
        }
    }
}

impl EscrowState {
    const INTERNAL_EVENTPAIR: &'static str = "p_internal";
    const EXTERNAL_EVENTPAIR: &'static str = "p_external";
    const INSPECT: &'static str = "frozen_inspect";

    async fn load(dict: fsandbox::DictionaryRef) -> Result<Self, Error> {
        let store =
            fuchsia_component::client::connect_to_protocol::<fsandbox::CapabilityStoreMarker>()?;
        let id_generator = sandbox::CapabilityIdGenerator::new();

        let dict_id = id_generator.next();
        let () = store
            .import(dict_id, fsandbox::Capability::Dictionary(dict))
            .await?
            .map_err(|e| anyhow!("{e:?}"))?;

        let remove_from_dict = |key: &'static str| async {
            let id = id_generator.next();
            match store
                .dictionary_remove(dict_id, key, Some(&fsandbox::WrappedNewCapabilityId { id }))
                .await?
            {
                Ok(()) => {
                    let fsandbox::Capability::Handle(handle) =
                        store.export(id).await?.map_err(|e| anyhow!("{e:?}"))?
                    else {
                        anyhow::bail!("Bad capability type from dictionary");
                    };
                    Ok(Some(handle))
                }
                Err(fsandbox::CapabilityStoreError::ItemNotFound) => Ok(None),
                Err(e) => {
                    anyhow::bail!("exporting frozen inspect {e:?}");
                }
            }
        };

        let p_internal = remove_from_dict(Self::INTERNAL_EVENTPAIR)
            .await?
            .ok_or_else(|| anyhow!("internal eventpair missing from escrow"))?;
        let p_external = remove_from_dict(Self::EXTERNAL_EVENTPAIR)
            .await?
            .ok_or_else(|| anyhow!("external eventpair missing from escrow"))?;
        let frozen_inspect = remove_from_dict(Self::INSPECT).await?;

        // TODO(https://fxbug.dev/383161492) Using the unescrowed inspect token, unescrow and
        // republish the inspect data itself (using fuchsia.inspect/InspectSink.FetchEscrow) so that
        // the component appears to be running if an inspect snapshot is taken (i.e. so that
        // escrowed=false) in the inspect snapshot).
        Ok(Self { p_internal, p_external, frozen_inspect })
    }

    async fn store(self) -> Result<fsandbox::DictionaryRef, Error> {
        let Self { p_internal, p_external, frozen_inspect } = self;
        let store =
            fuchsia_component::client::connect_to_protocol::<fsandbox::CapabilityStoreMarker>()?;
        let id_generator = sandbox::CapabilityIdGenerator::new();
        let dict_id = id_generator.next();
        let () = store.dictionary_create(dict_id).await?.map_err(|e| anyhow!("{e:?}"))?;

        let insert_in_dict = |handle, key| async {
            let id = id_generator.next();
            let () = store
                .import(id, fsandbox::Capability::Handle(handle))
                .await?
                .map_err(|e| anyhow!("{e:?}"))?;
            let () = store
                .dictionary_insert(dict_id, &fsandbox::DictionaryItem { key, value: id })
                .await?
                .map_err(|e| anyhow!("{e:?}"))?;
            Result::<_, anyhow::Error>::Ok(())
        };

        if let Some(frozen_inspect) = frozen_inspect {
            let () = insert_in_dict(frozen_inspect, Self::INSPECT.into()).await?;
        }
        let () = insert_in_dict(p_internal, Self::INTERNAL_EVENTPAIR.into()).await?;
        let () = insert_in_dict(p_external, Self::EXTERNAL_EVENTPAIR.into()).await?;

        let fsandbox::Capability::Dictionary(dictionary_ref) =
            store.export(dict_id).await?.map_err(|e| anyhow!("{e:?}"))?
        else {
            anyhow::bail!("Bad capability type from dictionary");
        };
        Ok(dictionary_ref)
    }
}

enum Services {
    CommitStatusProvider(fupdate::CommitStatusProviderRequestStream),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    async fn escrow_state_round_trip() {
        let (p_internal, p_external) = zx::EventPair::create();
        let frozen_inspect = Some(zx::Event::create().into());

        let state = EscrowState {
            p_internal: p_internal.into(),
            p_external: p_external.into(),
            frozen_inspect,
        };

        let stored = state.store().await.unwrap();
        let loaded = EscrowState::load(stored).await.unwrap();
        assert!(loaded.frozen_inspect.is_some());
    }

    #[fuchsia::test]
    async fn escrow_state_round_trip_missing_inspect() {
        let (p_internal, p_external) = zx::EventPair::create();

        let state = EscrowState {
            p_internal: p_internal.into(),
            p_external: p_external.into(),
            frozen_inspect: None,
        };

        let stored = state.store().await.unwrap();
        let loaded = EscrowState::load(stored).await.unwrap();
        assert_eq!(loaded.frozen_inspect, None);
    }
}
