// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Mock I2C FIDL device server for testing.

use fidl_next::Responder;
use fidl_next_fuchsia_hardware_i2c as fidl_i2c;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// An expected I2C transfer and the response to return.
#[derive(Clone, Debug, PartialEq)]
pub struct Expectation {
    /// Expected list of transactions in the transfer request.
    pub expected_transactions: Vec<fidl_i2c::Transaction>,
    /// Staged response returned to the client.
    pub response: Result<Vec<Vec<u8>>, zx::Status>,
}

#[derive(Default)]
struct MockI2cState {
    expectations: VecDeque<Expectation>,
}

/// Mock FIDL server implementing [`fidl_i2c::DeviceServerHandler`].
///
/// Validates incoming I2C transactions against a queue of staged [`Expectation`]s
/// and returns pre-staged responses.
#[derive(Clone, Default)]
pub struct MockI2cDevice {
    state: Arc<Mutex<MockI2cState>>,
}

impl MockI2cDevice {
    /// Creates a new [`MockI2cDevice`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a list of [`Expectation`]s to the expectation queue.
    pub fn expect(&self, expectations: impl IntoIterator<Item = Expectation>) {
        self.state.lock().unwrap().expectations.extend(expectations);
    }

    /// Expects a register read transaction.
    pub fn expect_read(
        &self,
        register_address: u16,
        expected_read_size: u32,
        read_data: Result<Vec<u8>, zx::Status>,
    ) {
        let response = match read_data {
            Ok(data) => Ok(vec![data]),
            Err(status) => Err(status),
        };
        self.expect([Expectation {
            expected_transactions: vec![
                fidl_i2c::Transaction {
                    data_transfer: Some(fidl_i2c::DataTransfer::WriteData(
                        register_address.to_be_bytes().to_vec(),
                    )),
                    stop: Some(false),
                },
                fidl_i2c::Transaction {
                    data_transfer: Some(fidl_i2c::DataTransfer::ReadSize(expected_read_size)),
                    stop: Some(true),
                },
            ],
            response,
        }]);
    }

    /// Expects a register write transaction.
    pub fn expect_write(
        &self,
        register_address: u16,
        expected_payload: &[u8],
        result: Result<(), zx::Status>,
    ) {
        let mut expected_bytes = Vec::with_capacity(2 + expected_payload.len());
        expected_bytes.extend_from_slice(&register_address.to_be_bytes());
        expected_bytes.extend_from_slice(expected_payload);

        self.expect([Expectation {
            expected_transactions: vec![fidl_i2c::Transaction {
                data_transfer: Some(fidl_i2c::DataTransfer::WriteData(expected_bytes)),
                stop: Some(true),
            }],
            response: result.map(|()| Vec::new()),
        }]);
    }

    /// Asserts that all staged expectations have been replayed.
    pub fn check_all_expectations_replayed(&self) {
        let state = self.state.lock().unwrap();
        assert!(
            state.expectations.is_empty(),
            "Not all expectations were replayed: {:?}",
            state.expectations
        );
    }
}

impl fidl_i2c::DeviceServerHandler for MockI2cDevice {
    async fn transfer(
        &mut self,
        request: fidl_next::Request<fidl_i2c::device::Transfer>,
        responder: Responder<fidl_i2c::device::Transfer>,
    ) {
        let expectation = {
            let mut state = self.state.lock().unwrap();
            state
                .expectations
                .pop_front()
                .expect("unexpected I2C transfer: no more expectations staged in MockI2cDevice")
        };

        assert_eq!(
            request.payload().transactions,
            expectation.expected_transactions,
            "received I2C transactions do not match expected transactions"
        );

        match expectation.response {
            Ok(read_data) => {
                let _ = responder.respond(read_data).await;
            }
            Err(s) => {
                let _ = responder.respond_err(s).await;
            }
        }
    }

