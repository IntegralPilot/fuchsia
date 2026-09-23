// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context, Result, bail, format_err};
use async_utils::async_once::Once;
use cm_types::Name;
use fidl::AsHandleRef;
use fidl::endpoints::ServerEnd;
use fidl_fuchsia_data as fdata;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_process as fprocess;
use fidl_fuchsia_process_lifecycle as fpl;
use fuchsia_component::directory::AsRefDirectory;
use fuchsia_component::server::{ServiceFs, ServiceObj, ServiceObjTrait};
use fuchsia_fs::directory::{WatchEvent, Watcher};
use futures::prelude::*;
use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;

/// Safely extracts a string from a VFS event without allocating
/// unless ownership is explicitly required.
fn extract_event_filename<'a>(path: &'a Path) -> Option<Cow<'a, str>> {
    let s = path.to_str()?;
    if s == "." {
        // Return a borrowed reference, no allocation
        Some(Cow::Borrowed("."))
    } else {
        // We can still borrow here, deferring .to_owned() until the caller
        // proves they need it.
        Some(Cow::Borrowed(s))
    }
}

async fn wait_for_first_instance(svc: &fio::DirectoryProxy) -> Result<String> {
    const INPUT_SERVICE: &str = "input";
    let (service_dir, request) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
    svc.as_ref_directory().open(
        INPUT_SERVICE,
        fio::Flags::PROTOCOL_DIRECTORY | fio::PERM_READABLE,
        request.into(),
    )?;
    let watcher = Watcher::new(&service_dir).await.context("failed to create watcher")?;

    let mut stream =
        watcher.map(|result| result.context("failed to get watcher event")).try_filter_map(|msg| {
            futures::future::ok(match msg.event {
                WatchEvent::EXISTING | WatchEvent::ADD_FILE => {
                    let filename = extract_event_filename(msg.filename.as_path())
                        .expect("filename must be valid utf8");

                    if filename.as_ref() == "." { None } else { Some(filename.into_owned()) }
                }
                _ => None,
            })
        });

    let first = stream.try_next().await?.ok_or_else(|| {
        format_err!("Watcher stream closed unexpectedly before finding an instance")
    })?;

    Ok(format!("{INPUT_SERVICE}/{first}"))
}

async fn connect_request(
    svc: &fio::DirectoryProxy,
    request: zx::Channel,
    protocol_name: &Name,
    instance_dir: &str,
) {
    let target_path = format!("{instance_dir}/{}", protocol_name.as_str());

    if let Err(e) = svc.as_ref_directory().open(&target_path, fio::Flags::PROTOCOL_SERVICE, request)
    {
        log::error!("[service-broker] Failed to forward connection to {target_path}: {e}");
    }
}

async fn first_instance_to_protocol<'a>(
    svc: fio::DirectoryProxy,
    fs: &mut ServiceFs<ServiceObj<'a, ()>>,
    protocol_name: Name,
    scope: &'a fuchsia_async::Scope,
) -> Result<()> {
    let cached_instance: Arc<Once<String>> = Arc::new(Once::new());
    let svc_arc = Arc::new(svc);

    fs.dir("svc").add_service_at("output", move |request: zx::Channel| {
        let svc = Arc::clone(&svc_arc);
        let protocol_name = protocol_name.clone();
        let cached_instance = Arc::clone(&cached_instance);

        scope.spawn(async move {
            // Safely initializes the path once. All subsequent connection requests
            // will instantly resolve the string without hitting the filesystem.
            let init_future = async || wait_for_first_instance(&svc).await;

            match cached_instance.get_or_try_init(init_future).await {
                Ok(instance_dir) => {
                    connect_request(&svc, request, &protocol_name, instance_dir).await;
                }
                Err(e) => {
                    log::error!(
                        "[service-broker] Failed to resolve first instance: {e}, {protocol_name}"
                    );
                }
            }
        });

        Some(())
    });

    Ok(())
}

