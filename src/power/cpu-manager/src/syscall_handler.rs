// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::CpuManagerError;
use crate::message::{Message, MessageResult, MessageReturn};
use crate::node::Node;
use anyhow::{Error, format_err};
use async_trait::async_trait;
use fidl_fuchsia_kernel as fkernel;
use fuchsia_inspect::{self as inspect, NumericProperty as _, Property as _};
use serde_derive::Deserialize;
use serde_json as json;
use std::rc::Rc;
use zx::sys;

/// Node: SyscallHandler
///
/// Summary: Executes syscalls so that these calls can be easily mocked by tests for dependent
///          nodes.
///
/// Handles Messages:
///     - GetNumCpus
///     - SetCpuPerformanceInfo
///
/// FIDL dependencies: None

#[derive(Default)]
pub struct SyscallHandlerBuilder<'a> {
    /// A fake CPU resource for injection into SyscallHandler.
    cpu_resource: Option<zx::Resource>,

    /// A fake Inspect root for injection into Syscall Handler.
    inspect_root: Option<&'a inspect::Node>,

    /// Flag that determines whether a fake CPU resource is used for integration
    /// tests.
    use_fake_cpu_resource: bool,
}

impl<'a> SyscallHandlerBuilder<'a> {
    pub fn new_from_json(json_data: json::Value) -> Self {
        #[derive(Default, Deserialize)]
        struct Config {
            use_fake_cpu_resource: Option<bool>,
        }

        #[derive(Deserialize)]
        struct JsonData {
            config: Option<Config>,
        }

        let data: JsonData = json::from_value(json_data).unwrap();
        Self {
            use_fake_cpu_resource: data
                .config
                .unwrap_or_default()
                .use_fake_cpu_resource
                .unwrap_or_default(),
            ..Default::default()
        }
    }

    #[cfg(test)]
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    fn with_cpu_resource(mut self, resource: zx::Resource) -> Self {
        self.cpu_resource = Some(resource);
        self
    }

    #[cfg(test)]
    fn with_inspect_root(mut self, root: &'a inspect::Node) -> Self {
        self.inspect_root = Some(root);
        self
    }

    pub async fn build(self) -> Result<Rc<SyscallHandler>, Error> {
        // If a CPU resource was not provided, query component_manager for it.
        let cpu_resource = match self.cpu_resource {
            Some(resource) => resource,
            None => {
                if self.use_fake_cpu_resource {
                    zx::Resource::from(zx::NullableHandle::invalid())
                } else {
                    let proxy = fuchsia_component::client::connect_to_protocol::<
                        fkernel::CpuResourceMarker,
                    >()?;
                    proxy.get().await?
                }
            }
        };

        // Optionally use the default inspect root node
        let inspect_root =
            self.inspect_root.unwrap_or_else(|| inspect::component::inspector().root());

        let inspect_data = InspectData::new(inspect_root, "SyscallHandler");

        Ok(Rc::new(SyscallHandler { cpu_resource, inspect: inspect_data }))
    }
}

/// Struct for handling syscalls.
#[derive(Debug)]
pub struct SyscallHandler {
    cpu_resource: zx::Resource,
    inspect: InspectData,
}

impl SyscallHandler {
    fn handle_get_num_cpus(&self) -> MessageResult {
        // There are no assumptions made by this unsafe block; it is only unsafe due to FFI.
        let num_cpus = unsafe { sys::zx_system_get_num_cpus() };

        Ok(MessageReturn::GetNumCpus(num_cpus))
    }

    fn handle_set_cpu_performance_info(
        &self,
        new_info: &Vec<sys::zx_cpu_performance_info_t>,
    ) -> MessageResult {
        // The unsafe block assumes only that the pointer to `new_info` agrees with the count
        // argument that follows.
        let status = unsafe {
            sys::zx_system_set_performance_info(
                self.cpu_resource.raw_handle(),
                sys::ZX_CPU_PERF_SCALE,
                new_info.as_ptr() as *const u8,
                new_info.len(),
            )
        };

        if let Err(e) = zx::Status::ok(status) {
            self.inspect.set_performance_info_error_count.add(1);
            let error_string = e.to_string();
            self.inspect.set_performance_info_last_error.set(&error_string);
            let verbose_string = format!(
                "{}: zx_system_set_performance_info returned error: {}",
                self.name(),
                error_string
            );
            log::error!("{}", verbose_string);
            return Err(format_err!("{}", verbose_string).into());
        }

        Ok(MessageReturn::SetCpuPerformanceInfo)
    }
}

