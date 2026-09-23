// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_I2C_LIB_I2C_CHANNEL_I2C_CHANNEL_H_
#define SRC_DEVICES_I2C_LIB_I2C_CHANNEL_I2C_CHANNEL_H_

#include <fidl/fuchsia.hardware.i2c/cpp/wire.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <zircon/types.h>

namespace i2c {

class I2cChannel {
 public:
  template <typename... Ts>
  class RetryResult : public zx::result<Ts...> {
   public:
    using base = zx::result<Ts...>;

    constexpr RetryResult(const base& other, size_t retry_count)
        : zx::result<Ts...>(other), retry_count_(retry_count) {}

    constexpr RetryResult(const base&& other, size_t retry_count)
        : zx::result<Ts...>(other), retry_count_(retry_count) {}

    size_t retry_count() const { return retry_count_; }

   private:
    size_t retry_count_;
  };

  // Returns an `I2cChannel` that is connected to the i2c fidl service `parent_name` instance found
  // in the namespace `incoming`.
  static zx::result<I2cChannel> FromIncoming(
      fdf::Namespace& incoming, std::string_view parent_name = component::kDefaultInstance);

  I2cChannel() = default;

  explicit I2cChannel(fidl::ClientEnd<fuchsia_hardware_i2c::Device> client)
      : client_(std::move(client)) {}

  I2cChannel(I2cChannel&& other) noexcept = default;
  I2cChannel& operator=(I2cChannel&& other) noexcept = default;

  I2cChannel(const I2cChannel& other) = delete;
  I2cChannel& operator=(const I2cChannel& other) = delete;

  ~I2cChannel() = default;

  // Performs typical i2c read: Writes `addr` (1 byte) and then reads `read_data.size()` bytes and
  // puts them in `read_data`. Returns the number of bytes read.
  zx::result<size_t> ReadSync(uint8_t addr, std::span<uint8_t> read_data);

  // Writes `write_data` with no trailing read.
  zx::result<> WriteSync(std::span<const uint8_t> write_data);

  // ReadSync() that will retry `max_retry_count` times. The function will wait for `retry_delay`
  // between each retry.
  RetryResult<size_t> ReadSyncRetries(uint8_t addr, std::span<uint8_t> read_data,
                                      uint8_t max_retry_count, zx::duration retry_delay);

  // WriteSync() that will retry `max_retry_count` times. The function will wait for `retry_delay`
  // between each retry.
  RetryResult<> WriteSyncRetries(std::span<const uint8_t> write_data, size_t max_retry_count,
                                 zx::duration retry_delay);

  // WriteSync() that will retry `max_retry_count` times. The function will wait for `retry_delay`
  // between each retry. Returns the number of bytes read.
  RetryResult<size_t> WriteReadSyncRetries(std::span<const uint8_t> write_data,
                                           std::span<uint8_t> read_data, size_t max_retry_count,
                                           zx::duration retry_delay);

  // Writes `write_data` and then reads `read_data.size()` bytes and puts them in `read_data`.
  zx::result<size_t> WriteReadSync(std::span<const uint8_t> write_data,
                                   std::span<uint8_t> read_data);

  fidl::WireResult<fuchsia_hardware_i2c::Device::Transfer> Transfer(
      fidl::VectorView<fuchsia_hardware_i2c::wire::Transaction> transactions);

  fidl::WireResult<fuchsia_hardware_i2c::Device::GetName> GetName();

  bool is_valid() const { return client_.is_valid(); }

 private:
  fidl::WireSyncClient<fuchsia_hardware_i2c::Device> client_;
};

}  // namespace i2c

template <typename... Ts>
struct std::formatter<i2c::I2cChannel::RetryResult<Ts...>> {
  constexpr auto parse(std::format_parse_context& ctx) { return ctx.begin(); }

  auto format(const i2c::I2cChannel::RetryResult<Ts...>& result, std::format_context& ctx) const {
    return std::format_to(ctx.out(), "{} after {} retries", result.status_string(),
                          result.retry_count());
  }
};

#endif  // SRC_DEVICES_I2C_LIB_I2C_CHANNEL_I2C_CHANNEL_H_
