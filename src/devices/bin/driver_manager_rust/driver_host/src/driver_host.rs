// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::runtime_dir::{CachedProcessInfo, create_runtime_dir};
use async_trait::async_trait;
use driver_manager_types::to_deprecated_property;
use driver_manager_utils::{open_lib_dir, open_pkg_file};
use fidl::endpoints::ClientEnd;
use fidl_fuchsia_component_runner as frunner;
use fidl_fuchsia_data as fdata;
use fidl_fuchsia_driver_framework as fdf;
use fidl_fuchsia_driver_host as fdh;
use fidl_fuchsia_driver_loader as floader;
use fidl_fuchsia_io as fio;
use fidl_fuchsia_ldsvc as fldsvc;
use fidl_fuchsia_mem as fmem;
use log::error;
use std::sync::Arc;
use vfs::directory::simple::Simple;
use vfs::execution_scope::ExecutionScope;

pub struct DriverStartArgs {
    pub node: ClientEnd<fdf::NodeMarker>,
    pub node_name: String,
    pub properties: fdf::NodePropertyDictionary2,
    pub symbols: Option<Vec<fdf::NodeSymbol>>,
    pub offers: Vec<fdf::Offer>,
    pub start_info: frunner::ComponentStartInfo,
    pub component_instance: zx::Event,
}

pub struct DriverLoadArgs {
    pub driver_soname: String,
    pub driver_file: zx::Vmo,
    pub lib_dir: ClientEnd<fio::DirectoryMarker>,
    pub additional_root_modules: Vec<floader::RootModule>,
}

fn get_program_string_value<'a>(
    program: &'a fdata::Dictionary,
    key: &str,
) -> Result<&'a str, zx::Status> {
    program
        .entries
        .as_ref()
        .and_then(|entries| {
            entries.iter().find(|entry| entry.key == key).and_then(|entry| {
                entry.value.as_ref().and_then(|value| match &**value {
                    fdata::DictionaryValue::Str(s) => Some(s.as_str()),
                    _ => None,
                })
            })
        })
        .ok_or(zx::Status::NOT_FOUND)
}

fn get_program_value_as_obj_vector<'a>(
    program: &'a fdata::Dictionary,
    key: &str,
) -> Result<Vec<&'a fdata::Dictionary>, zx::Status> {
    let entries = program.entries.as_ref().ok_or(zx::Status::NOT_FOUND)?;
    let entry = entries.iter().find(|e| e.key == key).ok_or(zx::Status::NOT_FOUND)?;
    let value = entry.value.as_ref().ok_or(zx::Status::NOT_FOUND)?;
    if let fdata::DictionaryValue::ObjVec(v) = &**value {
        Ok(v.iter().collect())
    } else {
        Err(zx::Status::WRONG_TYPE)
    }
}

fn get_filename(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, filename)| filename)
}

const COMPAT_DRIVER_RELATIVE_PATH: &str = "driver/compat.so";