async fn first_instance_to_default<T: ServiceObjTrait>(
    svc: fio::DirectoryProxy,
    fs: &mut ServiceFs<T>,
) -> Result<()> {
    // TODO(surajmalhotra): Do this wait every time we get a connection request to handle cases
    // where the instance goes away and comes back.
    let instance_dir_path = wait_for_first_instance(&svc).await?;
    let (instance_dir, request) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
    svc.as_ref_directory().open(
        &instance_dir_path,
        fio::Flags::PROTOCOL_DIRECTORY | fio::PERM_READABLE,
        request.into(),
    )?;

    fs.dir("svc").dir("output").add_remote("default", instance_dir);
    Ok(())
}

async fn filter_and_rename<T: ServiceObjTrait>(
    _svc: fio::DirectoryProxy,
    _fs: &mut ServiceFs<T>,
    _filter: &Vec<String>,
    _rename: &Vec<String>,
) -> Result<()> {
    bail!("filter_and_rename policy is not yet implemented");
    // Add a bunch of directories which forward requests?
}

fn get_value<'a>(dict: &'a fdata::Dictionary, key: &str) -> Option<&'a fdata::DictionaryValue> {
    match &dict.entries {
        Some(entries) => {
            for entry in entries {
                if entry.key == key {
                    return entry.value.as_ref().map(|val| &**val);
                }
            }
            None
        }
        _ => None,
    }
}

fn get_program_string<'a>(program: &'a fdata::Dictionary, key: &str) -> Result<&'a str> {
    if let Some(fdata::DictionaryValue::Str(value)) = get_value(program, key) {
        Ok(value)
    } else {
        Err(format_err!("{key} not found in program or is not a string"))
    }
}

fn get_program_strvec<'a>(
    program: &'a fdata::Dictionary,
    key: &str,
) -> Result<Option<&'a Vec<String>>> {
    match get_value(program, key) {
        Some(args_value) => match args_value {
            fdata::DictionaryValue::StrVec(vec) => Ok(Some(vec)),
            _ => Err(format_err!(
                "Expected {key} in program to be vector of strings, found something else"
            )),
        },
        None => Ok(None),
    }
}

