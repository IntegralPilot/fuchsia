// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#![deny(missing_docs)]
//! PlatformDevice interface.

use fdf_component::DriverError;
use fidl::{Persistable, Serializable};
use fidl_next_fuchsia_hardware_platform_device as fpdev;
use log::{debug, error};
use mmio::region::MmioRegion;
use mmio::vmo::{VmoMapping, VmoMemory};
use std::future::Future;
use zx_status::Status;

/// PlatformDevice interface.
pub trait PlatformDevice {
    /// The type of the [Mmio] implementation returned by this platform device.
    type Mmio;

    /// Maps an MMIO region by its id.
    fn map_mmio_by_id(&self, id: u32) -> impl Future<Output = Result<Self::Mmio, DriverError>>;

    /// Maps MMIO memory by its name.
    fn map_mmio_by_name(&self, name: &str)
    -> impl Future<Output = Result<Self::Mmio, DriverError>>;

    /// Gets typed metadata associated with this platform device.
    fn get_typed_metadata<T: Persistable + Serializable>(
        &self,
    ) -> impl Future<Output = Result<T, DriverError>>;

    /// Gets deserialized metadata associated with this platform device using default ID.
    fn get_deserialized_metadata<T: serde::de::DeserializeOwned>(
        &self,
    ) -> impl Future<Output = Result<T, DriverError>>;

    /// Gets persisted metadata bytes associated with this platform device by metadata ID.
    fn get_persisted_metadata_by_id(
        &self,
        metadata_id: &str,
    ) -> impl Future<Output = Result<Vec<u8>, DriverError>>;

    /// Gets metadata dictionary associated with this platform device by metadata ID.
    fn get_dictionary_metadata(
        &self,
        metadata_id: &str,
    ) -> impl Future<Output = Result<fidl_fuchsia_driver_metadata::Dictionary, DriverError>>;
}

impl PlatformDevice for fidl_next::Client<fpdev::Device> {
    type Mmio = MmioRegion<VmoMemory>;

    async fn map_mmio_by_id(&self, id: u32) -> Result<Self::Mmio, DriverError> {
        let mmio =
            self.get_mmio_by_id(id).await?.map_err(|s| s.err().unwrap_or(Status::INTERNAL))?;
        Ok(map_mmio(mmio)?)
    }

    async fn map_mmio_by_name(&self, name: &str) -> Result<Self::Mmio, DriverError> {
        let mmio =
            self.get_mmio_by_name(name).await?.map_err(|s| s.err().unwrap_or(Status::INTERNAL))?;
        Ok(map_mmio(mmio)?)
    }

    async fn get_typed_metadata<T: Persistable + Serializable>(&self) -> Result<T, DriverError> {
        let name = T::SERIALIZABLE_NAME;
        let metadata_res =
            self.get_metadata(name).await?.map_err(|s| s.err().unwrap_or(Status::INTERNAL))?;
        fidl::unpersist(&metadata_res.metadata).map_err(|err| {
            error!("Failed to parse pdev metadata: {err}");
            DriverError::Status(Status::INVALID_ARGS)
        })
    }

    async fn get_deserialized_metadata<T: serde::de::DeserializeOwned>(
        &self,
    ) -> Result<T, DriverError> {
        let name = "fuchsia.driver.metadata.Dictionary";
        let metadata_res =
            self.get_metadata(name).await?.map_err(|s| s.err().unwrap_or(Status::INTERNAL))?;
        let dict: fidl_fuchsia_driver_metadata::Dictionary =
            fidl::unpersist(&metadata_res.metadata).map_err(|err| {
                error!("Failed to unpersist dictionary: {err}");
                DriverError::Status(Status::INVALID_ARGS)
            })?;
        fdf_metadata::from_dictionary(dict).map_err(|err| {
            error!("Failed to deserialize config from dictionary: {err:?}");
            DriverError::Status(Status::INVALID_ARGS)
        })
    }

    async fn get_persisted_metadata_by_id(
        &self,
        metadata_id: &str,
    ) -> Result<Vec<u8>, DriverError> {
        let metadata_res = self
            .get_metadata(metadata_id)
            .await?
            .map_err(|s| s.err().unwrap_or(Status::INTERNAL))?;
        Ok(metadata_res.metadata)
    }

    async fn get_dictionary_metadata(
        &self,
        metadata_id: &str,
    ) -> Result<fidl_fuchsia_driver_metadata::Dictionary, DriverError> {
        let bytes = self.get_persisted_metadata_by_id(metadata_id).await?;
        fidl::unpersist(&bytes).map_err(|err| {
            debug!("Failed to unpersist metadata dictionary for {}: {:?}", metadata_id, err);
            DriverError::Status(Status::INVALID_ARGS)
        })
    }
}

/// Extension trait for [`DriverContext`] to simplify connecting to a platform device in a driver's
/// start routine.
pub trait PdevExt {
    /// Connects to the platform device ("pdev") in the incoming namespace.
    fn connect_to_pdev(&self) -> Result<fidl_next::Client<fpdev::Device>, DriverError>;
}

