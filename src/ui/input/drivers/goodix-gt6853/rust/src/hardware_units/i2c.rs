// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Type-safe I2C communication interface unit for Goodix GT6853 Touch IC.

use crate::data_types::traits::{ReadableRegister, WritableRegister};
use fidl_next_fuchsia_hardware_i2c as fidl_i2c;

/// Type-safe I2C message interface unit for the Goodix GT6853 Touch IC.
pub struct MessageInterfaceUnitI2c {
    client: fidl_next::Client<fidl_i2c::Device>,
}

impl MessageInterfaceUnitI2c {
    /// Creates a new [`MessageInterfaceUnitI2c`] with the given transport [`client`].
    pub const fn new(client: fidl_next::Client<fidl_i2c::Device>) -> Self {
        Self { client }
    }

    /// Reads a strongly-typed register implementing [`ReadableRegister`].
    ///
    /// # Errors
    ///
    /// Returns [`zx::Status::IO_DATA_INTEGRITY`] if the returned payload length does not match
    /// the expected register size, or a hardware [`zx::Status`] on bus transfer failure.
    ///
    /// All error conditions are logged.
    pub async fn read_reg<R: ReadableRegister>(&self) -> Result<R, zx::Status> {
        // @cite(gt6853-hardware-description): sec=2.2 title="Timing for Read Operation"
        // @cite(gt6853-programming-guide): sec=2.2 title="Timing for Read Operation"
        let address_bytes = R::ADDRESS.to_be_bytes();
        let expected_size = core::mem::size_of::<R>();

        let transactions = [
            fidl_i2c::Transaction {
                data_transfer: Some(fidl_i2c::DataTransfer::WriteData(address_bytes.to_vec())),
                stop: Some(false),
            },
            fidl_i2c::Transaction {
                data_transfer: Some(fidl_i2c::DataTransfer::ReadSize(expected_size as u32)),
                stop: Some(true),
            },
        ];

        let result = self
            .client
            .transfer(&transactions)
            .await
            .map_err(|e| {
                log::warn!("I2C FIDL transport error: {:?}", e);
                zx::Status::INTERNAL
            })?
            .map_err(|e| {
                log::warn!("I2C hardware transfer error: {:?}", e);
                e.err().unwrap_or(zx::Status::INTERNAL)
            })?;

        let [payload] = result.read_data.as_slice() else {
            log::warn!(
                "I2C read for register 0x{:04x} expected 1 read payload, got {}",
                R::ADDRESS,
                result.read_data.len()
            );
            return Err(zx::Status::IO_DATA_INTEGRITY);
        };

        if payload.len() != expected_size {
            log::warn!(
                "I2C read for register 0x{:04x} expected {} bytes, got {}",
                R::ADDRESS,
                expected_size,
                payload.len()
            );
            return Err(zx::Status::IO_DATA_INTEGRITY);
        }

        R::read_from_bytes(payload.as_slice()).map_err(|e| {
            log::warn!("Failed to parse register 0x{:04x} from bytes: {:?}", R::ADDRESS, e);
            zx::Status::IO_DATA_INTEGRITY
        })
    }

