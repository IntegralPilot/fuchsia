// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_GPIO_DRIVERS_AML_GPIO_AML_GPIO_H_
#define SRC_DEVICES_GPIO_DRIVERS_AML_GPIO_AML_GPIO_H_

#include <fidl/fuchsia.hardware.pinimpl/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/fidl.h>
#include <fidl/fuchsia.scheduler/cpp/fidl.h>
#include <lib/async/cpp/executor.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/metadata/cpp/metadata_server.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/fpromise/promise.h>
#include <lib/stdcompat/span.h>

#include <array>
#include <cstdint>
#include <optional>
#include <set>

#include <fbl/array.h>

namespace gpio {

struct AmlGpioBlock {
  uint32_t start_pin;
  uint32_t pin_block;
  uint32_t pin_count;
  uint32_t mux_offset;
  uint32_t oen_offset;
  uint32_t input_offset;
  uint32_t output_offset;
  uint32_t output_shift;  // Used for GPIOAO block
  uint32_t pull_offset;
  uint32_t pull_en_offset;
  uint32_t mmio_index;
  uint32_t pin_start;
  uint32_t ds_offset;
};

struct AmlGpioInterrupt {
  uint32_t pin_select_offset;
  uint32_t edge_polarity_offset;
  uint32_t filter_select_offset;
};

// TODO(42082459): Rename to AmlPin now that it implements the pinimpl protocol.
class AmlGpio : public fdf::WireServer<fuchsia_hardware_pinimpl::PinImpl> {
 public:
  struct InterruptInfo {
    uint16_t pin = kMaxGpioIndex + 1;
    zx::interrupt interrupt;
  };

  AmlGpio(fidl::ClientEnd<fuchsia_hardware_platform_device::Device> pdev, fdf::MmioBuffer mmio_gpio,
          fdf::MmioBuffer mmio_gpio_ao, fdf::MmioBuffer mmio_interrupt,
          cpp20::span<const AmlGpioBlock> gpio_blocks, const AmlGpioInterrupt* gpio_interrupt,
          uint32_t pid, fbl::Array<InterruptInfo> irq_info)
      : pdev_(std::move(pdev), fdf::Dispatcher::GetCurrent()->async_dispatcher()),
        mmios_{std::move(mmio_gpio), std::move(mmio_gpio_ao)},
        mmio_interrupt_(std::move(mmio_interrupt)),
        gpio_blocks_(gpio_blocks),
        gpio_interrupt_(gpio_interrupt),
        pid_(pid),
        irq_info_(std::move(irq_info)) {}

 private:
  friend class AmlGpioDriver;

  static constexpr int kMaxGpioIndex = 255;

