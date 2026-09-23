// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::traits;
use crate::traits::{Device, LogError, NotificationHandler};

use crate::magma_common_defs::{
    MAGMA_QUERY_MAXIMUM_INFLIGHT_PARAMS, MAX_INFLIGHT_MEMORY_MB, MAX_INFLIGHT_MESSAGES,
};
use crate::magma_system_connection::{
    ConnectionMessage, MagmaNotificationHandler, Owner as ConnectionOwner,
};
use fidl::endpoints::ServerEnd;
use futures::channel::mpsc::UnboundedSender;
use std::collections::HashMap;

use crate::magma_system_connection::MagmaStatus;
use anyhow::Context;
use fuchsia_async;
use futures::TryStreamExt;
use std::sync::{Arc, Mutex};
use zx;

struct Connection {
    sender: UnboundedSender<ConnectionMessage>,
    thread: Option<std::thread::JoinHandle<()>>,
}
pub struct MagmaSystemDevice {
    driver: Box<dyn traits::Driver>,
    msd_device: Box<dyn traits::Device>,
    perf_count_access_token_id: u64,
    connections: Mutex<HashMap<u64, Connection>>,
}

impl Drop for MagmaSystemDevice {
    fn drop(&mut self) {
        log::debug!("Dropping MagmaSystemDevice");
    }
}

impl ConnectionOwner for MagmaSystemDevice {
    fn driver(&self) -> &dyn traits::Driver {
        &*self.driver
    }

    fn perf_count_access_token_id(&self) -> u64 {
        self.perf_count_access_token_id
    }
}

impl MagmaSystemDevice {
    pub fn new(
        driver: Box<dyn traits::Driver>,
        msd_device: Box<dyn traits::Device>,
        perf_count_access_token_id: u64,
    ) -> Self {
        MagmaSystemDevice {
            driver,
            msd_device,
            perf_count_access_token_id,
            connections: Mutex::new(HashMap::new()),
        }
    }

    pub fn query(&self, id: u64) -> Result<(Option<zx::Vmo>, u64), MagmaStatus> {
        match id {
            MAGMA_QUERY_MAXIMUM_INFLIGHT_PARAMS => {
                let result = (MAX_INFLIGHT_MESSAGES << 32) | MAX_INFLIGHT_MEMORY_MB;
                Ok((None, result))
            }
            _ => self.msd_device.query(id),
        }
    }

    /// This stops all of the connection threads and waits for them to complete.
    pub fn stop_all_connections(&self) {
        // Swap our connections out so we aren't holding the lock.
        // (The connection thread tries to remove itself from this map before exiting)
        let mut connections =
            std::mem::replace(&mut *self.connections.lock().unwrap(), HashMap::new());

        for (_, connection) in connections.iter_mut() {
            let _ = connection.sender.unbounded_send(ConnectionMessage::ContextKilled);
        }

        for (_, connection) in connections.iter_mut() {
            if let Some(handle) = connection.thread.take() {
                let _ = handle.join();
            }
        }
    }

    pub fn start_connection_thread(
        self: std::sync::Arc<Self>,
        client_id: u64,
        client_type: crate::traits::MagmaClientType,
        primary_channel: ServerEnd<fidl_fuchsia_gpu_magma::PrimaryMarker>,
        notification_channel: ServerEnd<fidl_fuchsia_gpu_magma::NotificationMarker>,
    ) {
        let (message_sender, message_receiver) = futures::channel::mpsc::unbounded::<
            crate::magma_system_connection::ConnectionMessage,
        >();

        let device = self.clone();
        let connection_sender = message_sender.clone();
        let thread = std::thread::Builder::new()
            .name(format!("ConnectionThread {}", client_id))
            .spawn(move || {
                let mut executor = fuchsia_async::LocalExecutor::default();
                executor.run_singlethreaded(async move {
                    let _ = fuchsia_scheduler::set_role_for_this_thread(
                        "fuchsia.graphics.magma.connection",
                    )
                    .log_err("Failed to set thread role");

                    let notification_handler = Arc::new(MagmaNotificationHandler {
                        notification_channel,
                        message_sender: connection_sender,
                    });
                    let Some(connection) =
                        device.open(client_id, client_type, notification_handler.clone())
                    else {
                        log::error!("Failed to create connection");
                        return;
                    };

                    let system_conn = crate::magma_system_connection::MagmaSystemConnection::new(
                        device.clone(),
                        connection,
                        notification_handler,
                    );

                    let mut server =
                        crate::primary_fidl_server::PrimaryFidlServer::new(system_conn);
                    let _ = server
                        .run(primary_channel.into_stream(), message_receiver)
                        .await
                        .log_err("Failed to run PrimaryFidlServer");

                    let mut map = device.connections.lock().unwrap();
                    map.remove(&client_id);
                });
            })
            .unwrap();
        self.connections
            .lock()
            .unwrap()
            .insert(client_id, Connection { sender: message_sender, thread: Some(thread) });
    }

