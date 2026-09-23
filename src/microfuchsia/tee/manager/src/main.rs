// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Error};
use fidl_fuchsia_io as fio;
use fidl_fuchsia_tee::ApplicationRequestStream;
use fuchsia_async as fasync;
use fuchsia_component::client::{connect_to_protocol, connect_to_protocol_at_dir_root};
use fuchsia_component::server::ServiceFs;
use fuchsia_tee_manager_config::TAConfig;
use futures::prelude::*;
use tee_internal::Error as TeeError;

struct TAConnectRequest {
    uuid: String,
    stream: ApplicationRequestStream,
}

async fn run_application(mut request: TAConnectRequest, config: TAConfig) {
    // TODO: Check the config to see if this is singleInstance. If so, look for an existing
    // connection to this TA and connect to it.

    let child_name = request.uuid.to_string(); // TODO: This won't work for multi instance TAs
    let realm = connect_to_protocol::<fidl_fuchsia_component::RealmMarker>()
        .expect("connecting to Realm protocol");
    let child_decl = fidl_fuchsia_component_decl::Child {
        name: Some(child_name.to_string()),
        url: Some(config.url),
        startup: Some(fidl_fuchsia_component_decl::StartupMode::Eager),
        ..Default::default()
    };
    let (child_controller, child_controller_server) = fidl::endpoints::create_proxy();
    let create_child_args = fidl_fuchsia_component::CreateChildArgs {
        controller: Some(child_controller_server),
        ..Default::default()
    };
    if let Err(e) = realm
        .create_child(
            &fidl_fuchsia_component_decl::CollectionRef { name: "ta".to_string() },
            &child_decl,
            create_child_args,
        )
        .await
    {
        log::error!("Could not create child component in TA collection: {e:?}");
        return;
    }

    let (ta_exposed_dir, ta_exposed_dir_server) = fidl::endpoints::create_proxy();
    if let Err(e) = realm
        .open_exposed_dir(
            &fidl_fuchsia_component_decl::ChildRef {
                name: child_name,
                collection: Some("ta".to_string()),
            },
            ta_exposed_dir_server,
        )
        .await
    {
        log::error!("Could not open exposed directory on child component: {e:?}");
        return;
    }

    let ta =
        connect_to_protocol_at_dir_root::<fidl_fuchsia_tee::ApplicationMarker>(&ta_exposed_dir)
            .expect("connecting to Application protocol on child TA");

    use fidl_fuchsia_tee as ftee;
    fn overwrite_return_origin(mut result: ftee::OpResult) -> ftee::OpResult {
        result.return_origin = Some(ftee::ReturnOrigin::TrustedApplication);
        result
    }

    while let Some(request) = request.stream.next().await {
        match request {
            Ok(ftee::ApplicationRequest::OpenSession2 { parameter_set, responder }) => {
                let _ = match ta.open_session2(parameter_set).await {
                    Ok((code, result)) => responder.send(code, overwrite_return_origin(result)),
                    Err(_) => responder.send(
                        0,
                        ftee::OpResult {
                            return_code: Some(TeeError::TargetDead as u64),
                            return_origin: Some(ftee::ReturnOrigin::TrustedOs),
                            ..Default::default()
                        },
                    ),
                };
            }
            Ok(ftee::ApplicationRequest::InvokeCommand {
                session_id,
                command_id,
                parameter_set,
                responder,
            }) => {
                let _ = match ta.invoke_command(session_id, command_id, parameter_set).await {
                    Ok(result) => responder.send(overwrite_return_origin(result)),
                    Err(_) => responder.send(ftee::OpResult {
                        return_code: Some(TeeError::TargetDead as u64),
                        return_origin: Some(ftee::ReturnOrigin::TrustedOs),
                        ..Default::default()
                    }),
                };
            }
            Ok(ftee::ApplicationRequest::CloseSession { session_id, responder }) => {
                // This always succeeds - even if the TA isn't there we report success.
                let _ = ta.close_session(session_id).await;
                let _ = responder.send();
            }
            Err(_) => break,
        }
    }
    // TODO: We may need to keep the instance alive when we implement the singleInstance and
    // instanceKeepAlive properties. For now, explicitly drop the child controller when the client
    // disconnects.
    std::mem::drop(child_controller);
}

