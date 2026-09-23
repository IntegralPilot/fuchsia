// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod controller;
mod conversion;
pub mod socket_filters;

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::num::NonZeroUsize;
use std::pin::pin;
use std::sync::Arc;

use fidl::endpoints::RequestStream as _;
use futures::channel::mpsc;
use futures::future::FusedFuture as _;
use futures::lock::Mutex;
use futures::{FutureExt as _, StreamExt as _, TryStreamExt as _, select, stream};
use itertools::Itertools;
use log::{debug, error, info, warn};
use thiserror::Error;

use fidl_fuchsia_ebpf as febpf;
use fidl_fuchsia_net_filter as fnet_filter;
use fidl_fuchsia_net_filter_ext as fnet_filter_ext;
use fidl_fuchsia_net_root as fnet_root;
use fuchsia_async as fasync;

use crate::bindings::util::{ErrorLogExt, ScopeExt as _};
use controller::{CommitResult, Controller};

// The maximum number of events a client for the `fuchsia.net.filter/Watcher` is
// allowed to have queued. Clients will be dropped if they exceed this limit.
// Keep this a multiple of `fnet_filter::MAX_BATCH_SIZE` (5 is somewhat
// arbitrary) so that we don't artificially truncate the allowed batch size.
const MAX_PENDING_EVENTS: usize = (fnet_filter::MAX_BATCH_SIZE * 5) as usize;

#[derive(Default, Clone)]
pub(crate) struct UpdateDispatcher(Arc<Mutex<UpdateDispatcherInner>>);

#[derive(Default)]
struct UpdateDispatcherInner {
    resources: HashMap<fnet_filter_ext::ControllerId, Controller>,
    clients: Vec<WatcherSink>,
}

enum CommitError {
    RuleWithInvalidMatcher(fnet_filter_ext::RuleId),
    RuleWithInvalidAction(fnet_filter_ext::RuleId),
    TransparentProxyWithInvalidMatcher(fnet_filter_ext::RuleId),
    RedirectWithInvalidMatcher(fnet_filter_ext::RuleId),
    MasqueradeWithInvalidMatcher(fnet_filter_ext::RuleId),
    RejectWithInvalidMatcher(fnet_filter_ext::RuleId),
    CyclicalRoutineGraph(fnet_filter_ext::RoutineId),
    ErrorOnChange { index: usize, error: fnet_filter::CommitError },
}

impl UpdateDispatcherInner {
    fn new_controller(
        &mut self,
        id: fnet_filter_ext::ControllerId,
    ) -> fnet_filter_ext::ControllerId {
        let Self { resources, clients: _ } = self;
        let mut counter = 0usize;
        let fnet_filter_ext::ControllerId(id) = id;
        loop {
            // Suffix the `id` with `counter` after the first iteration.
            let id = fnet_filter_ext::ControllerId(
                NonZeroUsize::new(counter)
                    .map(|counter| format!("{id}-{counter}"))
                    .unwrap_or_else(|| id.clone()),
            );
            match resources.entry(id.clone()) {
                Entry::Vacant(entry) => {
                    let _ = entry.insert(Controller::default());
                    return id;
                }
                Entry::Occupied(_) => {
                    counter =
                        counter.checked_add(1).expect("should find a unique ID before overflowing");
                }
            }
        }
    }

    fn connect_or_create_new_controller(&mut self, id: fnet_filter_ext::ControllerId) {
        let _: &mut _ = self.resources.entry(id).or_default();
    }