impl DriverLoadArgs {
    pub async fn new(start_info: &mut frunner::ComponentStartInfo) -> Result<Self, zx::Status> {
        let program = start_info.program.as_ref().ok_or(zx::Status::INVALID_ARGS)?;
        let binary = get_program_string_value(program, "binary")?;

        let ns = start_info.ns.as_mut().ok_or(zx::Status::INVALID_ARGS)?;
        let pkg_dir_handle = ns
            .iter_mut()
            .find(|entry| entry.path.as_deref() == Some("/pkg"))
            .and_then(|entry| entry.directory.take())
            .ok_or(zx::Status::INVALID_ARGS)?;
        let pkg_dir = pkg_dir_handle.into_proxy();

        let driver_file = open_pkg_file(&pkg_dir, binary).await.map_err(|e| {
            error!("Failed to open pkg file '{}': {}", binary, e);
            zx::Status::INTERNAL
        })?;

        let lib_dir = open_lib_dir(&pkg_dir).map_err(|e| {
            error!("Failed to open lib dir: {}", e);
            zx::Status::INTERNAL
        })?;
        let mut additional_root_modules = vec![];
        if binary == COMPAT_DRIVER_RELATIVE_PATH {
            let compat = get_program_string_value(program, "compat")?;
            let v1_driver_file = open_pkg_file(&pkg_dir, compat).await.map_err(|e| {
                error!("Failed to open pkg file '{}': {}", compat, e);
                zx::Status::INTERNAL
            })?;
            additional_root_modules.push(floader::RootModule {
                name: Some(get_filename(compat).to_string()),
                binary: Some(v1_driver_file),
                ..Default::default()
            });
        }

        if let Ok(modules) = get_program_value_as_obj_vector(program, "modules") {
            for module in modules {
                let module_name = get_program_string_value(module, "module_name")?;
                if module_name == "#program.compat" {
                    continue;
                }
                let module_vmo = open_pkg_file(&pkg_dir, module_name).await.map_err(|e| {
                    error!("Failed to open pkg file '{}': {}", module_name, e);
                    zx::Status::INTERNAL
                })?;
                additional_root_modules.push(floader::RootModule {
                    name: Some(get_filename(module_name).to_string()),
                    binary: Some(module_vmo),
                    ..Default::default()
                });
            }
        }

        Ok(Self {
            driver_soname: get_filename(binary).to_string(),
            driver_file,
            lib_dir,
            additional_root_modules,
        })
    }
}

fn set_encoded_config(
    start_info: &mut frunner::ComponentStartInfo,
) -> Result<Option<zx::Vmo>, zx::Status> {
    if let Some(encoded_config) = start_info.encoded_config.take() {
        match encoded_config {
            fmem::Data::Buffer(fmem::Buffer { vmo, .. }) => Ok(Some(vmo)),
            fmem::Data::Bytes(bytes) => {
                let vmo = zx::Vmo::create(bytes.len() as u64)?;
                vmo.write(&bytes, 0)?;
                Ok(Some(vmo))
            }
            _ => {
                error!("Unsupported encoded config format");
                Err(zx::Status::INVALID_ARGS)
            }
        }
    } else {
        Ok(None)
    }
}

#[async_trait]
pub trait DriverHost {
    async fn start(
        &self,
        start_args: DriverStartArgs,
        driver: fidl::endpoints::ServerEnd<fdh::DriverMarker>,
    ) -> Result<(), zx::Status>;

    async fn start_with_dynamic_linker(
        &self,
        load_args: DriverLoadArgs,
        start_args: DriverStartArgs,
        driver: fidl::endpoints::ServerEnd<fdh::DriverMarker>,
    ) -> Result<(), zx::Status>;

    fn install_loader(
        &self,
        loader: fidl::endpoints::ClientEnd<fldsvc::LoaderMarker>,
    ) -> Result<(), zx::Status>;

    fn is_dynamic_linking_enabled(&self) -> bool;

    async fn get_process_koid(&self) -> Result<zx::Koid, zx::Status>;

    async fn get_process_info_internal(
        &self,
    ) -> Result<crate::runtime_dir::ProcessInfo, zx::Status>;

    async fn get_crash_info(
        &self,
        thread_koid: zx::Koid,
    ) -> Result<fdh::DriverCrashInfo, zx::Status>;

    fn trigger_stack_trace(&self);

    fn name_for_colocation(&self) -> &str;
}

pub struct DriverHostComponent {
    driver_host: fdh::DriverHostProxy,
    dynamic_linker_driver_loader: Option<floader::DriverHostProxy>,
    scope: ExecutionScope,
    process_info: Arc<CachedProcessInfo>,
    runtime_dir: Arc<Simple>,
    name_for_colocation: String,
}

impl DriverHostComponent {
    pub fn new(
        driver_host: fdh::DriverHostProxy,
        dynamic_linker_driver_loader: Option<floader::DriverHostProxy>,
        scope: ExecutionScope,
        name_for_colocation: String,
    ) -> Self {
        let process_info = Arc::new(CachedProcessInfo::new(driver_host.clone()));
        let runtime_dir = create_runtime_dir(process_info.clone());
        Self {
            driver_host,
            dynamic_linker_driver_loader,
            scope,
            process_info,
            runtime_dir,
            name_for_colocation,
        }
    }
}