    async fn get_name(&mut self, responder: Responder<fidl_i2c::device::GetName>) {
        let _ = responder.respond("goodix-gt6853-i2c").await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[fuchsia::test]
    async fn test_mock_i2c_transfer_success() {
        let mock = MockI2cDevice::new();
        mock.expect([Expectation {
            expected_transactions: vec![fidl_i2c::Transaction {
                data_transfer: Some(fidl_i2c::DataTransfer::ReadSize(2)),
                stop: Some(true),
            }],
            response: Ok(vec![vec![0x12, 0x34]]),
        }]);

        let (client_end, server_end) = fidl_next::fuchsia::create_channel::<fidl_i2c::Device>();
        let client = client_end.spawn();
        #[expect(unused)]
        let server_task = server_end.spawn(mock.clone());

        let transactions = [fidl_i2c::Transaction {
            data_transfer: Some(fidl_i2c::DataTransfer::ReadSize(2)),
            stop: Some(true),
        }];

        let result = client.transfer(&transactions).await.expect("FIDL transfer failed");
        assert_eq!(result.expect("I2C transfer failed").read_data, vec![vec![0x12, 0x34]]);
        mock.check_all_expectations_replayed();
    }

    #[fuchsia::test]
    async fn test_mock_i2c_transfer_error() {
        let mock = MockI2cDevice::new();
        mock.expect([Expectation {
            expected_transactions: vec![fidl_i2c::Transaction {
                data_transfer: Some(fidl_i2c::DataTransfer::ReadSize(2)),
                stop: Some(true),
            }],
            response: Err(zx::Status::IO_REFUSED),
        }]);

        let (client_end, server_end) = fidl_next::fuchsia::create_channel::<fidl_i2c::Device>();
        let client = client_end.spawn();
        #[expect(unused)]
        let server_task = server_end.spawn(mock.clone());

        let transactions = [fidl_i2c::Transaction {
            data_transfer: Some(fidl_i2c::DataTransfer::ReadSize(2)),
            stop: Some(true),
        }];

        let result = client.transfer(&transactions).await.expect("FIDL call failed");
        assert_eq!(result.unwrap_err().err(), Some(zx::Status::IO_REFUSED));
        mock.check_all_expectations_replayed();
    }

    #[fuchsia::test]
    async fn test_mock_i2c_expect_read_and_write() {
        let mock = MockI2cDevice::new();
        mock.expect_read(0x452C, 4, Ok(vec![1, 2, 3, 4]));
        mock.expect_write(0x2180, &[0x24], Ok(()));

        let (client_end, server_end) = fidl_next::fuchsia::create_channel::<fidl_i2c::Device>();
        let client = client_end.spawn();
        #[expect(unused)]
        let server_task = server_end.spawn(mock.clone());

        let read_transactions = [
            fidl_i2c::Transaction {
                data_transfer: Some(fidl_i2c::DataTransfer::WriteData(vec![0x45, 0x2C])),
                stop: Some(false),
            },
            fidl_i2c::Transaction {
                data_transfer: Some(fidl_i2c::DataTransfer::ReadSize(4)),
                stop: Some(true),
            },
        ];
        let read_result = client.transfer(&read_transactions).await.expect("read transfer failed");
        assert_eq!(read_result.expect("read failed").read_data, vec![vec![1, 2, 3, 4]]);

        let write_transactions = [fidl_i2c::Transaction {
            data_transfer: Some(fidl_i2c::DataTransfer::WriteData(vec![0x21, 0x80, 0x24])),
            stop: Some(true),
        }];
        let write_result =
            client.transfer(&write_transactions).await.expect("write transfer failed");
        assert!(write_result.is_ok());

        mock.check_all_expectations_replayed();
    }

    #[fuchsia::test]
    async fn test_mock_i2c_get_name() {
        let mock = MockI2cDevice::new();
        let (client_end, server_end) = fidl_next::fuchsia::create_channel::<fidl_i2c::Device>();
        let client = client_end.spawn();
        #[expect(unused)]
        let server_task = server_end.spawn(mock);

        let name = client.get_name().await.expect("get_name failed");
        assert_eq!(name.expect("get_name result failed").name, "goodix-gt6853-i2c");
    }

    #[fuchsia::test]
    #[should_panic(expected = "Not all expectations were replayed")]
    fn test_mock_i2c_unmet_expectations_panics() {
        let mock = MockI2cDevice::new();
        mock.expect_read(0x452C, 4, Ok(vec![1, 2, 3, 4]));
        mock.check_all_expectations_replayed();
    }

    #[fuchsia::test]
    #[should_panic(expected = "received I2C transactions do not match expected transactions")]
    async fn test_mock_i2c_mismatched_transaction_panics() {
        let mock = MockI2cDevice::new();
        mock.expect_read(0x452C, 4, Ok(vec![1, 2, 3, 4]));

        let (client_end, server_end) = fidl_next::fuchsia::create_channel::<fidl_i2c::Device>();
        let client = client_end.spawn();
        let server_task = server_end.spawn(mock);

        let mismatched_transactions = [
            fidl_i2c::Transaction {
                data_transfer: Some(fidl_i2c::DataTransfer::WriteData(vec![0x12, 0x34])),
                stop: Some(false),
            },
            fidl_i2c::Transaction {
                data_transfer: Some(fidl_i2c::DataTransfer::ReadSize(4)),
                stop: Some(true),
            },
        ];
        let _ = client.transfer(&mismatched_transactions).await;
        let _ = server_task.await;
    }

    #[fuchsia::test]
    #[should_panic(
        expected = "unexpected I2C transfer: no more expectations staged in MockI2cDevice"
    )]
    async fn test_mock_i2c_unexpected_transfer_panics() {
        let mock = MockI2cDevice::new();
        let (client_end, server_end) = fidl_next::fuchsia::create_channel::<fidl_i2c::Device>();
        let client = client_end.spawn();
        let server_task = server_end.spawn(mock);

        let transactions = [fidl_i2c::Transaction {
            data_transfer: Some(fidl_i2c::DataTransfer::ReadSize(2)),
            stop: Some(true),
        }];
        let _ = client.transfer(&transactions).await;
        let _ = server_task.await;
    }
}
