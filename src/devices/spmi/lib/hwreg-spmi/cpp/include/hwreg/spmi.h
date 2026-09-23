// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_SPMI_LIB_HWREG_SPMI_CPP_INCLUDE_HWREG_SPMI_H_
#define SRC_DEVICES_SPMI_LIB_HWREG_SPMI_CPP_INCLUDE_HWREG_SPMI_H_

#include <endian.h>
#include <fidl/fuchsia.hardware.spmi/cpp/wire.h>
#include <lib/fit/function.h>
#include <lib/zx/result.h>

#include <hwreg/bitfields.h>

namespace hwreg {

namespace internal {

inline zx::result<> MapError(fuchsia_hardware_spmi::DriverError error) {
  switch (error) {
    case fuchsia_hardware_spmi::DriverError::kInternal:
      return zx::error(ZX_ERR_INTERNAL);
    case fuchsia_hardware_spmi::DriverError::kNotSupported:
      return zx::error(ZX_ERR_NOT_SUPPORTED);
    case fuchsia_hardware_spmi::DriverError::kInvalidArgs:
      return zx::error(ZX_ERR_INVALID_ARGS);
    case fuchsia_hardware_spmi::DriverError::kBadState:
      return zx::error(ZX_ERR_BAD_STATE);
    case fuchsia_hardware_spmi::DriverError::kIoRefused:
      return zx::error(ZX_ERR_IO_REFUSED);
    default:
      return zx::error(ZX_ERR_NOT_FOUND);
  }
}

// Methods to convert between host and big/little endian for the SPMI bus.
static inline uint32_t HostToBigEndian(uint32_t val) { return htobe32(val); }
static inline uint16_t HostToBigEndian(uint16_t val) { return htobe16(val); }
static inline uint8_t HostToBigEndian(uint8_t val) { return val; }
static inline uint32_t BigEndianToHost(uint32_t val) { return betoh32(val); }
static inline uint16_t BigEndianToHost(uint16_t val) { return betoh16(val); }
static inline uint8_t BigEndianToHost(uint8_t val) { return val; }
static inline uint32_t HostToLittleEndian(uint32_t val) { return htole32(val); }
static inline uint16_t HostToLittleEndian(uint16_t val) { return htole16(val); }
static inline uint8_t HostToLittleEndian(uint8_t val) { return val; }
static inline uint32_t LittleEndianToHost(uint32_t val) { return letoh32(val); }
static inline uint16_t LittleEndianToHost(uint16_t val) { return letoh16(val); }
static inline uint8_t LittleEndianToHost(uint8_t val) { return val; }

template <typename ResultType>
zx::result<const uint8_t*> ParseReadResponse(const ResultType& response, size_t expected_size) {
  if (!response.ok()) {
    return zx::error(response.status());
  }
  const auto& res = response.value();
  if (res.is_error()) {
    return MapError(res.error_value()).take_error();
  }
  const auto& data = res.value()->data;
  if (data.size() != expected_size) {
    return zx::error(ZX_ERR_IO_DATA_INTEGRITY);
  }
  return zx::ok(data.data());
}

template <typename ResultType>
zx::result<const uint8_t*> ParseReadResponse(const ResultType&&, size_t) = delete;

template <typename ResultType>
zx::result<> ParseWriteResponse(const ResultType& response) {
  if (!response.ok()) {
    return zx::error(response.status());
  }
  const auto& res = response.value();
  if (res.is_error()) {
    return MapError(res.error_value()).take_error();
  }
  return zx::ok();
}

}  // namespace internal

struct LittleEndian;
struct BigEndian;

// Convert host to spmi byte order.
template <typename T, class ByteOrder>
static T ConvertToSpmiByteOrder(T value) {
  if (std::is_same<ByteOrder, BigEndian>::value) {
    // Big Endian
    return internal::HostToBigEndian(value);
  } else {
    // Little Endian / default (for 1 byte)
    return internal::HostToLittleEndian(value);
  }
}

// Convert from spmi byte order to host.
template <typename T, class ByteOrder>
static T ConvertFromSpmiByteOrder(T value) {
  if (std::is_same<ByteOrder, BigEndian>::value) {
    // Big Endian
    return internal::BigEndianToHost(value);
  } else {
    // Little Endian / default (for 1 byte)
    return internal::LittleEndianToHost(value);
  }
}

namespace internal {

template <typename IntType, typename SpmiByteOrder>
IntType UnpackRegisterValue(const uint8_t* data) {
  IntType value;
  memcpy(&value, data, sizeof(value));
  return ConvertFromSpmiByteOrder<IntType, SpmiByteOrder>(value);
}

template <typename IntType, typename SpmiByteOrder>
void PackRegisterValue(IntType value, uint8_t* buffer) {
  value = ConvertToSpmiByteOrder<IntType, SpmiByteOrder>(value);
  memcpy(buffer, &value, sizeof(value));
}

}  // namespace internal

// An instance of SpmiRegisterBase represents a staging copy of a register,
// which can be written to the device's register using SPMI protocol. It knows the register's
// address and stores a value for the register. The actual write/read is done upon calling
// ReadFrom()/WriteTo() methods.
//
// All usage rules of RegisterBase applies here with the following exceptions
// - ReadFrom()/WriteTo() methods will return zx_status_t instead of the class object and hence
//   cannot be used in method chaining.
template <class DerivedType, class IntType, class ByteOrder = void, class PrinterState = void>
class SpmiRegisterBase : public RegisterBase<DerivedType, IntType, PrinterState> {
  static_assert(std::is_same<ByteOrder, void>::value ||
                    std::is_same<ByteOrder, LittleEndian>::value ||
                    std::is_same<ByteOrder, BigEndian>::value,
                "unsupported byte order");
  // Byte order must be specified if register address/value is more than one byte
  static_assert(!((sizeof(IntType) > 1) && std::is_same<ByteOrder, void>::value),
                "Byte order must be specified");