#[async_trait]
impl DriverHost for DriverHostComponent {
    async fn start(
        &self,
        start_args: DriverStartArgs,
        driver: fidl::endpoints::ServerEnd<fdh::DriverMarker>,
    ) -> Result<(), zx::Status> {
        let mut start_info = start_args.start_info;
        let config = set_encoded_config(&mut start_info)?;

        let node_properties_2 = Some(start_args.properties);
        let node_properties = node_properties_2.as_ref().map(|props2| {
            props2
                .iter()
                .map(|entry2| fdf::NodePropertyEntry {
                    name: entry2.name.clone(),
                    properties: entry2.properties.iter().map(to_deprecated_property).collect(),
                })
                .collect::<Vec<_>>()
        });

        let fidl_start_args = fdf::DriverStartArgs {
            node: Some(start_args.node),
            node_name: Some(start_args.node_name),
            symbols: start_args.symbols,
            node_offers: Some(start_args.offers),
            node_properties,
            node_properties_2,
            node_token: Some(start_args.component_instance),
            url: start_info.resolved_url.take(),
            program: start_info.program.take(),
            incoming: start_info.ns.take(),
            outgoing_dir: start_info.outgoing_dir.take(),
            config,
            ..Default::default()
        };

        if let Some(runtime_dir) = start_info.runtime_dir.take() {
            vfs::directory::serve_on(
                self.runtime_dir.clone(),
                fio::PERM_READABLE,
                self.scope.clone(),
                runtime_dir,
            );
        }
        let host_name = if !self.name_for_colocation().is_empty() {
            format!("driver-host-{}", self.name_for_colocation())
        } else {
            String::new()
        };

        self.driver_host
            .start(fidl_start_args, driver, &host_name)
            .await
            .map_err(|e| {
                error!("Failed to start driver in driver host: {}", e);
                zx::Status::INTERNAL
            })?
            .map_err(zx::Status::err_from_raw)
    }

    async fn start_with_dynamic_linker(
        &self,
        load_args: DriverLoadArgs,
        start_args: DriverStartArgs,
        driver: fidl::endpoints::ServerEnd<fdh::DriverMarker>,
    ) -> Result<(), zx::Status> {
        let loader = self.dynamic_linker_driver_loader.as_ref().ok_or(zx::Status::NOT_SUPPORTED)?;

        let driver_soname = load_args.driver_soname.clone();
        let request = floader::DriverHostLoadDriverRequest {
            driver_soname: Some(driver_soname.clone()),
            driver_binary: Some(load_args.driver_file),
            driver_libs: Some(load_args.lib_dir),
            additional_root_modules: Some(load_args.additional_root_modules),
            ..Default::default()
        };

        loader
            .load_driver(request)
            .await
            .map_err(|e| {
                error!("Failed to load driver '{}' with dynamic linker: {}", driver_soname, e);
                if e.is_closed() { zx::Status::PEER_CLOSED } else { zx::Status::INTERNAL }
            })?
            .map_err(zx::Status::err_from_raw)?;

        self.start(start_args, driver).await
    }

    fn install_loader(
        &self,
        loader: fidl::endpoints::ClientEnd<fldsvc::LoaderMarker>,
    ) -> Result<(), zx::Status> {
        self.driver_host
            .install_loader(loader)
            .map_err(|e| if e.is_closed() { zx::Status::PEER_CLOSED } else { zx::Status::INTERNAL })
    }

    fn is_dynamic_linking_enabled(&self) -> bool {
        self.dynamic_linker_driver_loader.is_some()
    }

    async fn get_process_koid(&self) -> Result<zx::Koid, zx::Status> {
        self.process_info.get().await.map(|info| info.process_koid)
    }

    async fn get_process_info_internal(
        &self,
    ) -> Result<crate::runtime_dir::ProcessInfo, zx::Status> {
        self.process_info.get().await.cloned()
    }

