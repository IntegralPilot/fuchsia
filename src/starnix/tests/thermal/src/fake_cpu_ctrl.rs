// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use fidl_fuchsia_hardware_cpu_ctrl::{
    CpuOperatingPointInfo, DeviceRequest, DeviceRequestStream, ServiceRequest,
};
use fuchsia_component::server::ServiceFs;
use fuchsia_component_test::LocalComponentHandles;
use futures::prelude::*;

pub async fn mock_cpu_ctrl_service(handles: LocalComponentHandles) -> Result<(), Error> {
    let mut fs = ServiceFs::new();
    fs.dir("svc")
        .add_fidl_service_instance_at(
            "fuchsia.hardware.cpu.ctrl.Service",
            "domain0",
            |request: ServiceRequest| {
                let ServiceRequest::Device(stream) = request;
                (0u32, stream)
            },
        )
        .add_fidl_service_instance_at(
            "fuchsia.hardware.cpu.ctrl.Service",
            "domain1",
            |request: ServiceRequest| {
                let ServiceRequest::Device(stream) = request;
                (1u32, stream)
            },
        );
    fs.serve_connection(handles.outgoing_dir)?;

    fs.for_each_concurrent(None, |(domain_id, stream): (u32, DeviceRequestStream)| async move {
        stream
            .try_for_each(|request: DeviceRequest| async move {
                match request {
                    DeviceRequest::GetDomainId { responder } => {
                        responder.send(domain_id)?;
                    }
                    DeviceRequest::GetNumLogicalCores { responder } => {
                        let count = if domain_id == 0 { 2 } else { 4 };
                        responder.send(count)?;
                    }
                    DeviceRequest::GetLogicalCoreId { index, responder } => {
                        let id = if domain_id == 0 { index } else { 2 + index };
                        responder.send(id)?;
                    }
                    DeviceRequest::GetOperatingPointCount { responder } => {
                        let count = if domain_id == 0 { 4 } else { 2 };
                        responder.send(Ok(count))?;
                    }
                    DeviceRequest::GetCurrentOperatingPoint { responder } => {
                        responder.send(0)?;
                    }
                    DeviceRequest::GetOperatingPointInfo { opp, responder } => {
                        let freqs: &[i64] = if domain_id == 0 {
                            &[2024000000, 1512000000, 1256000000, 1128000000]
                        } else {
                            &[1024000000, 512000000]
                        };
                        if let Some(&frequency_hz) = freqs.get(opp as usize) {
                            let info = CpuOperatingPointInfo { frequency_hz, voltage_uv: 0 };
                            responder.send(Ok(&info))?;
                        } else {
                            responder.send(Err(zx::sys::ZX_ERR_OUT_OF_RANGE))?;
                        }
                    }
                    _ => {}
                }
                Ok(())
            })
            .await
            .unwrap();
    })
    .await;

    Ok(())
}