 public:
  using SpmiByteOrder = ByteOrder;
  // Delete base class ReadFrom() and WriteTo() methods and define new ones
  // which return zx_status_t in case of SPMI read/write failure
  template <typename T>
  DerivedType& ReadFrom(T* reg_io) = delete;
  template <typename T>
  DerivedType& WriteTo(T* mmio) = delete;

  using RegisterBaseType = RegisterBase<DerivedType, IntType, PrinterState>;

  zx::result<DerivedType> ReadFrom(const fidl::ClientEnd<fuchsia_hardware_spmi::Device>& client) {
    return ReadFrom(client.borrow());
  }

  zx::result<DerivedType> ReadFrom(
      const fidl::UnownedClientEnd<fuchsia_hardware_spmi::Device>& client) {
    auto response =
        fidl::WireCall(client)->RegisterRead(RegisterBaseType::reg_addr(), sizeof(IntType));
    auto data = internal::ParseReadResponse(response, sizeof(IntType));
    if (data.is_error()) {
      return data.take_error();
    }
    return zx::ok(RegisterBaseType::set_reg_value(
        internal::UnpackRegisterValue<IntType, SpmiByteOrder>(data.value())));
  }

  zx::result<> WriteTo(const fidl::ClientEnd<fuchsia_hardware_spmi::Device>& client) {
    return WriteTo(client.borrow());
  }

  zx::result<> WriteTo(const fidl::UnownedClientEnd<fuchsia_hardware_spmi::Device>& client) {
    uint8_t buffer[sizeof(IntType)];
    internal::PackRegisterValue<IntType, SpmiByteOrder>(RegisterBaseType::reg_value(), buffer);
    return internal::ParseWriteResponse(fidl::WireCall(client)->RegisterWrite(
        RegisterBaseType::reg_addr(),
        fidl::VectorView<uint8_t>::FromExternal(buffer, sizeof(IntType))));
  }

  void ReadFrom(fidl::WireClient<fuchsia_hardware_spmi::Device>& client,
                fit::callback<void(zx::result<DerivedType>)> callback) {
    uint32_t addr = RegisterBaseType::reg_addr();
    client->RegisterRead(addr, sizeof(IntType))
        .Then([addr, callback = std::move(callback)](
                  fidl::WireUnownedResult<fuchsia_hardware_spmi::Device::RegisterRead>&
                      response) mutable {
          auto data = internal::ParseReadResponse(response, sizeof(IntType));
          if (data.is_error()) {
            callback(data.take_error());
            return;
          }
          DerivedType reg;
          reg.set_reg_addr(addr);
          reg.set_reg_value(internal::UnpackRegisterValue<IntType, SpmiByteOrder>(data.value()));
          callback(zx::ok(std::move(reg)));
        });
  }

  void WriteTo(fidl::WireClient<fuchsia_hardware_spmi::Device>& client,
               fit::callback<void(zx::result<>)> callback) {
    uint8_t buffer[sizeof(IntType)];
    internal::PackRegisterValue<IntType, SpmiByteOrder>(RegisterBaseType::reg_value(), buffer);
    client
        ->RegisterWrite(RegisterBaseType::reg_addr(),
                        fidl::VectorView<uint8_t>::FromExternal(buffer, sizeof(IntType)))
        .Then([callback = std::move(callback)](
                  fidl::WireUnownedResult<fuchsia_hardware_spmi::Device::RegisterWrite>&
                      response) mutable { callback(internal::ParseWriteResponse(response)); });
  }
};

// An instance of SpmiRegisterAddr represents a typed register address: It
// holds the address of the register in the SPMI device and
// the type of its contents, RegType.  RegType represents the register's
// bitfields.  RegType should be a subclass of SpmiRegisterBase.
//
// All usage rules of RegisterAddr applies here with the following exceptions
// - FromValue() is the only valid method for creating RegType.
// - reg_addr is stored in uint32_t and will be casted to the exact length in SpmiRegisterBase.
template <class RegType>
class SpmiRegisterAddr : public RegisterAddr<RegType> {
 public:
  static_assert(
      std::is_base_of<
          SpmiRegisterBase<RegType, typename RegType::ValueType, typename RegType::SpmiByteOrder>,
          RegType>::value ||
          std::is_base_of<SpmiRegisterBase<RegType, typename RegType::ValueType,
                                           typename RegType::SpmiByteOrder, EnablePrinter>,
                          RegType>::value,
      "Parameter of SpmiRegisterAddr<> should derive from SpmiRegisterAddr");