    fn remove_controller(
        &mut self,
        id: &fnet_filter_ext::ControllerId,
        mut ctx: crate::bindings::Ctx,
    ) {
        let Self { resources, clients } = self;
        let removed_controller =
            resources.remove(id).expect("controller should only be removed once");

        // Merge the state from all remaining controllers and update Core.
        let (v4, v6) = resources.values().fold(
            (conversion::State::default(), conversion::State::default()),
            |(mut v4, mut v6), controller| {
                v4.merge(&controller.core_state_v4);
                v6.merge(&controller.core_state_v6);
                (v4, v6)
            },
        );
        ctx.api().filter().set_filter_state(v4.into(), v6.into()).expect(
            "removing an existing controller should never cause an error; all remaining state was \
            already validated when created, and controller removal should be self-contained as \
            resources can only refer to other resources owned by the same controller",
        );

        // Notify all existing watchers of the removed resources.
        let events = removed_controller
            .existing_ids()
            .map(|resource| fnet_filter_ext::Event::Removed(id.clone(), resource))
            .chain(std::iter::once(fnet_filter_ext::Event::EndOfUpdate))
            .collect::<Vec<_>>()
            .into_iter();
        for client in clients {
            client.send(events.clone());
        }
    }

    fn commit_changes(
        &mut self,
        controller_id: &fnet_filter_ext::ControllerId,
        changes: Vec<fnet_filter_ext::Change>,
        idempotent: bool,
        ebpf_registry: &controller::EbpfProgramRegistry,
        ctx: &mut crate::bindings::Ctx,
    ) -> Result<(), CommitError> {
        let Self { resources, clients } = self;

        // NB: we update NS3 core and broadcast the updates to clients under a
        // single lock to ensure bindings and core see updates in the same
        // order.

        let CommitResult { events, new_state, core_state_v4, core_state_v6 } = resources
            .get(controller_id)
            .expect("controller should not be removed while serving")
            .validate_and_convert_changes(changes, idempotent, ebpf_registry)?;

        // Merge the new Core state from this controller with existing state
        // from all the other controllers. Note that this is an infallible
        // operation because the Core state has already been validated for each
        // controller.
        let (v4, v6) = resources
            .iter()
            .filter_map(|(id, controller)| (id != controller_id).then_some(controller))
            .fold(
                (core_state_v4.clone(), core_state_v6.clone()),
                |(mut v4, mut v6), controller| {
                    v4.merge(&controller.core_state_v4);
                    v6.merge(&controller.core_state_v6);
                    (v4, v6)
                },
            );

        // Attempt to install the new filtering state in Netstack3 Core.
        ctx.api().filter().set_filter_state(v4.into(), v6.into()).map_err(|error| match error {
            netstack3_core::filter::ValidationError::RuleWithInvalidMatcher(rule_id) => {
                CommitError::RuleWithInvalidMatcher(rule_id)
            }
            netstack3_core::filter::ValidationError::RuleWithInvalidAction(rule_id) => {
                CommitError::RuleWithInvalidAction(rule_id)
            }
            netstack3_core::filter::ValidationError::TransparentProxyWithInvalidMatcher(
                rule_id,
            ) => CommitError::TransparentProxyWithInvalidMatcher(rule_id),
            netstack3_core::filter::ValidationError::RedirectWithInvalidMatcher(rule_id) => {
                CommitError::RedirectWithInvalidMatcher(rule_id)
            }
            netstack3_core::filter::ValidationError::MasqueradeWithInvalidMatcher(rule_id) => {
                CommitError::MasqueradeWithInvalidMatcher(rule_id)
            }
            netstack3_core::filter::ValidationError::RejectWithInvalidMatcher(rule_id) => {
                CommitError::RejectWithInvalidMatcher(rule_id)
            }
        })?;

        if !events.is_empty() {
            #[derive(Default, Debug)]
            struct Counts {
                // This field is seen as dead code by rustc because the `Default` and `Debug` impls
                // are ignored during dead code analysis.
                #[expect(dead_code)]
                namespaces: usize,
                routines: usize,
                jump_actions: usize,
                drop_actions: usize,
                tproxy_actions: usize,
                redirect_actions: usize,
                masquerade_actions: usize,
                mark_actions: usize,
                reject_actions: usize,
            }

            let mut counts = Counts { namespaces: new_state.len(), ..Default::default() };
            let _: &mut Counts = new_state.values().fold(&mut counts, |counts, namespace| {
                counts.routines += namespace.routines.len();
                for routine in namespace.routines.values() {
                    for rule in routine.rules.values() {
                        match rule.action {
                            fnet_filter_ext::Action::Jump(_) => counts.jump_actions += 1,
                            fnet_filter_ext::Action::Drop => counts.drop_actions += 1,
                            fnet_filter_ext::Action::TransparentProxy(_) => {
                                counts.tproxy_actions += 1
                            }
                            fnet_filter_ext::Action::Redirect { .. } => {
                                counts.redirect_actions += 1
                            }
                            fnet_filter_ext::Action::Masquerade { .. } => {
                                counts.masquerade_actions += 1
                            }
                            fnet_filter_ext::Action::Mark { .. } => counts.mark_actions += 1,
                            fnet_filter_ext::Action::Reject(_) => counts.reject_actions += 1,
                            fnet_filter_ext::Action::Accept
                            | fnet_filter_ext::Action::Return
                            | fnet_filter_ext::Action::None => {}
                        }
                    }
                }
                counts
            });
            info!("summary of updated filtering state for {controller_id:?}: {counts:?}");
        }

        // Only if validation was successful do we actually commit the changes.
        // This ensures that the state will never be only partially updated.
        resources
            .get_mut(controller_id)
            .expect("controller should not be removed while serving")
            .apply_new_state(new_state, core_state_v4, core_state_v6);

        // Notify all existing watchers of the update, if it was not a no-op.
        if !events.is_empty() {
            debug!("changes to filtering state for {controller_id:?}: {events:?}");

            let events = events
                .into_iter()
                .map(|event| match event {
                    controller::Event::Added(resource) => {
                        fnet_filter_ext::Event::Added(controller_id.clone(), resource)
                    }
                    controller::Event::Removed(resource) => {
                        fnet_filter_ext::Event::Removed(controller_id.clone(), resource)
                    }
                })
                .chain(std::iter::once(fnet_filter_ext::Event::EndOfUpdate));
            for client in clients {
                client.send(events.clone());
            }
        }

        Ok(())
    }

