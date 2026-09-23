// Copyright 2023 The Fuchsia Authors. All rights reserved>.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::model::actions::StopAction;
use crate::model::component::{
    ExtendedInstance, IncomingCapabilities, StartReason, WeakComponentInstance,
};
use ::runner::component::StopInfo;
use cm_types::FLAGS_MAX_POSSIBLE_RIGHTS;
use errors::OpenExposedDirError;
use fidl::endpoints::{ProtocolMarker, RequestStream};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_io as fio;
use fuchsia_async as fasync;
use fuchsia_trace::{self as trace, duration, instant};
use futures::prelude::*;
use log::{error, warn};
use routing::component_instance::ComponentInstanceInterface;
use vfs::directory::entry::OpenRequest;
use vfs::{Path, ToObjectRequest};

pub async fn run_controller(
    weak_component_instance: WeakComponentInstance,
    stream: fcomponent::ControllerRequestStream,
) {
    if let Err(err) = serve_controller(weak_component_instance, stream).await {
        warn!(err:%; "Error serving {}", fcomponent::ControllerMarker::DEBUG_NAME);
    }
}

pub async fn serve_controller(
    weak_component_instance: WeakComponentInstance,
    mut stream: fcomponent::ControllerRequestStream,
) -> Result<(), fidl::Error> {
    while let Some(request) = stream.try_next().await? {
        match request {
            fcomponent::ControllerRequest::Start { args, execution_controller, responder } => {
                duration!("component_manager", "Controller.Start");
                let Ok(component) = weak_component_instance.upgrade() else {
                    // If the component doesn't exist anymore, then the execution scope we're
                    // scheduled in is being torn down and our task will end momentarily. To not
                    // race with that, let's preemptively wrap things up here and close the
                    // channel.
                    return Ok(());
                };
                let res: Result<(), fcomponent::Error> = async {
                    if component.is_started().await {
                        return Err(fcomponent::Error::InstanceAlreadyStarted);
                    }
                    let execution_controller_stream = execution_controller.into_stream();
                    let control_handle = execution_controller_stream.control_handle();
                    let execution_controller = ExecutionControllerTask {
                        _task: fasync::Task::spawn(execution_controller_task(
                            weak_component_instance.clone(),
                            execution_controller_stream,
                        )),
                        control_handle,
                        stop_payload: None,
                    };
                    let incoming = IncomingCapabilities::from_start_child_args(
                        component.context.remote_capabilities(),
                        args,
                    )?;

                    if let Err(err) = component
                        .start(&StartReason::Controller, Some(execution_controller), incoming)
                        .await
                    {
                        warn!(err:%; "failed to start component");
                        return Err(err.into());
                    }
                    Ok(())
                }
                .await;
                let result = format!("{res:?}");
                responder.send(res)?;
                instant!(
                    c"component_manager", c"Controller.Start/Result", trace::Scope::Process,
                    "result" => result.as_str());
            }
            fcomponent::ControllerRequest::IsStarted { responder } => {
                let Ok(component) = weak_component_instance.upgrade() else {
                    // If the component doesn't exist anymore, then the execution scope we're
                    // scheduled in is being torn down and our task will end momentarily. To not
                    // race with that, let's preemptively wrap things up here and close the
                    // channel.
                    return Ok(());
                };
                responder.send(Ok(component.is_started().await))?;
            }
            fcomponent::ControllerRequest::OpenExposedDir { exposed_dir, responder } => {
                let Ok(component) = weak_component_instance.upgrade() else {
                    // If the component doesn't exist anymore, then the execution scope we're
                    // scheduled in is being torn down and our task will end momentarily. To not
                    // race with that, let's preemptively wrap things up here and close the
                    // channel.
                    return Ok(());
                };
                let response = async {
                    // Resolve child in order to instantiate exposed_dir.
                    component.resolve().await.map_err(|e| {
                        warn!("resolve failed for component {}: {}", component.moniker, e);
                        fcomponent::Error::InstanceCannotResolve
                    })?;
                    // We request the maximum possible rights from the parent directory connection.
                    let flags = FLAGS_MAX_POSSIBLE_RIGHTS | fio::Flags::PROTOCOL_DIRECTORY;
                    let mut object_request = flags.to_object_request(exposed_dir);
                    component
                        .open_exposed(OpenRequest::new(
                            component.execution_scope.clone(),
                            flags,
                            Path::dot(),
                            &mut object_request,
                        ))
                        .await
                        .map_err(|error| match error {
                            OpenExposedDirError::InstanceDestroyed
                            | OpenExposedDirError::InstanceNotResolved => {
                                fcomponent::Error::InstanceDied
                            }
                            OpenExposedDirError::Open(_) => fcomponent::Error::Internal,
                        })?;
                    Ok(())
                }
                .await;
                responder.send(response)?;
            }
            fcomponent::ControllerRequest::GetExposedDictionary { responder } => {
                let Ok(component) = weak_component_instance.upgrade() else {
                    // If the component doesn't exist anymore, then the execution scope we're
                    // scheduled in is being torn down and our task will end momentarily. To not
                    // race with that, let's preemptively wrap things up here and close the
                    // channel.
                    return Ok(());
                };
                let res = async {
                    let resolved = component
                        .lock_resolved_state()
                        .await
                        .map_err(|_| fcomponent::Error::InstanceCannotResolve)?;
                    let exposed_dict =
                        resolved.get_exposed_dict(component.as_weak().into()).await.clone();
                    Ok(exposed_dict.to_fsandbox())
                }
                .await;
                responder.send(res)?;
            }
            fcomponent::ControllerRequest::GetOutputDictionary { responder } => {
                let Ok(component) = weak_component_instance.upgrade() else {
                    // If the component doesn't exist anymore, then the execution scope we're
                    // scheduled in is being torn down and our task will end momentarily. To not
                    // race with that, let's preemptively wrap things up here and close the
                    // channel.
                    return Ok(());
                };
                let res = async {
                    let resolved = component
                        .lock_resolved_state()
                        .await
                        .map_err(|_| fcomponent::Error::InstanceCannotResolve)?;
                    let exposed_dict =
                        resolved.get_exposed_dict(component.as_weak().into()).await.clone();
                    let (e1, e2) = zx::EventPair::create();
                    component
                        .context
                        .remote_capabilities()
                        .store(e1, exposed_dict)
                        .expect("we used a valid handle");
                    Ok(e2)
                }
                .await;
                responder.send(res)?;
            }
            fcomponent::ControllerRequest::Destroy { responder } => {
                let res = (|| {
                    let Ok(component) = weak_component_instance.upgrade() else {
                        return Ok(None);
                    };
                    let Ok(ext_parent) = component.parent.upgrade() else {
                        return Ok(None);
                    };
                    let parent;
                    match ext_parent {
                        ExtendedInstance::AboveRoot(_) => {
                            // This is the root component, which can't be destroyed
                            return Err(fcomponent::Error::AccessDenied);
                        }
                        ExtendedInstance::Component(p) => {
                            parent = p;
                        }
                    }
                    let moniker = component.moniker.clone();
                    if let None = moniker
                        .leaf()
                        .expect("we already checked this is not the root component")
                        .collection()
                    {
                        // Disallow destruction of static children
                        return Err(fcomponent::Error::AccessDenied);
                    }
                    Ok(Some((parent, moniker)))
                })();
                let (parent, moniker) = match res {
                    Ok(Some(v)) => v,
                    Ok(None) => {
                        // Already destroyed.
                        responder.send(Ok(()))?;
                        continue;
                    }
                    Err(e) => {
                        responder.send(Err(e))?;
                        continue;
                    }
                };
                parent.execution_scope.clone().spawn(async move {
                    let child_name =
                        moniker.leaf().expect("we already checked this is not the root component");
                    match parent.remove_dynamic_child(&child_name).await {
                        Ok(()) => {
                            _ = responder.send(Ok(()));
                        }
                        Err(err) => {
                            warn!(err:%, moniker:%;
                                  "Controller/Destroy: component destruction unexpectedly failed");
                            _ = responder.send(Err(fcomponent::Error::Internal));
                        }
                    }
                });
            }
            fcomponent::ControllerRequest::_UnknownMethod { ordinal, .. } => {
                warn!(ordinal:%; "fuchsia.component/Controller received unknown method");
            }
        }
    }
    Ok(())
}