impl PdevExt for fdf_component::DriverContext {
    fn connect_to_pdev(&self) -> Result<fidl_next::Client<fpdev::Device>, DriverError> {
        let service = self
            .incoming
            .service::<fdf_component::ServiceInstance<fpdev::Service>>()
            .instance("pdev")
            .connect_next()?;
        let (client_end, server_end) = fidl_next::fuchsia::create_channel();
        service.device(server_end)?;
        Ok(client_end.spawn())
    }
}

fn map_mmio(mmio: fpdev::Mmio) -> Result<MmioRegion<VmoMemory>, Status> {
    let (Some(vmo), Some(offset), Some(size)) = (mmio.vmo, mmio.offset, mmio.size) else {
        error!("Mmio device missing vmo, offset or size");
        return Err(Status::INTERNAL);
    };
    let offset = offset as usize;
    let size = size as usize;

    let mmio = VmoMapping::map(offset, size, vmo).map_err(|err| {
        error!("Failed to map Mmio memory for vmo: {err}");
        Status::INTERNAL
    })?;
    Ok(mmio)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fidl_next::{Request, Responder};
    use fidl_test_metadata::{IntMetadata, Metadata};
    use fuchsia_async::Task;
    use mmio::Mmio;
    use std::collections::HashMap;
    use zx::{Vmo, VmoOp};

    struct TestServer {
        mmios: Vec<(&'static str, Option<fpdev::Mmio>)>,
        metadata: HashMap<&'static str, Vec<u8>>,
    }

    impl TestServer {
        fn new() -> Self {
            Self { mmios: Vec::new(), metadata: HashMap::new() }
        }

        fn append_mmio(&mut self, name: &'static str, vmo: Vmo, offset: usize, size: usize) {
            self.mmios.push((
                name,
                Some(fpdev::Mmio {
                    offset: Some(offset as u64),
                    size: Some(size as u64),
                    vmo: Some(vmo),
                }),
            ));
        }

        fn set_typed_metadata<T: Persistable + Serializable>(&mut self, metadata: &T) {
            let bytes = fidl::persist(metadata).unwrap();
            self.metadata.insert(T::SERIALIZABLE_NAME, bytes);
        }

        fn take_mmio_by_id(&mut self, id: u32) -> Result<fpdev::Mmio, Status> {
            self.mmios
                .get_mut(id as usize)
                .ok_or(Status::NOT_FOUND)?
                .1
                .take()
                .ok_or(Status::ALREADY_BOUND)
        }

        fn take_mmio_by_name(&mut self, name: &str) -> Result<fpdev::Mmio, Status> {
            self.mmios
                .iter_mut()
                .find(|(n, _)| *n == name)
                .ok_or(Status::NOT_FOUND)?
                .1
                .take()
                .ok_or(Status::ALREADY_BOUND)
        }

        fn read_metadata(&self, id: &str) -> Result<&[u8], Status> {
            self.metadata.get(id).map(|v| v.as_slice()).ok_or(Status::NOT_FOUND)
        }

        fn run(
            self,
        ) -> (
            fidl_next::Client<fpdev::Device>,
            Task<Result<(), fidl_next::ProtocolError<zx::Status>>>,
        ) {
            let (client_end, server_end) = fidl_next::fuchsia::create_channel::<fpdev::Device>();
            let client = client_end.spawn();
            let server = Task::local(async move {
                let dispatcher = fidl_next::ServerDispatcher::new(server_end);
                dispatcher.run_local(self).await.map(|_| ())
            });
            (client, server)
        }
    }

    impl fpdev::DeviceLocalServerHandler for TestServer {
        async fn get_mmio_by_id(
            &mut self,
            request: Request<fpdev::device::GetMmioById>,
            responder: Responder<fpdev::device::GetMmioById>,
        ) {
            let index = request.payload().index;
            match self.take_mmio_by_id(index) {
                Ok(mmio) => {
                    let _ = responder.respond(mmio).await;
                }
                Err(status) => {
                    let _ = responder.respond_err(status).await;
                }
            }
        }

        async fn get_mmio_by_name(
            &mut self,
            request: Request<fpdev::device::GetMmioByName>,
            responder: Responder<fpdev::device::GetMmioByName>,
        ) {
            let name = &request.payload().name;
            match self.take_mmio_by_name(name) {
                Ok(mmio) => {
                    let _ = responder.respond(mmio).await;
                }
                Err(status) => {
                    let _ = responder.respond_err(status).await;
                }
            }
        }

        async fn get_interrupt_by_id(
            &mut self,
            _request: Request<fpdev::device::GetInterruptById>,
            _responder: Responder<fpdev::device::GetInterruptById>,
        ) {
            unimplemented!("not used by tests");
        }

        async fn get_interrupt_by_name(
            &mut self,
            _request: Request<fpdev::device::GetInterruptByName>,
            _responder: Responder<fpdev::device::GetInterruptByName>,
        ) {
            unimplemented!("not used by tests");
        }

        async fn get_bti_by_id(
            &mut self,
            _request: Request<fpdev::device::GetBtiById>,
            _responder: Responder<fpdev::device::GetBtiById>,
        ) {
            unimplemented!("not used by tests");
        }

        async fn get_bti_by_name(
            &mut self,
            _request: Request<fpdev::device::GetBtiByName>,
            _responder: Responder<fpdev::device::GetBtiByName>,
        ) {
            unimplemented!("not used by tests");
        }

        async fn get_smc_by_id(
            &mut self,
            _request: Request<fpdev::device::GetSmcById>,
            _responder: Responder<fpdev::device::GetSmcById>,
        ) {
            unimplemented!("not used by tests");
        }

        async fn get_smc_by_name(
            &mut self,
            _request: Request<fpdev::device::GetSmcByName>,
            _responder: Responder<fpdev::device::GetSmcByName>,
        ) {
            unimplemented!("not used by tests");
        }

        async fn get_power_configuration(
            &mut self,
            _responder: Responder<fpdev::device::GetPowerConfiguration>,
        ) {
            unimplemented!("not used by tests");
        }

        async fn get_node_device_info(
            &mut self,
            _responder: Responder<fpdev::device::GetNodeDeviceInfo>,
        ) {
            unimplemented!("not used by tests");
        }

        async fn get_board_info(&mut self, _responder: Responder<fpdev::device::GetBoardInfo>) {
            unimplemented!("not used by tests");
        }

        async fn get_metadata(
            &mut self,
            request: Request<fpdev::device::GetMetadata>,
            responder: Responder<fpdev::device::GetMetadata>,
        ) {
            let id = &request.payload().id;
            match self.read_metadata(id) {
                Ok(metadata) => {
                    let _ = responder.respond(metadata).await;
                }
                Err(status) => {
                    let _ = responder.respond_err(status).await;
                }
            }
        }
    }

    #[fuchsia::test]
    async fn test_pdev() {
        let mut server = TestServer::new();

        let vmo = Vmo::create(4096).unwrap();
        vmo.op_range(VmoOp::ZERO, 0, 4096).unwrap();
        server.append_mmio("zero", vmo, 0, 4096);

        // Prepare the MMIO region.
        let vmo = Vmo::create(1024).unwrap();
        for i in 0..256 {
            vmo.write(&((i as u32).to_le_bytes()), (i * size_of::<u32>()) as u64).unwrap();
        }
        server.append_mmio("dev", vmo, 32 * size_of::<u32>(), 16);

        server.set_typed_metadata(&Metadata {
            test_field: Some("foo".to_string()),
            ..Default::default()
        });

        let dict = fidl_fuchsia_driver_metadata::Dictionary {
            entries: Some(vec![fidl_fuchsia_driver_metadata::DictionaryEntry {
                key: "test_key".to_string(),
                value: fidl_fuchsia_driver_metadata::DictionaryValue::Str("test_val".to_string()),
            }]),
            ..Default::default()
        };
        let dict_bytes = fidl::persist(&dict).unwrap();
        server.metadata.insert("test_dict_id", dict_bytes.clone());

        let (client, server) = server.run();

        let mmio = client.map_mmio_by_id(1).await.unwrap();
        assert_eq!(
            client.map_mmio_by_id(1).await.err().map(|e| e.log_to_status()),
            Some(Status::ALREADY_BOUND)
        );
        assert_eq!(
            client.map_mmio_by_id(2).await.err().map(|e| e.log_to_status()),
            Some(Status::NOT_FOUND)
        );
        assert_eq!(
            client.map_mmio_by_name("dev").await.err().map(|e| e.log_to_status()),
            Some(Status::ALREADY_BOUND)
        );

        assert_eq!(mmio.load32(0), 32);

        let mmio = client.map_mmio_by_name("zero").await.unwrap();
        assert_eq!(mmio.len(), 4096);
        assert_eq!(mmio.load64(128), 0);

        assert_eq!(
            client.get_typed_metadata::<Metadata>().await.unwrap(),
            Metadata { test_field: Some("foo".to_string()), ..Default::default() }
        );

        assert_eq!(
            client.get_typed_metadata::<IntMetadata>().await.err().map(|e| e.log_to_status()),
            Some(Status::NOT_FOUND)
        );

        let raw_bytes = client.get_persisted_metadata_by_id("test_dict_id").await.unwrap();
        assert_eq!(raw_bytes, dict_bytes);

        let retrieved_dict = client.get_dictionary_metadata("test_dict_id").await.unwrap();
        assert_eq!(retrieved_dict, dict);

        // Probing for dictionary metadata when typed metadata is present fails gracefully.
        assert_eq!(
            client
                .get_dictionary_metadata(Metadata::SERIALIZABLE_NAME)
                .await
                .err()
                .map(|e| e.log_to_status()),
            Some(Status::INVALID_ARGS)
        );

        let _ = server.abort().await;
    }
}