    fn connect_new_client(&mut self) -> Watcher {
        let Self { resources, clients } = self;
        let (watcher, sink) = Watcher::new_with_existing_resources(resources);
        clients.push(sink);
        watcher
    }

    fn disconnect_client(&mut self, watcher: Watcher) {
        let Self { resources: _, clients } = self;
        let (i, _): (usize, &WatcherSink) = Itertools::exactly_one(
            clients.iter().enumerate().filter(|(_i, client)| client.is_connected_to(&watcher)),
        )
        .expect("watcher should be connected to exactly one sink");
        let _: WatcherSink = clients.swap_remove(i);
    }
}

#[derive(Debug)]
struct WatcherSink {
    sink: mpsc::Sender<fnet_filter_ext::Event>,
    cancel: async_utils::event::Event,
}

impl WatcherSink {
    fn send(&mut self, changes: impl Iterator<Item = fnet_filter_ext::Event>) {
        for change in changes {
            self.sink.try_send(change).unwrap_or_else(|e| {
                if e.is_full() {
                    if self.cancel.signal() {
                        warn!(
                            "too many unconsumed events (the client may not be \
                            calling Watch frequently enough): {}",
                            MAX_PENDING_EVENTS
                        )
                    }
                } else {
                    panic!("unexpected error trying to send: {:?}", e)
                }
            })
        }
    }

    fn is_connected_to(&self, watcher: &Watcher) -> bool {
        self.sink.is_connected_to(&watcher.receiver)
    }
}

struct Watcher {
    existing_resources: <Vec<fnet_filter_ext::Event> as std::iter::IntoIterator>::IntoIter,
    receiver: mpsc::Receiver<fnet_filter_ext::Event>,
    canceled: async_utils::event::Event,
}

