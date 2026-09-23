// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fidl_fuchsia_data as fdata;
use fidl_fuchsia_io as fio;
use fuchsia_component::directory::open_file_async;
use zx::Status;

pub(crate) fn basename(path: &str) -> &str {
    match path.rsplit_once('/') {
        Some((_, filename)) => filename,
        None => path,
    }
}

pub(crate) async fn get_file_vmo(
    incoming: &impl fuchsia_component::directory::AsRefDirectory,
    relative_path: &str,
) -> Result<zx::Vmo, Status> {
    let driver_library =
        open_file_async(incoming, &["/pkg/", relative_path].concat(), fio::RX_STAR_DIR)
            .map_err(|_| Status::INVALID_ARGS)?;
    driver_library
        .get_backing_memory(
            fio::VmoFlags::READ | fio::VmoFlags::EXECUTE | fio::VmoFlags::PRIVATE_CLONE,
        )
        .await
        .map_err(|_| Status::PEER_CLOSED)?
        .map_err(Status::err_from_raw)
}

#[derive(Debug, PartialEq)]
pub(crate) enum DispatcherOpts {
    Default = 0,
    AllowSyncCalls = 1,
}

impl std::convert::From<&fdata::Dictionary> for DispatcherOpts {
    fn from(program: &fdata::Dictionary) -> Self {
        let mut dispatcher_opts = DispatcherOpts::Default;
        if let Ok(Some(opts)) = get_program_strvec(program, "default_dispatcher_opts") {
            for opt in opts {
                match opt.as_str() {
                    "allow_sync_calls" => dispatcher_opts = DispatcherOpts::AllowSyncCalls,
                    _ => log::warn!("Ignoring unknown default_dispatcher_opt: {opt}"),
                }
            }
        }
        dispatcher_opts
    }
}

fn get_value<'a>(dict: &'a fdata::Dictionary, key: &str) -> Option<&'a fdata::DictionaryValue> {
    match &dict.entries {
        Some(entries) => {
            for entry in entries {
                if entry.key == key {
                    return entry.value.as_deref();
                }
            }
            None
        }
        _ => None,
    }
}

pub(crate) fn get_program_string<'a>(
    program: &'a fdata::Dictionary,
    key: &str,
) -> Result<&'a str, Status> {
    match get_value(program, key) {
        Some(fdata::DictionaryValue::Str(value)) => Ok(value),
        Some(_) => {
            log::error!("Expected {key} to be a string, found something else");
            Err(Status::WRONG_TYPE)
        }
        None => Err(Status::NOT_FOUND),
    }
}

pub(crate) fn get_program_strvec<'a>(
    program: &'a fdata::Dictionary,
    key: &str,
) -> Result<Option<&'a Vec<String>>, Status> {
    match get_value(program, key) {
        Some(args_value) => match args_value {
            fdata::DictionaryValue::StrVec(vec) => Ok(Some(vec)),
            _ => {
                log::error!("Expected {key} to be vector of strings, found something else");
                Err(Status::WRONG_TYPE)
            }
        },
        None => Ok(None),
    }
}

pub(crate) fn get_program_objvec<'a>(
    program: &'a fdata::Dictionary,
    key: &str,
) -> Result<Option<&'a Vec<fdata::Dictionary>>, Status> {
    match get_value(program, key) {
        Some(args_value) => match args_value {
            fdata::DictionaryValue::ObjVec(vec) => Ok(Some(vec)),
            _ => {
                log::error!("Expected {key} to be vector of strings, found something else");
                Err(Status::WRONG_TYPE)
            }
        },
        None => Ok(None),
    }
}

pub(crate) fn update_process_name(host_name: &str, driver_url: &str, driver_count: usize) {
    let name = if !host_name.is_empty() {
        host_name.to_string()
    } else if driver_count > 0 {
        let Ok(current_name) = fuchsia_runtime::process_self().get_name() else {
            return;
        };
        let current_name = current_name.to_string();
        let driver_name = match current_name.split_once(' ') {
            Some((driver_name, _)) => driver_name,
            None => &current_name,
        };

        format!("{driver_name} (+{} more)", driver_count - 1)
    } else {
        basename(driver_url).to_string()
    };
    let name = zx::Name::new_lossy(&name);
    let _ = fuchsia_runtime::process_self().set_name(&name);
    let _ = zx::Thread::raise_user_exception(
        zx::RaiseExceptionOptions::TARGET_JOB_DEBUGGER,
        zx::sys::ZX_EXCP_USER_CODE_PROCESS_NAME_CHANGED,
        0,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::LazyLock;

    #[fuchsia::test]
    async fn basename_gets_file() {
        assert_eq!(basename("foo/bar"), "bar");
        assert_eq!(basename("foo"), "foo");
        assert_eq!(basename(""), "");
    }

    static PROGRAM: LazyLock<fdata::Dictionary> = LazyLock::new(|| fdata::Dictionary {
        entries: Some(vec![
            fdata::DictionaryEntry {
                key: "str".to_string(),
                value: Some(Box::new(fdata::DictionaryValue::Str("value".to_string()))),
            },
            fdata::DictionaryEntry {
                key: "strvec".to_string(),
                value: Some(Box::new(fdata::DictionaryValue::StrVec(vec![
                    "value1".to_string(),
                    "value2".to_string(),
                ]))),
            },
            fdata::DictionaryEntry {
                key: "objvec".to_string(),
                value: Some(Box::new(fdata::DictionaryValue::ObjVec(vec![
                    fdata::Dictionary::default(),
                    fdata::Dictionary::default(),
                ]))),
            },
        ]),
        ..Default::default()
    });

    #[fuchsia::test]
    async fn get_program_string_test() {
        assert_eq!(get_program_string(&PROGRAM, "str"), Ok("value"));
        assert_eq!(get_program_string(&PROGRAM, "foo"), Err(Status::NOT_FOUND));
        assert_eq!(get_program_string(&PROGRAM, "strvec"), Err(Status::WRONG_TYPE));
    }

    #[fuchsia::test]
    async fn get_program_strvec_test() {
        assert_eq!(get_program_strvec(&PROGRAM, "foo"), Ok(None));
        assert_eq!(get_program_strvec(&PROGRAM, "objvec"), Err(Status::WRONG_TYPE));
    }

    #[fuchsia::test]
    async fn get_program_objvec_test() {
        assert_eq!(
            get_program_objvec(&PROGRAM, "objvec"),
            Ok(Some(&vec![fdata::Dictionary::default(), fdata::Dictionary::default()])),
        );
        assert_eq!(get_program_objvec(&PROGRAM, "foo"), Ok(None));
        assert_eq!(get_program_objvec(&PROGRAM, "strvec"), Err(Status::WRONG_TYPE));
    }

    #[fuchsia::test]
    async fn parse_dispatcher_opts() {
        let program = fdata::Dictionary {
            entries: Some(vec![fdata::DictionaryEntry {
                key: "default_dispatcher_opts".to_string(),
                value: Some(Box::new(fdata::DictionaryValue::StrVec(vec![
                    "allow_sync_calls".to_string(),
                ]))),
            }]),
            ..Default::default()
        };
        assert_eq!(DispatcherOpts::from(LazyLock::force(&PROGRAM)), DispatcherOpts::Default);
        assert_eq!(DispatcherOpts::from(&program), DispatcherOpts::AllowSyncCalls);
    }
}