#[async_trait(?Send)]
impl Node for SyscallHandler {
    fn name(&self) -> String {
        "SyscallHandler".to_string()
    }

    async fn handle_message(&self, msg: &Message) -> MessageResult {
        match msg {
            Message::GetNumCpus => self.handle_get_num_cpus(),
            Message::SetCpuPerformanceInfo(new_info) => {
                self.handle_set_cpu_performance_info(new_info)
            }

            _ => Err(CpuManagerError::Unsupported),
        }
    }
}

#[derive(Debug)]
struct InspectData {
    _root_node: inspect::Node,

    // Properties
    set_performance_info_error_count: inspect::UintProperty,
    set_performance_info_last_error: inspect::StringProperty,
}

impl InspectData {
    fn new(parent: &inspect::Node, node_name: &str) -> Self {
        let root_node = parent.create_child(node_name);

        let set_performance_info = root_node.create_child("zx_system_set_performance_info");
        let set_performance_info_error_count = set_performance_info.create_uint("error_count", 0);
        let set_performance_info_last_error =
            set_performance_info.create_string("last_error", "<None>");
        root_node.record(set_performance_info);

        Self {
            _root_node: root_node,
            set_performance_info_error_count,
            set_performance_info_last_error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use diagnostics_assertions::assert_data_tree;

    /// Tests that well-formed configuration JSON does not panic the `new_from_json` function.
    #[fuchsia::test]
    async fn test_new_from_json() {
        let json_data = json::json!({
            "type": "SyscallHandler",
            "name": "syscall_handler"
        });
        let builder = SyscallHandlerBuilder::new_from_json(json_data);

        // Expect PEER_CLOSED due to no CpuResource connection.
        builder.build().await.unwrap_err();
    }

    #[fuchsia::test]
    async fn test_new_from_json_with_empty_config() {
        let json_data = json::json!({
            "type": "SyscallHandler",
            "name": "syscall_handler",
            "config": {}
        });
        let builder = SyscallHandlerBuilder::new_from_json(json_data);

        // Expect PEER_CLOSED due to no CpuResource connection.
        builder.build().await.unwrap_err();
    }

    #[fuchsia::test]
    async fn test_new_from_json_with_use_fake_cpu_resource_returns_invalid_handle() {
        let json_data = json::json!({
            "type": "SyscallHandler",
            "name": "syscall_handler",
            "config": {
                "use_fake_cpu_resource": true
            }
        });
        let builder = SyscallHandlerBuilder::new_from_json(json_data);
        let handler = builder.build().await.unwrap();
        assert_eq!(
            zx::NullableHandle::invalid().as_handle_ref(),
            handler.cpu_resource.as_handle_ref()
        );
    }

    // Tests that errors are logged to Inspect as expected.
    #[fuchsia::test]
    async fn test_inspect_data() {
        let resource = zx::NullableHandle::invalid().into();
        let inspector = inspect::Inspector::default();
        let builder = SyscallHandlerBuilder::new()
            .with_cpu_resource(resource)
            .with_inspect_root(inspector.root());
        let handler = builder.build().await.unwrap();

        // The test links against a fake implementation of zx_system_set_performance_info that is
        // hard-coded to return ZX_ERR_BAD_HANDLE.
        let set_performance_info_status_string = zx::Status::BAD_HANDLE.to_string();

        match handler.handle_message(&Message::SetCpuPerformanceInfo(Vec::new())).await {
            Ok(_) => panic!("Expected to receive an error"),
            Err(CpuManagerError::GenericError(e)) => {
                assert!(e.to_string().contains(&set_performance_info_status_string));
            }
            Err(e) => panic!("Expected GenericError; received {:?}", e),
        }

        assert_data_tree!(
            inspector,
            root: {
                SyscallHandler: {
                    zx_system_set_performance_info: {
                        error_count: 1u64,
                        last_error: set_performance_info_status_string.clone(),
                    },
                }
            }
        );
    }
}
