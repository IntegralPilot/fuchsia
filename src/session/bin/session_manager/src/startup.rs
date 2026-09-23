// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::cobalt;
use fidl::endpoints::{ServerEnd, create_proxy};
use fidl_fuchsia_component as fcomponent;
use fidl_fuchsia_component_decl as fdecl;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_session as fsession;
use fuchsia_async as fasync;
use fuchsia_component::runtime::{Data, DataValue, Dictionary};
use log::info;
use thiserror::Error;

/// Errors returned by calls startup functions.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum StartupError {
    #[error("Existing session not destroyed at \"{}/{}\": {:?}", collection, name, err)]
    NotDestroyed { name: String, collection: String, err: fcomponent::Error },

    #[error("Session {} not created at \"{}/{}\": Bedrock error {:?}", url, collection, name, err)]
    BedrockError { name: String, collection: String, url: String, err: String },

    #[error("Session {} not created at \"{}/{}\": {:?}", url, collection, name, err)]
    NotCreated { name: String, collection: String, url: String, err: fcomponent::Error },

    #[error(
        "Exposed directory of session {} at \"{}/{}\" not opened: {:?}",
        url,
        collection,
        name,
        err
    )]
    ExposedDirNotOpened { name: String, collection: String, url: String, err: fcomponent::Error },

    #[error("Session {} not launched at \"{}/{}\": {:?}", url, collection, name, err)]
    NotLaunched { name: String, collection: String, url: String, err: fcomponent::Error },

    #[error("Attempt to restart a not running session")]
    NotRunning,
}

impl From<StartupError> for fsession::LaunchError {
    fn from(e: StartupError) -> fsession::LaunchError {
        match e {
            StartupError::NotDestroyed { .. } => fsession::LaunchError::DestroyComponentFailed,
            StartupError::NotCreated { err, .. } => match err {
                fcomponent::Error::InstanceCannotResolve => fsession::LaunchError::NotFound,
                _ => fsession::LaunchError::CreateComponentFailed,
            },
            StartupError::ExposedDirNotOpened { .. }
            | StartupError::BedrockError { .. }
            | StartupError::NotLaunched { .. } => fsession::LaunchError::CreateComponentFailed,
            StartupError::NotRunning => fsession::LaunchError::NotFound,
        }
    }
}

impl From<StartupError> for fsession::RestartError {
    fn from(e: StartupError) -> fsession::RestartError {
        match e {
            StartupError::NotDestroyed { .. } => fsession::RestartError::DestroyComponentFailed,
            StartupError::NotCreated { err, .. } => match err {
                fcomponent::Error::InstanceCannotResolve => fsession::RestartError::NotFound,
                _ => fsession::RestartError::CreateComponentFailed,
            },
            StartupError::ExposedDirNotOpened { .. }
            | StartupError::BedrockError { .. }
            | StartupError::NotLaunched { .. } => fsession::RestartError::CreateComponentFailed,
            StartupError::NotRunning => fsession::RestartError::NotRunning,
        }
    }
}

impl From<StartupError> for fsession::LifecycleError {
    fn from(e: StartupError) -> fsession::LifecycleError {
        match e {
            StartupError::NotDestroyed { .. } => fsession::LifecycleError::DestroyComponentFailed,
            StartupError::NotCreated { err, .. } => match err {
                fcomponent::Error::InstanceCannotResolve => {
                    fsession::LifecycleError::ResolveComponentFailed
                }
                _ => fsession::LifecycleError::CreateComponentFailed,
            },
            StartupError::ExposedDirNotOpened { .. }
            | StartupError::BedrockError { .. }
            | StartupError::NotLaunched { .. } => fsession::LifecycleError::CreateComponentFailed,
            StartupError::NotRunning => fsession::LifecycleError::NotFound,
        }
    }
}

/// The name of the session child component.
const SESSION_NAME: &str = "session";

/// The name of the child collection the session is added to, must match the declaration in
/// `session_manager.cml`.
const SESSION_CHILD_COLLECTION: &str = "session";