#[fuchsia::main]
async fn main() -> Result<(), Error> {
    let mut configs = HashMap::new();
    if let Ok(d) = std::fs::read_dir("/config") {
        let uuid_filenames = d
            .filter(|entry| entry.is_ok())
            .map(|entry| entry.unwrap().path())
            .filter(|path| !path.is_dir())
            .map(|entry| (entry.clone(), entry.file_name().unwrap().to_os_string()))
            .collect::<Vec<_>>();
        for (path, file) in uuid_filenames {
            let uuid = Path::new(&file)
                .file_stem()
                .ok_or_else(|| anyhow::anyhow!("Expected path with extension"))?;
            let config = TAConfig::parse_config(&path)?;
            let _ = configs.insert(
                uuid.to_str()
                    .ok_or_else(|| anyhow::anyhow!("UUID string did not decode"))?
                    .to_string(),
                config,
            );
        }
    }

    let mut fs = ServiceFs::new_local();
    let mut svc_dir = fs.dir("svc");
    let mut ta_dir = svc_dir.dir("ta");

    for uuid in configs.keys() {
        let _ = ta_dir.dir(uuid).add_fidl_service(move |stream: ApplicationRequestStream| {
            TAConnectRequest { uuid: uuid.to_string(), stream }
        });
    }

    let system_props_file = fuchsia_fs::file::open_in_namespace(
        "/pkg/data/properties/system_properties",
        fio::PERM_READABLE,
    )
    .context("Failed to open system properties")?;
    let vmo = system_props_file
        .get_backing_memory(fio::VmoFlags::READ | fio::VmoFlags::PRIVATE_CLONE)
        .await?
        .map_err(fidl::Status::err_from_raw)
        .context("Failed to get system properties vmo")?;

    let mut data_dir = fs.dir("data");
    let mut properties_dir = data_dir.dir("properties");
    let _ = properties_dir.add_vmo_file_at("system_properties", vmo);

    let _ = fs.take_and_serve_directory_handle()?;

    let application_scope = fasync::Scope::new();

    while let Some(request) = fs.next().await {
        match configs.get(&request.uuid) {
            Some(config) => {
                let _ = application_scope
                    .spawn(fasync::Task::spawn(run_application(request, config.clone())));
            }
            None => {
                log::warn!("Received connection request for unknown UUID {:?}", request.uuid);
            }
        }
    }

    application_scope.join().await;

    Ok(())
}

#[cfg(test)]
pub mod tests {
    use std::path::Path;
    use tee_properties::{PropEnumerator, PropSet, PropType};

    // This test validates the base platform-specified properties which are hosted in TA manager.
    #[test]
    fn validate_system_properties() {
        let properties_file_path = Path::new("/pkg/data/properties/system_properties");

        let prop_set =
            PropSet::from_config_file(&properties_file_path).expect("prop set loads successfully");
        let mut prop_enum = PropEnumerator::new();
        prop_enum.start(std::sync::Arc::new(prop_set));

        // Will report ItemNotFound error if it has moved past the end of the property set.
        while prop_enum.get_property_name().is_ok() {
            let prop_type: PropType =
                prop_enum.get_property_type().expect("get property type successfully");
            let parse_success = match prop_type {
                PropType::BinaryBlock => prop_enum.get_property_as_binary_block().is_ok(),
                PropType::UnsignedInt32 => prop_enum.get_property_as_u32().is_ok(),
                PropType::UnsignedInt64 => prop_enum.get_property_as_u64().is_ok(),
                PropType::Boolean => prop_enum.get_property_as_bool().is_ok(),
                PropType::Uuid => prop_enum.get_property_as_uuid().is_ok(),
                PropType::Identity => prop_enum.get_property_as_identity().is_ok(),
                PropType::String => prop_enum.get_property_as_string().is_ok(),
            };
            assert!(
                parse_success,
                "Bad value found, parsing expected to fail: {:?}",
                prop_enum.get_property_name().expect("get name successfully")
            );
            // next() returns item not found error when it reaches past the end as well.
            if prop_enum.next().is_err() {
                break;
            }
        }
    }
}