    async fn get_crash_info(
        &self,
        thread_koid: zx::Koid,
    ) -> Result<fdh::DriverCrashInfo, zx::Status> {
        // Bypass the driver host if the crashing thread is the main thread which means the driver host
        // itself is what crashed.
        if let Ok(info) = self.process_info.get().await
            && info.main_thread_koid == thread_koid
        {
            return Err(zx::Status::NOT_FOUND);
        }

        self.driver_host
            .find_driver_crash_info_by_thread_koid(thread_koid.raw_koid())
            .await
            .map_err(|e| {
                error!("Failed to get crash info from driver host: {}", e);
                zx::Status::INTERNAL
            })?
            .map_err(zx::Status::err_from_raw)
    }

    fn name_for_colocation(&self) -> &str {
        &self.name_for_colocation
    }

    fn trigger_stack_trace(&self) {
        let _ = self.driver_host.trigger_stack_trace();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_filename() {
        assert_eq!(get_filename("foo/bar/baz.so"), "baz.so");
        assert_eq!(get_filename("baz.so"), "baz.so");
        assert_eq!(get_filename("/baz.so"), "baz.so");
        assert_eq!(get_filename(""), "");
    }

    #[test]
    fn test_get_program_string_value() {
        let program = fdata::Dictionary {
            entries: Some(vec![
                fdata::DictionaryEntry {
                    key: "binary".to_string(),
                    value: Some(Box::new(fdata::DictionaryValue::Str("driver.so".to_string()))),
                },
                fdata::DictionaryEntry {
                    key: "other".to_string(),
                    value: Some(Box::new(fdata::DictionaryValue::Str("value".to_string()))),
                },
            ]),
            ..Default::default()
        };

        assert_eq!(get_program_string_value(&program, "binary"), Ok("driver.so"));
        assert_eq!(get_program_string_value(&program, "other"), Ok("value"));
        assert_eq!(get_program_string_value(&program, "missing"), Err(zx::Status::NOT_FOUND));
    }

    #[test]
    fn test_get_program_value_as_obj_vector() {
        let dict1 = fdata::Dictionary {
            entries: Some(vec![fdata::DictionaryEntry {
                key: "k1".to_string(),
                value: Some(Box::new(fdata::DictionaryValue::Str("v1".to_string()))),
            }]),
            ..Default::default()
        };
        let dict2 = fdata::Dictionary {
            entries: Some(vec![fdata::DictionaryEntry {
                key: "k2".to_string(),
                value: Some(Box::new(fdata::DictionaryValue::Str("v2".to_string()))),
            }]),
            ..Default::default()
        };

        let program = fdata::Dictionary {
            entries: Some(vec![fdata::DictionaryEntry {
                key: "modules".to_string(),
                value: Some(Box::new(fdata::DictionaryValue::ObjVec(vec![
                    dict1.clone(),
                    dict2.clone(),
                ]))),
            }]),
            ..Default::default()
        };

        let modules = get_program_value_as_obj_vector(&program, "modules").unwrap();
        assert_eq!(modules.len(), 2);
        assert_eq!(get_program_string_value(modules[0], "k1"), Ok("v1"));
        assert_eq!(get_program_string_value(modules[1], "k2"), Ok("v2"));
    }

    #[test]
    fn test_set_encoded_config_buffer() {
        let vmo = zx::Vmo::create(100).unwrap();
        vmo.write(b"test", 0).unwrap();
        let mut start_info = frunner::ComponentStartInfo {
            encoded_config: Some(fmem::Data::Buffer(fmem::Buffer {
                vmo: vmo.duplicate_handle(zx::Rights::SAME_RIGHTS).unwrap(),
                size: 4,
            })),
            ..Default::default()
        };

        let config_vmo = set_encoded_config(&mut start_info).unwrap().unwrap();
        let mut buf = [0u8; 4];
        config_vmo.read(&mut buf, 0).unwrap();
        assert_eq!(&buf, b"test");
    }

    #[test]
    fn test_set_encoded_config_bytes() {
        let mut start_info = frunner::ComponentStartInfo {
            encoded_config: Some(fmem::Data::Bytes(b"test_bytes".to_vec())),
            ..Default::default()
        };

        let config_vmo = set_encoded_config(&mut start_info).unwrap().unwrap();
        let mut buf = [0u8; 10];
        config_vmo.read(&mut buf, 0).unwrap();
        assert_eq!(&buf, b"test_bytes");
    }
}
