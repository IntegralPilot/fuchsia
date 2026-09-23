// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "i2c-channel.h"

namespace i2c {

zx::result<I2cChannel> I2cChannel::FromIncoming(fdf::Namespace& incoming,
                                                std::string_view parent_name) {
  zx::result i2c = incoming.Connect<fuchsia_hardware_i2c::Service::Device>(parent_name);
  if (i2c.is_error()) {
    return i2c.take_error();
  }
  return zx::ok(I2cChannel{std::move(i2c.value())});
}

zx::result<size_t> I2cChannel::ReadSync(uint8_t addr, std::span<uint8_t> read_data) {
  std::array<uint8_t, 1> write_data = {addr};
  return WriteReadSync(write_data, read_data);
}

zx::result<> I2cChannel::WriteSync(std::span<const uint8_t> write_data) {
  return zx::make_result(WriteReadSync(write_data, {}).status_value());
}

I2cChannel::RetryResult<size_t> I2cChannel::ReadSyncRetries(uint8_t addr,
                                                            std::span<uint8_t> read_data,
                                                            uint8_t max_retry_count,
                                                            zx::duration retry_delay) {
  std::array<uint8_t, 1> write_data{addr};
  return WriteReadSyncRetries(write_data, read_data, max_retry_count, retry_delay);
}

I2cChannel::RetryResult<> I2cChannel::WriteSyncRetries(std::span<const uint8_t> write_data,
                                                       size_t max_retry_count,
                                                       zx::duration retry_delay) {
  RetryResult result = WriteReadSyncRetries(write_data, {}, max_retry_count, retry_delay);
  if (result.is_error()) {
    return {result.take_error(), result.retry_count()};
  }
  return {zx::ok(), result.retry_count()};
}

I2cChannel::RetryResult<size_t> I2cChannel::WriteReadSyncRetries(
    std::span<const uint8_t> write_data, std::span<uint8_t> read_data, size_t max_retry_count,
    zx::duration retry_delay) {
  size_t retry_count = 0;
  zx::result result = WriteReadSync(write_data, read_data);
  while (result.is_error() && retry_count < max_retry_count) {
    zx::nanosleep(zx::deadline_after(retry_delay));
    retry_count++;
    result = WriteReadSync(write_data, read_data);
  }
  return {result, retry_count};
}

zx::result<size_t> I2cChannel::WriteReadSync(std::span<const uint8_t> write_data,
                                             std::span<uint8_t> read_data) {
  auto read_size = read_data.size();
  if (write_data.size() > fuchsia_hardware_i2c::wire::kMaxTransferSize ||
      read_size > fuchsia_hardware_i2c::wire::kMaxTransferSize) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  fidl::Arena arena;

  fuchsia_hardware_i2c::wire::Transaction transactions[2];
  size_t index = 0;
  if (!write_data.empty()) {
    fidl::VectorView<uint8_t> write_data_fidl(arena, write_data.size());
    if (!write_data.empty()) {
      memcpy(write_data_fidl.data(), write_data.data(), write_data.size());
    }
    auto write_transfer =
        fuchsia_hardware_i2c::wire::DataTransfer::WithWriteData(arena, write_data_fidl);
    transactions[index++] = fuchsia_hardware_i2c::wire::Transaction::Builder(arena)
                                .data_transfer(write_transfer)
                                .Build();
  }
  if (read_size > 0) {
    auto read_transfer =
        fuchsia_hardware_i2c::wire::DataTransfer::WithReadSize(static_cast<uint32_t>(read_size));
    transactions[index++] = fuchsia_hardware_i2c::wire::Transaction::Builder(arena)
                                .data_transfer(read_transfer)
                                .Build();
  }

  if (index == 0) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  const auto reply = client_->Transfer(
      fidl::VectorView<fuchsia_hardware_i2c::wire::Transaction>::FromExternal(transactions, index));
  if (!reply.ok()) {
    return zx::error(reply.status());
  }
  if (reply->is_error()) {
    return zx::error(reply->error_value());
  }

  size_t bytes_read = 0;
  if (read_size > 0) {
    const auto& read_data_src = reply->value()->read_data;
    // Truncate the returned buffer to match the behavior of the Banjo version.
    if (read_data_src.size() != 1) {
      return zx::error(ZX_ERR_IO);
    }

    bytes_read = std::min(read_size, read_data_src[0].size());
    memcpy(read_data.data(), read_data_src[0].data(), bytes_read);
  }

  return zx::ok(bytes_read);
}

fidl::WireResult<fuchsia_hardware_i2c::Device::Transfer> I2cChannel::Transfer(
    fidl::VectorView<fuchsia_hardware_i2c::wire::Transaction> transactions) {
  return client_->Transfer(transactions);
}

fidl::WireResult<fuchsia_hardware_i2c::Device::GetName> I2cChannel::GetName() {
  return client_->GetName();
}

}  // namespace i2c