impl Watcher {
    fn new_with_existing_resources(
        resources: &HashMap<fnet_filter_ext::ControllerId, Controller>,
    ) -> (Self, WatcherSink) {
        let existing_resources = resources
            .iter()
            .flat_map(|(controller, resources)| {
                resources
                    .existing_resources()
                    .map(|resource| fnet_filter_ext::Event::Existing(controller.clone(), resource))
            })
            .chain(std::iter::once(fnet_filter_ext::Event::Idle))
            .collect::<Vec<_>>()
            .into_iter();
        let (sender, receiver) = mpsc::channel(MAX_PENDING_EVENTS);
        let cancel = async_utils::event::Event::new();

        (
            Watcher { existing_resources, receiver, canceled: cancel.clone() },
            WatcherSink { sink: sender, cancel },
        )
    }

    fn watch(&mut self) -> impl futures::Future<Output = Vec<fnet_filter_ext::Event>> + Unpin + '_ {
        let Self { existing_resources, receiver, canceled: _ } = self;
        stream::iter(existing_resources)
            .chain(receiver)
            .ready_chunks(fnet_filter::MAX_BATCH_SIZE.into())
            .into_future()
            .map(|(r, _ready_chunks)| r.expect("underlying event stream unexpectedly ended"))
    }
}

pub(crate) async fn serve_state(
    mut stream: fnet_filter::StateRequestStream,
    dispatcher: UpdateDispatcher,
) -> Result<(), fidl::Error> {
    while let Some(request) = stream.try_next().await? {
        match request {
            fnet_filter::StateRequest::GetWatcher { options: _, request, control_handle: _ } => {
                let requests = request.into_stream();
                fasync::Scope::current().spawn_request_stream_handler(requests, |requests| {
                    serve_watcher(requests, dispatcher.clone())
                })
            }
        }
    }
    Ok(())
}

#[derive(Debug, Error)]
enum ServeWatcherError {
    #[error("the request stream contained a FIDL error")]
    ErrorInStream(fidl::Error),
    #[error("a FIDL error was encountered while sending the response")]
    FailedToRespond(fidl::Error),
    #[error("the client called `Watch` while a previous call was already pending")]
    PreviousPendingWatch,
    #[error("the client was canceled")]
    Canceled,
    #[error("the service scope was canceled")]
    ScopeCanceled,
}

impl ErrorLogExt for ServeWatcherError {
    fn log_level(&self) -> log::Level {
        match self {
            Self::ErrorInStream(fidl) | Self::FailedToRespond(fidl) => fidl.log_level(),
            Self::Canceled | Self::ScopeCanceled | Self::PreviousPendingWatch => log::Level::Warn,
        }
    }
}

async fn serve_watcher(
    stream: fnet_filter::WatcherRequestStream,
    dispatcher: UpdateDispatcher,
) -> Result<(), ServeWatcherError> {
    let scope_guard =
        fasync::Scope::current().active_guard().ok_or(ServeWatcherError::ScopeCanceled)?;
    let UpdateDispatcher(dispatcher) = &dispatcher;
    let mut watcher = dispatcher.lock().await.connect_new_client();
    let canceled_fut = watcher.canceled.wait();

    let result = {
        let mut request_stream = stream.map_err(ServeWatcherError::ErrorInStream).fuse();
        let mut canceled_fut = pin!(canceled_fut);
        let mut scope_canceled_fut = pin!(scope_guard.on_cancel().fuse());
        let mut hanging_get = futures::future::OptionFuture::default();
        loop {
            hanging_get = futures::select! {
                request = request_stream.try_next() => match request {
                    Ok(Some(fnet_filter::WatcherRequest::Watch { responder } )) => {
                        if hanging_get.is_terminated() {
                            // Convince the compiler that we're not holding on to a
                            // borrow of `watcher`.
                            drop(hanging_get);

                            // Either there is no pending request or the previously
                            // pending one has completed. We can accept a new
                            // hanging get.
                            Some(watcher.watch().map(move |events| (responder, events))).into()
                        } else {
                            break Err(ServeWatcherError::PreviousPendingWatch);
                        }
                    },
                    Ok(None) => break Ok(()),
                    Err(e) => break Err(e),
                },
                r = hanging_get => {
                    let (responder, events) = r.expect("OptionFuture is not selected if empty");
                    let events = events.into_iter().map(Into::into).collect::<Vec<_>>();
                    match responder.send(&events) {
                        Ok(()) => None.into(),
                        Err(e) => break Err(ServeWatcherError::FailedToRespond(e)),
                    }
                },
                () = canceled_fut => break Err(ServeWatcherError::Canceled),
                () = scope_canceled_fut => break Err(ServeWatcherError::ScopeCanceled),
            };
        }
    };

    dispatcher.lock().await.disconnect_client(watcher);
    // Only drop the scope guard after we've disconnected from the dispatcher,
    // so that we ensure the watcher sink always has a valid sender.
    drop(scope_guard);

    result
}