  // Delete base class ReadFrom() as read from SPMI can fail and
  // cannot be used for constructing RegType.
  template <typename T>
  RegType ReadFrom(T* reg_io) = delete;

  zx::result<RegType> ReadFrom(const fidl::ClientEnd<fuchsia_hardware_spmi::Device>& client) {
    return ReadFrom(client.borrow());
  }

  zx::result<RegType> ReadFrom(
      const fidl::UnownedClientEnd<fuchsia_hardware_spmi::Device>& client) {
    RegType reg;
    reg.set_reg_addr(RegisterAddr<RegType>::addr());
    return reg.ReadFrom(client);
  }

  void ReadFrom(fidl::WireClient<fuchsia_hardware_spmi::Device>& client,
                fit::callback<void(zx::result<RegType>)> callback) {
    RegType reg;
    reg.set_reg_addr(RegisterAddr<RegType>::addr());
    reg.ReadFrom(client, std::move(callback));
  }

  SpmiRegisterAddr(uint32_t reg_addr) : RegisterAddr<RegType>(reg_addr) {}
};

// Helper for consolidating contiguous reads/writes to SPMI registers.
class SpmiRegisterArray {
 public:
  SpmiRegisterArray(uint32_t base_address, size_t size) : base_address_(base_address) {
    regs_.resize(size);
  }

  zx::result<SpmiRegisterArray> ReadFrom(
      const fidl::ClientEnd<fuchsia_hardware_spmi::Device>& client) {
    return ReadFrom(client.borrow());
  }

  zx::result<SpmiRegisterArray> ReadFrom(
      const fidl::UnownedClientEnd<fuchsia_hardware_spmi::Device>& client) {
    auto response =
        fidl::WireCall(client)->RegisterRead(base_address_, static_cast<uint32_t>(regs_.size()));
    auto data = internal::ParseReadResponse(response, regs_.size());
    if (data.is_error()) {
      return data.take_error();
    }
    memcpy(regs_.data(), data.value(), regs_.size());
    return zx::ok(*this);
  }

  void ReadFrom(fidl::WireClient<fuchsia_hardware_spmi::Device>& client,
                fit::callback<void(zx::result<SpmiRegisterArray>)> callback) {
    const uint32_t base_address = base_address_;
    const size_t size = regs_.size();
    client->RegisterRead(base_address, static_cast<uint32_t>(size))
        .Then([base_address, size, callback = std::move(callback)](
                  fidl::WireUnownedResult<fuchsia_hardware_spmi::Device::RegisterRead>&
                      response) mutable {
          auto data = internal::ParseReadResponse(response, size);
          if (data.is_error()) {
            callback(data.take_error());
            return;
          }
          SpmiRegisterArray result(base_address, size);
          memcpy(result.regs().data(), data.value(), size);
          callback(zx::ok(std::move(result)));
        });
  }

  zx::result<> WriteTo(const fidl::ClientEnd<fuchsia_hardware_spmi::Device>& client) {
    return WriteTo(client.borrow());
  }

  zx::result<> WriteTo(const fidl::UnownedClientEnd<fuchsia_hardware_spmi::Device>& client) {
    return internal::ParseWriteResponse(fidl::WireCall(client)->RegisterWrite(
        base_address_, fidl::VectorView<uint8_t>::FromExternal(regs_)));
  }

  void WriteTo(fidl::WireClient<fuchsia_hardware_spmi::Device>& client,
               fit::callback<void(zx::result<>)> callback) {
    client->RegisterWrite(base_address_, fidl::VectorView<uint8_t>::FromExternal(regs_))
        .Then([callback = std::move(callback)](
                  fidl::WireUnownedResult<fuchsia_hardware_spmi::Device::RegisterWrite>&
                      response) mutable { callback(internal::ParseWriteResponse(response)); });
  }

  // `regs()` accesses register values of contiguous addresses from `base_address_` in
  // SpmiByteOrder. Callers of this function should use `ConvertTo/FromSpmiByteOrder` to get the
  // correct endianness for host. Callers should also pay attention to alignment issues. If needed,
  // memcpy to/from a new buffer.
  std::vector<uint8_t>& regs() { return regs_; }

 private:
  uint32_t base_address_;
  std::vector<uint8_t> regs_;
};

}  // namespace hwreg

#endif  // SRC_DEVICES_SPMI_LIB_HWREG_SPMI_CPP_INCLUDE_HWREG_SPMI_H_