/// Launches the specified session.
///
/// Any existing session child will be destroyed prior to launching the new session.
///
/// Returns a controller for the session component, or an error.
///
/// # Parameters
/// - `session_url`: The URL of the session to launch.
/// - `config_capabilities`: Configuration capabilities that will target the session.
/// - `exposed_dir`: The server end on which to serve the session's exposed directory.
/// - `realm`: The realm in which to launch the session.
///
/// # Errors
/// If there was a problem creating or binding to the session component instance.
pub async fn launch_session(
    session_url: &str,
    config_capabilities: Vec<fdecl::Configuration>,
    exposed_dir: ServerEnd<fio::DirectoryMarker>,
    realm: &fcomponent::RealmProxy,
) -> Result<fcomponent::ExecutionControllerProxy, StartupError> {
    info!(session_url; "Launching session");

    let start_time = zx::MonotonicInstant::get();
    let controller = set_session(session_url, config_capabilities, realm, exposed_dir).await?;
    let end_time = zx::MonotonicInstant::get();

    fasync::Task::local(async move {
        if let Ok(cobalt_logger) = cobalt::get_logger() {
            // The result is disregarded as there is not retry-logic if it fails, and the error is
            // not meant to be fatal.
            let _ = cobalt::log_session_launch_time(cobalt_logger, start_time, end_time).await;
        }
    })
    .detach();

    Ok(controller)
}

/// Stops the current session, if any.
///
/// # Parameters
/// - `realm`: The realm in which the session exists.
///
/// # Errors
/// `StartupError::NotDestroyed` if the session component could not be destroyed.
pub async fn stop_session(realm: &fcomponent::RealmProxy) -> Result<(), StartupError> {
    realm_management::destroy_child_component(SESSION_NAME, SESSION_CHILD_COLLECTION, realm)
        .await
        .map_err(|err| StartupError::NotDestroyed {
            name: SESSION_NAME.to_string(),
            collection: SESSION_CHILD_COLLECTION.to_string(),
            err,
        })
}

async fn create_config_dict(
    config_capabilities: Vec<fdecl::Configuration>,
) -> Result<Option<Dictionary>, anyhow::Error> {
    if config_capabilities.is_empty() {
        return Ok(None);
    }
    let dictionary = Dictionary::new().await;
    for config in config_capabilities {
        let Some(value) = config.value else { continue };
        let Some(key) = config.name else { continue };
        let data = Data::new(DataValue::Bytes(fidl::persist(&value)?)).await;
        dictionary.insert(&key, data).await;
    }
    Ok(Some(dictionary))
}