pub(crate) async fn serve_root(
    mut stream: fnet_root::FilterRequestStream,
    dispatcher: UpdateDispatcher,
    ctx: crate::bindings::Ctx,
) -> Result<(), fidl::Error> {
    use fnet_root::FilterRequest;

    while let Some(request) = stream.try_next().await? {
        match request {
            FilterRequest::OpenController { id, request, control_handle: _ } => {
                let dispatcher = dispatcher.clone();
                let ctx = ctx.clone();
                let UpdateDispatcher(inner) = &dispatcher;
                let id = fnet_filter_ext::ControllerId(id);
                inner.lock().await.connect_or_create_new_controller(id.clone());
                let stream = request.into_stream();
                fasync::Scope::current().spawn_request_stream_handler(stream, |stream| async move {
                    serve_controller(&id, stream, &dispatcher, ctx).await
                })
            }
        }
    }
    // NB: we do not remove the controller on channel closure, because this is a
    // connection made through fuchsia.net.root/Filter, so it has auto-detach
    // behavior.
    Ok(())
}

pub(crate) async fn serve_control(
    mut stream: fnet_filter::ControlRequestStream,
    dispatcher: UpdateDispatcher,
    ctx: crate::bindings::Ctx,
) -> Result<(), fidl::Error> {
    use fnet_filter::ControlRequest;

    while let Some(request) = stream.try_next().await? {
        match request {
            ControlRequest::OpenController { id, request, control_handle: _ } => {
                let dispatcher = dispatcher.clone();
                let UpdateDispatcher(inner) = &dispatcher;
                let final_id = inner.lock().await.new_controller(fnet_filter_ext::ControllerId(id));

                let stream = request.into_stream();
                let ctx = ctx.clone();

                fasync::Scope::current().spawn_request_stream_handler(
                    stream,
                    |stream| async move {
                        let result =
                            serve_controller(&final_id, stream, &dispatcher, ctx.clone()).await;

                        let UpdateDispatcher(inner) = &dispatcher;
                        inner.lock().await.remove_controller(&final_id, ctx);
                        result
                    },
                );
            }
            ControlRequest::ReopenDetachedController { key: _, request: _, control_handle } => {
                error!("TODO(https://fxbug.dev/42182623): detaching is not implemented");
                control_handle.shutdown_with_epitaph(zx::Status::NOT_SUPPORTED);
            }
        }
    }
    Ok(())
}