async fn execution_controller_task(
    weak_component_instance: WeakComponentInstance,
    mut stream: fcomponent::ExecutionControllerRequestStream,
) {
    while let Ok(Some(request)) = stream.try_next().await {
        match request {
            fcomponent::ExecutionControllerRequest::Stop { control_handle: _ } => {
                let component = weak_component_instance.upgrade();
                if component.is_err() {
                    return;
                }
                component.unwrap().actions().register_no_wait(StopAction::new(false)).await;
            }
            fcomponent::ExecutionControllerRequest::_UnknownMethod { ordinal, .. } => {
                warn!(ordinal:%; "fuchsia.component/ExecutionController received unknown method");
            }
        }
    }
}

pub struct ExecutionControllerTask {
    _task: fasync::Task<()>,
    control_handle: fcomponent::ExecutionControllerControlHandle,
    stop_payload: Option<StopInfo>,
}

impl Drop for ExecutionControllerTask {
    fn drop(&mut self) {
        match self.stop_payload.as_ref() {
            Some(payload) => {
                // There's not much we can do if the other end has closed their channel
                let _ = self.control_handle.send_on_stop(&fcomponent::StoppedPayload {
                    status: Some(zx::Status::result_into_raw(payload.termination_status)),
                    exit_code: payload.exit_code,
                    ..Default::default()
                });
            }
            None => {
                // TODO(https://fxbug.dev/42081036): stop_payload is not when system is shutting down
                error!("stop_payload was not set before the ExecutionControllerTask was dropped");
            }
        }
    }
}

impl ExecutionControllerTask {
    pub fn set_stop_payload(&mut self, info: StopInfo) {
        self.stop_payload = Some(info);
    }
}