/// Sets the currently active session.
///
/// If an existing session is running, the session's component instance will be destroyed prior to
/// creating the new session, effectively replacing the session.
///
/// # Parameters
/// - `session_url`: The URL of the session to instantiate.
/// - `config_capabilities`: Configuration capabilities that will target the session.
/// - `realm`: The realm in which to create the session.
/// - `exposed_dir`: The server end on which the session's exposed directory will be served.
///
/// # Errors
/// Returns an error if any of the realm operations fail, or the realm is unavailable.
async fn set_session(
    session_url: &str,
    config_capabilities: Vec<fdecl::Configuration>,
    realm: &fcomponent::RealmProxy,
    exposed_dir: ServerEnd<fio::DirectoryMarker>,
) -> Result<fcomponent::ExecutionControllerProxy, StartupError> {
    realm_management::destroy_child_component(SESSION_NAME, SESSION_CHILD_COLLECTION, realm)
        .await
        .or_else(|err: fcomponent::Error| match err {
            // Since the intent is simply to clear out the existing session child if it exists,
            // related errors are disregarded.
            fcomponent::Error::InvalidArguments
            | fcomponent::Error::InstanceNotFound
            | fcomponent::Error::CollectionNotFound => Ok(()),
            _ => Err(err),
        })
        .map_err(|err| StartupError::NotDestroyed {
            name: SESSION_NAME.to_string(),
            collection: SESSION_CHILD_COLLECTION.to_string(),
            err,
        })?;

    let (controller, controller_server_end) = create_proxy::<fcomponent::ControllerMarker>();
    let dictionary = create_config_dict(config_capabilities).await.map_err(|err| {
        StartupError::BedrockError {
            name: SESSION_NAME.to_string(),
            collection: SESSION_CHILD_COLLECTION.to_string(),
            url: session_url.to_string(),
            err: format!("{err:#?}"),
        }
    })?;
    let create_child_args = fcomponent::CreateChildArgs {
        controller: Some(controller_server_end),
        additional_inputs: dictionary.map(|dictionary| dictionary.handle),
        ..Default::default()
    };
    realm_management::create_child_component(
        SESSION_NAME,
        session_url,
        SESSION_CHILD_COLLECTION,
        create_child_args,
        realm,
    )
    .await
    .map_err(|err| StartupError::NotCreated {
        name: SESSION_NAME.to_string(),
        collection: SESSION_CHILD_COLLECTION.to_string(),
        url: session_url.to_string(),
        err,
    })?;

    realm_management::open_child_component_exposed_dir(
        SESSION_NAME,
        SESSION_CHILD_COLLECTION,
        realm,
        exposed_dir,
    )
    .await
    .map_err(|err| StartupError::ExposedDirNotOpened {
        name: SESSION_NAME.to_string(),
        collection: SESSION_CHILD_COLLECTION.to_string(),
        url: session_url.to_string(),
        err,
    })?;

    // Start the component.
    let (execution_controller, execution_controller_server_end) =
        create_proxy::<fcomponent::ExecutionControllerMarker>();
    controller
        .start(fcomponent::StartChildArgs::default(), execution_controller_server_end)
        .await
        .map_err(|_| fcomponent::Error::Internal)
        .and_then(std::convert::identity)
        .map_err(|_err| StartupError::NotLaunched {
            name: SESSION_NAME.to_string(),
            collection: SESSION_CHILD_COLLECTION.to_string(),
            url: session_url.to_string(),
            err: fcomponent::Error::InstanceCannotStart,
        })?;

    Ok(execution_controller)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::{SESSION_CHILD_COLLECTION, SESSION_NAME, set_session, stop_session};
    use anyhow::Error;
    use fidl::endpoints::create_endpoints;
    use fidl_fuchsia_component as fcomponent;
    use fidl_fuchsia_io as fio;
    use fidl_test_util::spawn_stream_handler;
    use session_testing::{spawn_directory_server, spawn_server};
    use std::sync::LazyLock;
    use test_util::Counter;

    #[fuchsia::test]
    async fn set_session_calls_realm_methods_in_appropriate_order() -> Result<(), Error> {
        // The number of realm calls which have been made so far.
        static NUM_REALM_REQUESTS: LazyLock<Counter> = LazyLock::new(|| Counter::new(0));

        let session_url = "session";

        let directory_request_handler = move |directory_request| match directory_request {
            fio::DirectoryRequest::Open { .. } => {
                assert_eq!(NUM_REALM_REQUESTS.get(), 4);
            }
            _ => panic!("Directory handler received an unexpected request"),
        };

        let realm = spawn_stream_handler(move |realm_request| async move {
            match realm_request {
                fcomponent::RealmRequest::DestroyChild { child, responder } => {
                    assert_eq!(NUM_REALM_REQUESTS.get(), 0);
                    assert_eq!(child.collection, Some(SESSION_CHILD_COLLECTION.to_string()));
                    assert_eq!(child.name, SESSION_NAME);

                    let _ = responder.send(Ok(()));
                }
                fcomponent::RealmRequest::CreateChild { collection, decl, args, responder } => {
                    assert_eq!(NUM_REALM_REQUESTS.get(), 1);
                    assert_eq!(decl.url.unwrap(), session_url);
                    assert_eq!(decl.name.unwrap(), SESSION_NAME);
                    assert_eq!(&collection.name, SESSION_CHILD_COLLECTION);

                    spawn_server(args.controller.unwrap(), move |controller_request| {
                        match controller_request {
                            fcomponent::ControllerRequest::Start { responder, .. } => {
                                let _ = responder.send(Ok(()));
                            }
                            fcomponent::ControllerRequest::IsStarted { .. } => unimplemented!(),
                            fcomponent::ControllerRequest::GetExposedDictionary { .. } => {
                                unimplemented!()
                            }
                            fcomponent::ControllerRequest::GetOutputDictionary { .. } => {
                                unimplemented!()
                            }
                            fcomponent::ControllerRequest::OpenExposedDir { .. } => {
                                unimplemented!()
                            }
                            fcomponent::ControllerRequest::Destroy { .. } => {
                                unimplemented!()
                            }
                            fcomponent::ControllerRequest::_UnknownMethod { .. } => {
                                unimplemented!()
                            }
                        }
                    });

                    let _ = responder.send(Ok(()));
                }
                fcomponent::RealmRequest::OpenExposedDir { child, exposed_dir, responder } => {
                    assert_eq!(NUM_REALM_REQUESTS.get(), 2);
                    assert_eq!(child.collection, Some(SESSION_CHILD_COLLECTION.to_string()));
                    assert_eq!(child.name, SESSION_NAME);

                    spawn_directory_server(exposed_dir, directory_request_handler);
                    let _ = responder.send(Ok(()));
                }
                _ => panic!("Realm handler received an unexpected request"),
            }
            NUM_REALM_REQUESTS.inc();
        });

        let (_exposed_dir, exposed_dir_server_end) = create_endpoints::<fio::DirectoryMarker>();
        let _controller = set_session(session_url, vec![], &realm, exposed_dir_server_end).await?;

        Ok(())
    }

    #[fuchsia::test]
    async fn set_session_starts_component() -> Result<(), Error> {
        static NUM_START_CALLS: LazyLock<Counter> = LazyLock::new(|| Counter::new(0));

        let session_url = "session";

        let realm = spawn_stream_handler(move |realm_request| async move {
            match realm_request {
                fcomponent::RealmRequest::DestroyChild { responder, .. } => {
                    let _ = responder.send(Ok(()));
                }
                fcomponent::RealmRequest::CreateChild { args, responder, .. } => {
                    spawn_server(args.controller.unwrap(), move |controller_request| {
                        match controller_request {
                            fcomponent::ControllerRequest::Start { responder, .. } => {
                                NUM_START_CALLS.inc();
                                let _ = responder.send(Ok(()));
                            }
                            fcomponent::ControllerRequest::IsStarted { .. } => unimplemented!(),
                            fcomponent::ControllerRequest::GetExposedDictionary { .. } => {
                                unimplemented!()
                            }
                            fcomponent::ControllerRequest::GetOutputDictionary { .. } => {
                                unimplemented!()
                            }
                            fcomponent::ControllerRequest::OpenExposedDir { .. } => {
                                unimplemented!()
                            }
                            fcomponent::ControllerRequest::Destroy { .. } => {
                                unimplemented!()
                            }
                            fcomponent::ControllerRequest::_UnknownMethod { .. } => {
                                unimplemented!()
                            }
                        }
                    });
                    let _ = responder.send(Ok(()));
                }
                fcomponent::RealmRequest::OpenExposedDir { responder, .. } => {
                    let _ = responder.send(Ok(()));
                }
                _ => panic!("Realm handler received an unexpected request"),
            }
        });

        let (_exposed_dir, exposed_dir_server_end) = create_endpoints::<fio::DirectoryMarker>();
        let _controller = set_session(session_url, vec![], &realm, exposed_dir_server_end).await?;
        assert_eq!(NUM_START_CALLS.get(), 1);

        Ok(())
    }

    #[fuchsia::test]
    async fn stop_session_calls_destroy_child() -> Result<(), Error> {
        static NUM_DESTROY_CHILD_CALLS: LazyLock<Counter> = LazyLock::new(|| Counter::new(0));

        let realm = spawn_stream_handler(move |realm_request| async move {
            match realm_request {
                fcomponent::RealmRequest::DestroyChild { child, responder } => {
                    assert_eq!(NUM_DESTROY_CHILD_CALLS.get(), 0);
                    assert_eq!(child.collection, Some(SESSION_CHILD_COLLECTION.to_string()));
                    assert_eq!(child.name, SESSION_NAME);

                    let _ = responder.send(Ok(()));
                }
                _ => panic!("Realm handler received an unexpected request"),
            }
            NUM_DESTROY_CHILD_CALLS.inc();
        });

        stop_session(&realm).await?;
        assert_eq!(NUM_DESTROY_CHILD_CALLS.get(), 1);

        Ok(())
    }
}