  fidl::ProtocolHandler<fuchsia_hardware_pinimpl::PinImpl> CreateHandler() {
    return bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->get(),
                                   fidl::kIgnoreBindingClosure);
  }

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_pinimpl::PinImpl> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    fdf::error("Unexpected pinimpl FIDL call: 0x{:x}", metadata.method_ordinal);
  }

  void Read(fuchsia_hardware_pinimpl::wire::PinImplReadRequest* request, fdf::Arena& arena,
            ReadCompleter::Sync& completer) override;
  void SetBufferMode(fuchsia_hardware_pinimpl::wire::PinImplSetBufferModeRequest* request,
                     fdf::Arena& arena, SetBufferModeCompleter::Sync& completer) override;
  void GetInterrupt(fuchsia_hardware_pinimpl::wire::PinImplGetInterruptRequest* request,
                    fdf::Arena& arena, GetInterruptCompleter::Sync& completer) override;
  void ConfigureInterrupt(fuchsia_hardware_pinimpl::wire::PinImplConfigureInterruptRequest* request,
                          fdf::Arena& arena, ConfigureInterruptCompleter::Sync& completer) override;
  void ReleaseInterrupt(fuchsia_hardware_pinimpl::wire::PinImplReleaseInterruptRequest* request,
                        fdf::Arena& arena, ReleaseInterruptCompleter::Sync& completer) override;
  void Configure(fuchsia_hardware_pinimpl::wire::PinImplConfigureRequest* request,
                 fdf::Arena& arena, ConfigureCompleter::Sync& completer) override;

  fuchsia_hardware_pin::Pull GetPull(const AmlGpioBlock* block, uint32_t pinindex);
  void SetPull(const AmlGpioBlock* block, uint32_t pinindex, fuchsia_hardware_pin::Pull pull);

  uint64_t GetFunction(uint32_t index, const AmlGpioBlock* block);
  void SetFunction(uint32_t index, const AmlGpioBlock* block, uint64_t function);

  uint64_t GetDriveStrength(uint32_t index, const AmlGpioBlock* block);
  void SetDriveStrength(uint32_t index, const AmlGpioBlock* block, uint64_t drive_strength_ua);

  uint32_t GetUnusedIrqIndex(uint32_t pin) const;
  void SetIrqIndex(uint32_t pin, uint8_t index);
  void ClearIrqIndex(uint32_t pin, uint8_t index);

  zx_status_t AmlPinToBlock(uint32_t pin, const AmlGpioBlock** out_block,
                            uint32_t* out_pin_index) const;

  void SetInterruptMode(uint32_t irq_index, fuchsia_hardware_gpio::InterruptMode mode);

  fidl::WireClient<fuchsia_hardware_platform_device::Device> pdev_;
  std::array<fdf::MmioBuffer, 2> mmios_;  // separate MMIO for AO domain
  fdf::MmioBuffer mmio_interrupt_;
  const cpp20::span<const AmlGpioBlock> gpio_blocks_;
  const AmlGpioInterrupt* gpio_interrupt_;
  const uint32_t pid_;
  fbl::Array<InterruptInfo> irq_info_;
  uint8_t irq_status_{};
  uint8_t wake_irq_status_{};
  std::array<std::optional<fuchsia_hardware_gpio::InterruptMode>, kMaxGpioIndex + 1> pin_irq_modes_;
  std::set<uint32_t> wake_vector_pins_;
  fdf::ServerBindingGroup<fuchsia_hardware_pinimpl::PinImpl> bindings_;
};

class AmlGpioDriver : public fdf::DriverBase2 {
 public:
  explicit AmlGpioDriver() : fdf::DriverBase2("aml-gpio") {}

  void Start(fdf::DriverContext context, fdf::StartCompleter completer) override;

 protected:
  const std::shared_ptr<fdf::Namespace>& incoming() const { return incoming_; }

 protected:
  // MapMmio can be overridden by a test in order to provide an fdf::MmioBuffer backed by a fake.
  virtual fpromise::promise<fdf::MmioBuffer, zx_status_t> MapMmio(
      fidl::WireClient<fuchsia_hardware_platform_device::Device>& pdev, uint32_t mmio_id);

 private:
  fpromise::promise<void, zx_status_t> InitResources();
  void OnGetNodeDeviceInfo(const fuchsia_hardware_platform_device::wire::NodeDeviceInfo& info,
                           fpromise::completer<void, zx_status_t> completer);
  void OnGetBoardInfo(const fuchsia_hardware_platform_device::wire::BoardInfo& board_info,
                      uint32_t irq_count, fpromise::completer<void, zx_status_t> completer);
  void MapMmios(uint32_t pid, uint32_t irq_count, fpromise::completer<void, zx_status_t> completer);
  void InitDevice(uint32_t pid, uint32_t irq_count, std::vector<fdf::MmioBuffer> mmios,
                  fpromise::completer<void, zx_status_t> completer);
  zx::result<> AddNode();

  std::shared_ptr<fdf::Namespace> incoming_;

  fidl::WireClient<fuchsia_driver_framework::NodeController> controller_;
  fidl::WireClient<fuchsia_hardware_platform_device::Device> pdev_;
  std::unique_ptr<AmlGpio> device_;
  std::optional<async::Executor> executor_;
  fdf_metadata::MetadataServer<fuchsia_hardware_pinimpl::Metadata> pin_metadata_server_;
  fdf_metadata::MetadataServer<fuchsia_scheduler::RoleName> scheduler_role_name_metadata_server_;
};

}  // namespace gpio

#endif  // SRC_DEVICES_GPIO_DRIVERS_AML_GPIO_AML_GPIO_H_