async fn serve_controller(
    id: &fnet_filter_ext::ControllerId,
    mut stream: fnet_filter::NamespaceControllerRequestStream,
    UpdateDispatcher(dispatcher): &UpdateDispatcher,
    mut ctx: crate::bindings::Ctx,
) -> Result<(), fidl::Error> {
    use fnet_filter::NamespaceControllerRequest;

    let control_handle = stream.control_handle();
    control_handle.send_on_id_assigned(&id.0)?;

    // List of ebpf programs registered by this controller.
    let mut ebpf_registry = controller::EbpfProgramRegistry::new();

    // Set of OnSignal futures waiting on ebpf program handles.
    let mut ebpf_shutdown_signals = stream::FuturesUnordered::new();

    let mut pending_changes = Vec::new();
    loop {
        let request = select! {
            request = stream.try_next() =>  {
                match request? {
                    Some(request) => request,
                    None => break Ok(()),
                }
            },

            // Select `ebpf_shutdown_signals` only if it is not empty.
            id = ebpf_shutdown_signals.select_next_some() => {
                ebpf_registry.remove(id);
                continue;
            }
        };

        match request {
            NamespaceControllerRequest::RegisterEbpfProgram {
                handle: febpf::ProgramHandle { handle },
                program,
                responder,
            } => {
                let program = match program.try_into() {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("encountered an invalid RegisterEbpfProgram request: {:?}", e);
                        responder.send(Err(
                            fnet_filter::RegisterEbpfProgramError::MissingRequiredField,
                        ))?;
                        continue;
                    }
                };
                let id =
                    febpf::ProgramId { id: handle.koid().expect("Failed to get koid").raw_koid() };
                let result = ebpf_registry.register(ctx.bindings_ctx(), id, program);

                if result.is_ok() {
                    // Observe signals on the program handle to remove it from set when necessary.
                    ebpf_shutdown_signals.push(
                        fasync::OnSignals::new(
                            handle,
                            zx::Signals::EVENTPAIR_PEER_CLOSED
                                | zx::Signals::from_bits_truncate(febpf::PROGRAM_DEFUNCT_SIGNAL),
                        )
                        .map(move |result| {
                            match result {
                                // The program is dropped when either of the two signals is raised.
                                Ok(_signals) => (),
                                Err(e) => {
                                    error!(
                                        "Error waiting for signals on an eBPF program handle: {:?}",
                                        e
                                    );
                                }
                            };
                            id
                        }),
                    );
                }

                responder.send(result)?;
            }
            NamespaceControllerRequest::PushChanges { changes, responder } => {
                let num_changes = changes.len();
                if pending_changes.len() + num_changes > fnet_filter::MAX_COMMIT_SIZE.into() {
                    responder.send(fnet_filter::ChangeValidationResult::TooManyChanges(
                        fnet_filter::Empty {},
                    ))?;
                    continue;
                }

                enum Error {
                    ReturnToClient(fnet_filter::ChangeValidationError),
                    Fatal,
                }

                let result = changes
                    .into_iter()
                    .enumerate()
                    .map(|(i, change)| {
                        change.try_into().map_err(|error| {
                            use fnet_filter::ChangeValidationError;
                            use fnet_filter_ext::FidlConversionError;

                            warn!("encountered an invalid filter change at index {i}: {error:?}");

                            let error = match error {
                                FidlConversionError::UnknownUnionVariant(type_name) => {
                                    warn!("client specified unknown variant of {type_name}");
                                    Error::Fatal
                                }
                                FidlConversionError::MissingNamespaceId
                                | FidlConversionError::MissingNamespaceDomain
                                | FidlConversionError::MissingRoutineId
                                | FidlConversionError::MissingRoutineType
                                | FidlConversionError::MissingIpInstallationHook
                                | FidlConversionError::MissingNatInstallationHook => {
                                    Error::ReturnToClient(
                                        ChangeValidationError::MissingRequiredField,
                                    )
                                }
                                FidlConversionError::ZeroInterfaceId => Error::ReturnToClient(
                                    ChangeValidationError::InvalidInterfaceMatcher,
                                ),
                                FidlConversionError::InvalidAddressRange
                                | FidlConversionError::AddressRangeFamilyMismatch
                                | FidlConversionError::SubnetPrefixTooLong
                                | FidlConversionError::SubnetHostBitsSet => Error::ReturnToClient(
                                    ChangeValidationError::InvalidAddressMatcher,
                                ),
                                FidlConversionError::InvalidPortMatcherRange => {
                                    Error::ReturnToClient(ChangeValidationError::InvalidPortMatcher)
                                }
                                FidlConversionError::UnspecifiedTransparentProxyPort => {
                                    Error::ReturnToClient(
                                        ChangeValidationError::InvalidTransparentProxyAction,
                                    )
                                }
                                FidlConversionError::UnspecifiedNatPort => {
                                    Error::ReturnToClient(ChangeValidationError::InvalidNatAction)
                                }
                                FidlConversionError::InvalidPortRange => {
                                    Error::ReturnToClient(ChangeValidationError::InvalidPortRange)
                                }
                                FidlConversionError::NotAnError => unreachable!(
                                    "should not get this error when converting a `Change`"
                                ),
                            };
                            (i, error)
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(|changes| {
                        pending_changes.extend(changes);
                    });

                let result = match result {
                    Ok(()) => fnet_filter::ChangeValidationResult::Ok(fnet_filter::Empty {}),
                    Err((valid_change_count, error)) => {
                        let error = match error {
                            Error::ReturnToClient(error) => error,
                            Error::Fatal => {
                                error!("fatal error serving filtering controller; closing channel");
                                control_handle.shutdown_with_epitaph(zx::Status::NOT_SUPPORTED);
                                return Ok(());
                            }
                        };
                        // Return `Ok` for all valid changes, followed by the error we encountered,
                        // and pad the results with `NotReached` for the remaining changes.
                        let errors = std::iter::repeat(fnet_filter::ChangeValidationError::Ok)
                            .take(valid_change_count)
                            .chain(std::iter::once(error))
                            .chain(
                                std::iter::repeat(fnet_filter::ChangeValidationError::NotReached)
                                    .take(num_changes - valid_change_count - 1),
                            )
                            .collect::<Vec<_>>();
                        fnet_filter::ChangeValidationResult::ErrorOnChange(errors)
                    }
                };
                responder.send(result)?;
            }
            NamespaceControllerRequest::Commit { payload: options, responder } => {
                let fnet_filter::CommitOptions { idempotent, .. } = options;
                let idempotent = idempotent.unwrap_or(false);
                let changes = std::mem::take(&mut pending_changes);
                let num_changes = changes.len();
                let result = dispatcher.lock().await.commit_changes(
                    id,
                    changes,
                    idempotent,
                    &ebpf_registry,
                    &mut ctx,
                );
                let result = match result {
                    Ok(()) => fnet_filter::CommitResult::Ok(fnet_filter::Empty {}),
                    Err(error) => match error {
                        CommitError::RuleWithInvalidMatcher(rule) => {
                            fnet_filter::CommitResult::RuleWithInvalidMatcher(rule.into())
                        }
                        CommitError::RuleWithInvalidAction(rule) => {
                            fnet_filter::CommitResult::RuleWithInvalidAction(rule.into())
                        }
                        CommitError::TransparentProxyWithInvalidMatcher(rule) => {
                            fnet_filter::CommitResult::TransparentProxyWithInvalidMatcher(
                                rule.into(),
                            )
                        }
                        CommitError::RedirectWithInvalidMatcher(rule) => {
                            fnet_filter::CommitResult::RedirectWithInvalidMatcher(rule.into())
                        }
                        CommitError::MasqueradeWithInvalidMatcher(rule) => {
                            fnet_filter::CommitResult::MasqueradeWithInvalidMatcher(rule.into())
                        }
                        CommitError::RejectWithInvalidMatcher(rule) => {
                            fnet_filter::CommitResult::RejectWithInvalidMatcher(rule.into())
                        }
                        CommitError::CyclicalRoutineGraph(routine) => {
                            fnet_filter::CommitResult::CyclicalRoutineGraph(routine.into())
                        }
                        CommitError::ErrorOnChange { index, error } => {
                            let errors = std::iter::repeat(fnet_filter::CommitError::Ok)
                                .take(index)
                                .chain(std::iter::once(error))
                                .chain(
                                    std::iter::repeat(fnet_filter::CommitError::NotReached)
                                        .take(num_changes - index - 1),
                                )
                                .collect::<Vec<_>>();
                            fnet_filter::CommitResult::ErrorOnChange(errors)
                        }
                    },
                };
                responder.send(result)?;
            }
            NamespaceControllerRequest::Detach { responder: _ } => {
                error!("TODO(https://fxbug.dev/42182623): detaching is not implemented");
                control_handle.shutdown_with_epitaph(zx::Status::NOT_SUPPORTED);
            }
        }
    }
}