pub async fn main(
    ns_entries: Vec<fprocess::NameInfo>,
    directory_request: ServerEnd<fio::DirectoryMarker>,
    lifecycle: ServerEnd<fpl::LifecycleMarker>,
    program: Option<fdata::Dictionary>,
) -> Result<()> {
    drop(lifecycle);
    if directory_request.as_handle_ref().is_invalid() {
        bail!("No valid handle found for outgoing directory");
    }
    let Some(svc) = ns_entries.into_iter().find(|e| e.path == "/svc") else {
        bail!("No /svc in namespace");
    };
    let Some(program) = program else {
        bail!("No program section provided");
    };
    let scope = fuchsia_async::Scope::new();
    let svc = svc.directory.into_proxy();
    let mut fs = ServiceFs::new();
    match get_program_string(&program, "policy")? {
        "first_instance_to_protocol" => {
            let protocol_name_str = get_program_string(&program, "protocol_name")?;

            let protocol_name = Name::new(protocol_name_str).map_err(|e| {
                format_err!("Invalid protocol_name '{protocol_name_str}' in program dict: {e}")
            })?;

            first_instance_to_protocol(svc, &mut fs, protocol_name, &scope).await
        }
        "first_instance_to_default" => first_instance_to_default(svc, &mut fs).await,
        "filter_and_rename" => {
            let empty = vec![];
            let filter = get_program_strvec(&program, "filter")?.unwrap_or(&empty);
            let rename = get_program_strvec(&program, "rename")?.unwrap_or(&empty);
            filter_and_rename(svc, &mut fs, filter, rename).await
        }
        policy => Err(format_err!("Unsupported policy specified: {policy}")),
    }?;

    log::debug!("[service-broker] Initialized.");

    fs.serve_connection(directory_request).context("failed to serve outgoing namespace")?;
    fs.collect::<()>().await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl::endpoints::{Proxy, create_endpoints, create_proxy};
    use fidl_fuchsia_data as fdata;
    use fuchsia_async as fasync;
    use futures::StreamExt;

    fn make_program_dict(entries: Vec<(&str, fdata::DictionaryValue)>) -> fdata::Dictionary {
        let entries = entries
            .into_iter()
            .map(|(k, v)| fdata::DictionaryEntry { key: k.to_string(), value: Some(Box::new(v)) })
            .collect();
        fdata::Dictionary { entries: Some(entries), ..Default::default() }
    }

    #[test]
    fn test_get_program_string() {
        let dict = make_program_dict(vec![(
            "policy",
            fdata::DictionaryValue::Str("first_instance_to_protocol".to_string()),
        )]);

        assert_eq!(get_program_string(&dict, "policy").unwrap(), "first_instance_to_protocol");

        let err = get_program_string(&dict, "missing_key").unwrap_err();
        assert_eq!(err.to_string(), "missing_key not found in program or is not a string");
    }

    #[test]
    fn test_get_program_strvec() {
        let dict = make_program_dict(vec![(
            "filter",
            fdata::DictionaryValue::StrVec(vec!["fuchsia.foo.Bar".to_string()]),
        )]);

        let vec = get_program_strvec(&dict, "filter").unwrap().unwrap();
        assert_eq!(vec.len(), 1);
        assert_eq!(vec[0], "fuchsia.foo.Bar");

        assert!(get_program_strvec(&dict, "rename").unwrap().is_none());
    }

    #[fuchsia::test]
    async fn test_filter_and_rename_graceful_failure() {
        let (dir_proxy, _server_end) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let mut fs = ServiceFs::<ServiceObj<'_, ()>>::new();

        let result = filter_and_rename(dir_proxy, &mut fs, &vec![], &vec![]).await;
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err().to_string(),
            "filter_and_rename policy is not yet implemented"
        );
    }

    #[fuchsia::test]
    async fn test_broker_caching_and_routing_end_to_end() {
        let (svc_dir, svc_server_end) = create_proxy::<fio::DirectoryMarker>();
        let mut fake_svc_fs = ServiceFs::new();

        fake_svc_fs.dir("input").dir("instance_123").add_service_at(
            "my_protocol",
            |req: zx::Channel| {
                let _ = req.write(&[1], &mut []);
                Some(())
            },
        );

        fasync::Task::spawn(async move {
            fake_svc_fs.serve_connection(svc_server_end).unwrap();
            fake_svc_fs.collect::<()>().await;
        })
        .detach();

        let ns_entries = vec![fprocess::NameInfo {
            path: "/svc".to_string(),
            directory: svc_dir.into_channel().unwrap().into_zx_channel().into(),
        }];

        let (out_dir, out_server_end) = create_proxy::<fio::DirectoryMarker>();
        let (_, lifecycle_server_end) = create_endpoints::<fpl::LifecycleMarker>();

        let program_dict = make_program_dict(vec![
            ("policy", fdata::DictionaryValue::Str("first_instance_to_protocol".to_string())),
            ("protocol_name", fdata::DictionaryValue::Str("my_protocol".to_string())),
        ]);

        fasync::Task::spawn(async move {
            let res =
                main(ns_entries, out_server_end, lifecycle_server_end, Some(program_dict)).await;
            assert!(res.is_ok(), "Broker main task failed");
        })
        .detach();

        let (client_end, server_end) = zx::Channel::create();

        out_dir
            .open("svc/output", fio::Flags::PROTOCOL_SERVICE, &fio::Options::default(), server_end)
            .expect("Failed to send open request to broker");

        let signals = fasync::OnSignals::new(&client_end, zx::Signals::CHANNEL_READABLE)
            .await
            .expect("Failed waiting for signal. Routing may have dropped the channel.");

        assert!(signals.contains(zx::Signals::CHANNEL_READABLE));

        let (client_end2, server_end2) = zx::Channel::create();
        out_dir
            .open("svc/output", fio::Flags::PROTOCOL_SERVICE, &fio::Options::default(), server_end2)
            .unwrap();

        let _ = fasync::OnSignals::new(&client_end2, zx::Signals::CHANNEL_READABLE).await.unwrap();
    }
}