    /// Writes a strongly-typed register implementing [`WritableRegister`].
    ///
    /// # Errors
    ///
    /// Returns a hardware [`zx::Status`] on bus transfer failure.
    ///
    /// All error conditions are logged.
    pub async fn write_reg<W: WritableRegister>(&self, val: &W) -> Result<(), zx::Status> {
        // @cite(gt6853-hardware-description): sec=2.1 title="Timing for Write Operation"
        // @cite(gt6853-programming-guide): sec=2.1 title="Timing for Write Operation"
        let address_bytes = W::ADDRESS.to_be_bytes();
        let val_bytes = val.as_bytes();

        let mut write_payload = Vec::with_capacity(address_bytes.len() + val_bytes.len());
        write_payload.extend_from_slice(&address_bytes);
        write_payload.extend_from_slice(val_bytes);

        let transactions = [fidl_i2c::Transaction {
            data_transfer: Some(fidl_i2c::DataTransfer::WriteData(write_payload)),
            stop: Some(true),
        }];

        self.client
            .transfer(&transactions)
            .await
            .map_err(|e| {
                log::warn!("I2C FIDL transport error: {:?}", e);
                zx::Status::INTERNAL
            })?
            .map_err(|e| {
                log::warn!("I2C hardware write error: {:?}", e);
                e.err().unwrap_or(zx::Status::INTERNAL)
            })?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data_types::reports::{InitialTouchReport, RemainingTouchContacts};
    use crate::data_types::traits::AddressableRegister;
    use crate::registers::control::CpuControl;
    use crate::registers::product_info::ProductIdentification;
    use crate::registers::status::{BootTarget, FirmwareStatus};
    use crate::testing::mock_i2c::{Expectation, MockI2cDevice};
    use zerocopy::IntoBytes;

    struct TestFixture {
        mock: MockI2cDevice,
        unit: MessageInterfaceUnitI2c,
    }

    impl TestFixture {
        fn new() -> (Self, fidl_next::ServerEnd<fidl_i2c::Device>) {
            let mock = MockI2cDevice::new();
            let (client_end, server_end) = fidl_next::fuchsia::create_channel::<fidl_i2c::Device>();
            let client = client_end.spawn();
            let unit = MessageInterfaceUnitI2c::new(client);
            (Self { mock, unit }, server_end)
        }
    }

    #[fuchsia::test]
    async fn test_read_reg_success() {
        let (fixture, server_end) = TestFixture::new();
        #[expect(unused)]
        let server_task = server_end.spawn(fixture.mock.clone());
        fixture.mock.expect_read(FirmwareStatus::ADDRESS, 4, Ok(vec![0x08, 0x00, 0x00, 0x00]));

        let status: FirmwareStatus = fixture.unit.read_reg().await.expect("read_reg failed");
        assert_eq!(status, FirmwareStatus::HEALTHY);
        fixture.mock.check_all_expectations_replayed();
    }

    #[fuchsia::test]
    async fn test_write_reg_single_byte() {
        let (fixture, server_end) = TestFixture::new();
        #[expect(unused)]
        let server_task = server_end.spawn(fixture.mock.clone());
        fixture.mock.expect_write(CpuControl::ADDRESS, &[0x24], Ok(()));

        fixture.unit.write_reg(&CpuControl::HOLD).await.expect("write_reg failed");
        fixture.mock.check_all_expectations_replayed();
    }

    #[fuchsia::test]
    async fn test_write_reg_multi_byte() {
        let (fixture, server_end) = TestFixture::new();
        #[expect(unused)]
        let server_task = server_end.spawn(fixture.mock.clone());
        fixture.mock.expect_write(BootTarget::ADDRESS, &[0x55; 8], Ok(()));

        fixture.unit.write_reg(&BootTarget::RAM).await.expect("write_reg failed");
        fixture.mock.check_all_expectations_replayed();
    }

    #[fuchsia::test]
    async fn test_read_initial_touch_report_10_bytes() {
        let (fixture, server_end) = TestFixture::new();
        #[expect(unused)]
        let server_task = server_end.spawn(fixture.mock.clone());
        let mut raw_10_bytes = vec![0x81, 0x01];
        let contact0 = [0x00, 0xE8, 0x03, 0xD0, 0x07, 0x64, 0x00, 0x00];
        raw_10_bytes.extend_from_slice(&contact0);
        fixture.mock.expect_read(InitialTouchReport::ADDRESS, 10, Ok(raw_10_bytes));

        let report: InitialTouchReport =
            fixture.unit.read_reg().await.expect("read_reg InitialTouchReport failed");

        assert_eq!(report.status.coordinates_ready(), true);
        assert_eq!(report.contact_count.count(), 1);
        assert_eq!(report.first_contact.x(), 1000);
        assert_eq!(report.first_contact.y(), 2000);
        assert_eq!(report.first_contact.id(), 0);
        fixture.mock.check_all_expectations_replayed();
    }

    #[fuchsia::test]
    async fn test_read_remaining_touch_contacts_burst() {
        let (fixture, server_end) = TestFixture::new();
        #[expect(unused)]
        let server_task = server_end.spawn(fixture.mock.clone());
        let mut remaining_raw = Vec::new();
        for i in 1..=3 {
            let contact = [i as u8, 0x64, 0x00, 0xC8, 0x00, 0x32, 0x00, 0x00];
            remaining_raw.extend_from_slice(&contact);
        }
        fixture.mock.expect_read(RemainingTouchContacts::<3>::ADDRESS, 24, Ok(remaining_raw));

        let remaining: RemainingTouchContacts<3> =
            fixture.unit.read_reg().await.expect("read_reg RemainingTouchContacts failed");

        assert_eq!(remaining.contacts[0].id(), 1);
        assert_eq!(remaining.contacts[1].id(), 2);
        assert_eq!(remaining.contacts[2].id(), 3);
        fixture.mock.check_all_expectations_replayed();
    }

    #[fuchsia::test]
    async fn test_read_composite_product_identification_72_bytes() {
        let (fixture, server_end) = TestFixture::new();
        #[expect(unused)]
        let server_task = server_end.spawn(fixture.mock.clone());
        let prod_info_raw = vec![0x42; 72];
        fixture.mock.expect_read(ProductIdentification::ADDRESS, 72, Ok(prod_info_raw));

        let prod_info: ProductIdentification =
            fixture.unit.read_reg().await.expect("read product info failed");
        assert_eq!(prod_info.as_bytes().len(), 72);
        fixture.mock.check_all_expectations_replayed();
    }

    #[fuchsia::test]
    async fn test_read_reg_i2c_bus_failure_propagates() {
        let (fixture, server_end) = TestFixture::new();
        #[expect(unused)]
        let server_task = server_end.spawn(fixture.mock.clone());
        fixture.mock.expect_read(FirmwareStatus::ADDRESS, 4, Err(zx::Status::IO_REFUSED));

        let result: Result<FirmwareStatus, zx::Status> = fixture.unit.read_reg().await;
        assert_eq!(result.err(), Some(zx::Status::IO_REFUSED));
        fixture.mock.check_all_expectations_replayed();
    }

    #[fuchsia::test]
    async fn test_read_reg_length_mismatch_returns_io_data_integrity() {
        let (fixture, server_end) = TestFixture::new();
        #[expect(unused)]
        let server_task = server_end.spawn(fixture.mock.clone());
        fixture.mock.expect_read(FirmwareStatus::ADDRESS, 4, Ok(vec![0x01]));

        let result: Result<FirmwareStatus, zx::Status> = fixture.unit.read_reg().await;
        assert_eq!(result.err(), Some(zx::Status::IO_DATA_INTEGRITY));
        fixture.mock.check_all_expectations_replayed();
    }

    #[fuchsia::test]
    async fn test_read_reg_no_payload_returns_io_data_integrity() {
        let (fixture, server_end) = TestFixture::new();
        #[expect(unused)]
        let server_task = server_end.spawn(fixture.mock.clone());
        fixture.mock.expect([Expectation {
            expected_transactions: vec![
                fidl_i2c::Transaction {
                    data_transfer: Some(fidl_i2c::DataTransfer::WriteData(
                        FirmwareStatus::ADDRESS.to_be_bytes().to_vec(),
                    )),
                    stop: Some(false),
                },
                fidl_i2c::Transaction {
                    data_transfer: Some(fidl_i2c::DataTransfer::ReadSize(4)),
                    stop: Some(true),
                },
            ],
            response: Ok(vec![]),
        }]);

        let result: Result<FirmwareStatus, zx::Status> = fixture.unit.read_reg().await;
        assert_eq!(result.err(), Some(zx::Status::IO_DATA_INTEGRITY));
        fixture.mock.check_all_expectations_replayed();
    }

    #[fuchsia::test]
    async fn test_write_reg_i2c_bus_failure_propagates() {
        let (fixture, server_end) = TestFixture::new();
        #[expect(unused)]
        let server_task = server_end.spawn(fixture.mock.clone());
        fixture.mock.expect_write(CpuControl::ADDRESS, &[0x24], Err(zx::Status::IO_REFUSED));

        let result = fixture.unit.write_reg(&CpuControl::HOLD).await;
        assert_eq!(result.err(), Some(zx::Status::IO_REFUSED));
        fixture.mock.check_all_expectations_replayed();
    }
}
