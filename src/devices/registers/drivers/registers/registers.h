// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_REGISTERS_DRIVERS_REGISTERS_REGISTERS_H_
#define SRC_DEVICES_REGISTERS_DRIVERS_REGISTERS_REGISTERS_H_

#include <fidl/fuchsia.hardware.registers/cpp/fidl.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <zircon/types.h>

#include <list>
#include <map>

#include <fbl/auto_lock.h>

namespace registers {

#define SWITCH_BY_TAG(TAG, FUNC, ARGS...)               \
  [&]() {                                               \
    switch (TAG) {                                      \
      case fuchsia_hardware_registers::Mask::Tag::kR8:  \
        return FUNC<uint8_t>(ARGS);                     \
      case fuchsia_hardware_registers::Mask::Tag::kR16: \
        return FUNC<uint16_t>(ARGS);                    \
      case fuchsia_hardware_registers::Mask::Tag::kR32: \
        return FUNC<uint32_t>(ARGS);                    \
      case fuchsia_hardware_registers::Mask::Tag::kR64: \
        return FUNC<uint64_t>(ARGS);                    \
    }                                                   \
  }()

template <typename T>
size_t GetSize() {
  return sizeof(T);
}

struct MmioInfo {
  fdf::MmioBuffer mmio_;
  std::vector<fbl::Mutex> locks_;

  template <typename T>
  static zx::result<MmioInfo> Create(fdf::MmioBuffer mmio) {
    auto register_count = mmio.get_size() / sizeof(T);
    if (mmio.get_size() % sizeof(T)) {
      fdf::error("MMIO size does not cover full registers");
      return zx::error(ZX_ERR_INTERNAL);
    }

    std::vector<fbl::Mutex> locks(register_count);
    return zx::ok(MmioInfo{
        .mmio_ = std::move(mmio),
        .locks_ = std::move(locks),
    });
  }
};

class RegistersDevice;

template <typename T>
class Register : public fidl::WireServer<fuchsia_hardware_registers::Device> {
 public:
  explicit Register(std::shared_ptr<MmioInfo> mmio, std::string id,
                    std::map<uint64_t, std::pair<T, uint32_t>> masks)
      : mmio_(std::move(mmio)), id_(std::move(id)), masks_(std::move(masks)) {}
  ~Register() override = default;

  auto GetHandler() {
    return fuchsia_hardware_registers::Service::InstanceHandler({
        .device = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                          fidl::kIgnoreBindingClosure),
    });
  }
  const std::string& id() { return id_; }

 private:
  friend class RegistersDevice;

  void ReadRegister8(ReadRegister8RequestView request,
                     ReadRegister8Completer::Sync& completer) override {
    ReadRegister(request->offset, request->mask, completer);
  }
  void ReadRegister16(ReadRegister16RequestView request,
                      ReadRegister16Completer::Sync& completer) override {
    ReadRegister(request->offset, request->mask, completer);
  }
  void ReadRegister32(ReadRegister32RequestView request,
                      ReadRegister32Completer::Sync& completer) override {
    ReadRegister(request->offset, request->mask, completer);
  }
  void ReadRegister64(ReadRegister64RequestView request,
                      ReadRegister64Completer::Sync& completer) override {
    ReadRegister(request->offset, request->mask, completer);
  }
  void WriteRegister8(WriteRegister8RequestView request,
                      WriteRegister8Completer::Sync& completer) override {
    WriteRegister(request->offset, request->mask, request->value, completer);
  }
  void WriteRegister16(WriteRegister16RequestView request,
                       WriteRegister16Completer::Sync& completer) override {
    WriteRegister(request->offset, request->mask, request->value, completer);
  }
  void WriteRegister32(WriteRegister32RequestView request,
                       WriteRegister32Completer::Sync& completer) override {
    WriteRegister(request->offset, request->mask, request->value, completer);
  }
  void WriteRegister64(WriteRegister64RequestView request,
                       WriteRegister64Completer::Sync& completer) override {
    WriteRegister(request->offset, request->mask, request->value, completer);
  }

  template <typename Ty, typename Completer>
  void ReadRegister(uint64_t offset, Ty mask, Completer& completer);
  template <typename Ty, typename Completer>
  void WriteRegister(uint64_t offset, Ty mask, Ty value, Completer& completer);

  zx::result<> ReadRegister(uint64_t offset, T mask, T* out_value);
  zx::result<> WriteRegister(uint64_t offset, T mask, T value);

  bool VerifyMask(T mask, uint64_t register_offset);