    pub async fn handle_debug_utils(
        self: Arc<Self>,
        mut stream: fidl_fuchsia_gpu_magma::DebugUtilsRequestStream,
    ) -> anyhow::Result<()> {
        while let Some(request) = stream.try_next().await.context("Stream error")? {
            match request {
                fidl_fuchsia_gpu_magma::DebugUtilsRequest::SetPowerState {
                    power_state,
                    responder,
                } => {
                    let start_time = std::time::Instant::now();
                    let (sender, receiver) = futures::channel::oneshot::channel::<i32>();

                    self.msd_device.set_power_state(
                        power_state,
                        Box::new(move |status| {
                            let _ = sender.send(status);
                        }),
                    );

                    match receiver.await {
                        Ok(0) => {
                            let elapsed = start_time.elapsed().as_nanos() as u64;
                            responder.send(Ok(elapsed)).context("Failed to send response")?;
                        }
                        _ => {
                            responder
                                .send(Err(zx::Status::INTERNAL.into_raw()))
                                .context("Failed to send error response")?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

impl crate::magma_system_connection::Owner for std::sync::Arc<MagmaSystemDevice> {
    fn driver(&self) -> &dyn crate::traits::Driver {
        (**self).driver()
    }
    fn perf_count_access_token_id(&self) -> u64 {
        (**self).perf_count_access_token_id()
    }
}

impl crate::traits::Device for MagmaSystemDevice {
    fn set_memory_pressure_level(&self, level: u32) {
        self.msd_device.set_memory_pressure_level(level)
    }

    fn query(&self, id: u64) -> Result<(Option<zx::Vmo>, u64), MagmaStatus> {
        self.query(id)
    }

    fn get_icd_list(&self) -> Result<Vec<crate::traits::MsdIcdInfo>, MagmaStatus> {
        self.msd_device.get_icd_list()
    }

    fn set_power_state(&self, power_state: i64, callback: Box<dyn FnOnce(i32) + Send>) {
        self.msd_device.set_power_state(power_state, callback)
    }

    fn dump_status(&self, dump_flags: u32) {
        self.msd_device.dump_status(dump_flags)
    }

    fn open(
        &self,
        client_id: u64,
        client_type: crate::traits::MagmaClientType,
        notification_handler: Arc<dyn NotificationHandler>,
    ) -> Option<Box<dyn crate::traits::Connection>> {
        self.msd_device.open(client_id, client_type, notification_handler)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mock::{MockDevice, MockDriver};
    use crate::traits::Device;

    #[fuchsia::test]
    fn maximum_inflight_messages() {
        let driver = MockDriver;
        let msd_device = MockDevice;
        let device = MagmaSystemDevice::new(Box::new(driver), Box::new(msd_device), 0);

        const MAGMA_QUERY_MAXIMUM_INFLIGHT_PARAMS: u64 = 5;
        let result = device.query(MAGMA_QUERY_MAXIMUM_INFLIGHT_PARAMS).unwrap();
        let value = result.1;

        let messages = (value >> 32) as u32;
        let memory = value as u32;

        assert_eq!(messages, 1000);
        assert_eq!(memory, 100);
    }

    #[fuchsia::test]
    fn get_icd_list() {
        let driver = MockDriver;
        let msd_device = MockDevice;
        let device = MagmaSystemDevice::new(Box::new(driver), Box::new(msd_device), 0);

        let result = device.get_icd_list().unwrap();
        assert!(result.is_empty());
    }
}