  std::shared_ptr<MmioInfo> mmio_;
  const std::string id_;
  const std::map<uint64_t, std::pair<T, uint32_t>> masks_;  // base_address to (mask, reg_count)

  compat::SyncInitializedDeviceServer compat_server_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;
  fidl::ServerBindingGroup<fuchsia_hardware_registers::Device> bindings_;
};

template <typename T>
template <typename Ty, typename Completer>
void Register<T>::ReadRegister(uint64_t offset, Ty mask, Completer& completer) {
  if constexpr (!std::is_same_v<T, Ty>) {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
    return;
  }
  T val;
  // Need cast to compile
  auto result = ReadRegister(offset, static_cast<T>(mask), &val);
  if (result.is_ok()) {
    // Need cast to compile
    completer.ReplySuccess(static_cast<Ty>(val));
  } else {
    completer.ReplyError(result.error_value());
  }
}

template <typename T>
template <typename Ty, typename Completer>
void Register<T>::WriteRegister(uint64_t offset, Ty mask, Ty value, Completer& completer) {
  if constexpr (!std::is_same_v<T, Ty>) {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
    return;
  }
  // Need cast to compile
  auto result = WriteRegister(offset, static_cast<T>(mask), static_cast<T>(value));
  if (result.is_ok()) {
    completer.ReplySuccess();
  } else {
    completer.ReplyError(result.error_value());
  }
}

template <typename T>
zx::result<> Register<T>::ReadRegister(uint64_t offset, T mask, T* out_value) {
  if (!VerifyMask(mask, offset)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  fbl::AutoLock lock(&mmio_->locks_[offset / sizeof(T)]);
  *out_value = mmio_->mmio_.ReadMasked(mask, offset);
  return zx::ok();
}

template <typename T>
zx::result<> Register<T>::WriteRegister(uint64_t offset, T mask, T value) {
  if (!VerifyMask(mask, offset)) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  fbl::AutoLock lock(&mmio_->locks_[offset / sizeof(T)]);
  mmio_->mmio_.ModifyBits(value, mask, offset);
  return zx::ok();
}

// Returns: true if mask requested is covered by allowed mask.
//          false if mask requested is not covered by allowed mask or mask is not found.
template <typename T>
bool Register<T>::VerifyMask(T mask, const uint64_t offset) {
  auto it = masks_.upper_bound(offset);
  if (offset % sizeof(T)) {
    fdf::error("VerifyMask failed for '{}': offset 0x{:x} is not aligned to size {}", id_, offset,
               sizeof(T));
    return false;
  }
  if (it == masks_.begin()) {
    fdf::error("VerifyMask failed for '{}': offset 0x{:x} is before first allowed register", id_,
               offset);
    return false;
  }
  it--;

  auto base_address = it->first;
  auto reg_mask = it->second.first;
  auto reg_count = it->second.second;

  if ((offset - base_address) / sizeof(T) >= reg_count) {
    fdf::error("VerifyMask failed for '{}': offset 0x{:x} is out of range for base 0x{:x} count {}",
               id_, offset, base_address, reg_count);
    return false;
  }

  if ((mask | reg_mask) != reg_mask) {
    fdf::error(
        "VerifyMask failed for '{}': requested mask 0x{:x} is not covered by allowed mask 0x{:x} at offset 0x{:x}",
        id_, mask, reg_mask, offset);
    return false;
  }

  return true;
}

class RegistersDevice : public fdf::DriverBase2 {
 private:
  static constexpr char kDeviceName[] = "registers-device";

 public:
  RegistersDevice() : fdf::DriverBase2(kDeviceName) {}

  zx::result<> Start(fdf::DriverContext context) override;

 private:
  friend class TestRegistersDevice;

  // Virtual for testing.
  virtual zx::result<> MapMmio(fuchsia_hardware_registers::Mask::Tag& tag);

  template <typename T>
  zx::result<> Create(const fuchsia_hardware_registers::RegistersMetadataEntry& reg);

  template <typename T>
  zx::result<> CreateNode(Register<T>& reg);

  using RegisterType =
      std::variant<Register<uint8_t>, Register<uint16_t>, Register<uint32_t>, Register<uint64_t>>;
  std::list<RegisterType> registers_;
  std::map<uint32_t, std::shared_ptr<MmioInfo>> mmios_;  // MMIO ID to MmioInfo

  std::shared_ptr<fdf::Namespace> incoming_;
  std::string node_name_;
};

}  // namespace registers

#endif  // SRC_DEVICES_REGISTERS_DRIVERS_REGISTERS_REGISTERS_H_
