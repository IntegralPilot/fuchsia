// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_SDMMC_FAKE_SDMMC_DEVICE_H_
#define SRC_DEVICES_BLOCK_DRIVERS_SDMMC_FAKE_SDMMC_DEVICE_H_

#include <fidl/fuchsia.hardware.sdmmc/cpp/driver/fidl.h>
#include <fuchsia/hardware/sdio/cpp/banjo.h>
#include <fuchsia/hardware/sdmmc/cpp/banjo.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/fit/function.h>
#include <lib/stdcompat/span.h>

#include <array>
#include <map>
#include <variant>
#include <vector>

#include <ddktl/device.h>

#include "src/lib/vmo_store/vmo_store.h"

namespace sdmmc {

class FakeSdmmcDevice : public ddk::SdmmcProtocol<FakeSdmmcDevice>,
                        public fdf::WireServer<fuchsia_hardware_sdmmc::Sdmmc> {
 public:
  using Command = uint32_t;

  // Convenience types to allow command callbacks with different combinations of arguments and
  // return values.
  using CommandCallbackRequestOnly = fit::function<void(const sdmmc_req_t&)>;
  using CommandCallbackResponseOnly = fit::function<void(uint32_t[4])>;
  using CommandCallbackRequestAndResponse = fit::function<void(const sdmmc_req_t&, uint32_t[4])>;
  using CommandCallbackWithData = fit::function<void(cpp20::span<uint8_t>)>;
  using CommandCallbackWithStatus = fit::function<zx_status_t(const sdmmc_req_t&)>;

  using CommandCallback = std::variant<CommandCallbackRequestOnly, CommandCallbackResponseOnly,
                                       CommandCallbackRequestAndResponse, CommandCallbackWithData,
                                       CommandCallbackWithStatus>;

  static constexpr uint32_t kBadRegionStart = 0x0bad00;
  static constexpr uint32_t kBadRegionMask = 0x0fff00;

  // This is the sector size from the eMMC specification. It is valid for cards over 2GB which we
  // assume all of our supported cards will be.
  static constexpr size_t kBlockSize = 512;
  static constexpr size_t kBlockMask = ~static_cast<size_t>(kBlockSize - 1);

  // This is a placeholder value, not currently enforced.
  static constexpr size_t kBlockCount = 0x100000;

  FakeSdmmcDevice() : proto_{.ops = &sdmmc_protocol_ops_, .ctx = this}, host_info_({}) {
    for (auto& store : registered_vmos_) {
      store.emplace(vmo_store::Options{});
    }
  }

  // For testing using Banjo.
  ddk::SdmmcProtocolClient GetClient() const { return ddk::SdmmcProtocolClient(&proto_); }

  // For testing using FIDL.
  zx::result<fdf::ClientEnd<fuchsia_hardware_sdmmc::Sdmmc>> GetFidlClientEnd() {
    zx::result endpoints = fdf::CreateEndpoints<fuchsia_hardware_sdmmc::Sdmmc>();
    if (endpoints.is_error()) {
      return endpoints.take_error();
    }
    auto server_dispatcher = fdf_testing::DriverRuntime::GetInstance()->StartBackgroundDispatcher();
    fdf::BindServer(server_dispatcher->get(), std::move(endpoints->server), this);
    return zx::ok(std::move(endpoints->client));
  }

  void set_host_info(const sdmmc_host_info_t& host_info) { host_info_ = host_info; }

  const std::map<Command, uint32_t>& command_counts() const { return command_counts_; }
  std::vector<sdmmc_req_t>& requests() { return requests_; }

  void Reset() {
    for (auto& sector : sectors_) {
      sector.clear();
    }

    command_counts_.clear();
    command_callbacks_.clear();
    requests_.clear();
    interrupt_cb_ = {};
    set_signal_voltage_status_ = ZX_OK;
    set_bus_width_status_ = ZX_OK;
    set_bus_freq_status_ = ZX_OK;
    set_timing_status_ = ZX_OK;
    perform_tuning_status_ = ZX_OK;
    signal_voltage_ = SDMMC_VOLTAGE_MAX;
    bus_width_ = SDMMC_BUS_WIDTH_ONE;
    bus_freq_ = 0;
    timing_ = SDMMC_TIMING_MAX;
    erase_group_start_.reset();
    erase_group_end_.reset();

    for (auto& store : registered_vmos_) {
      store.emplace(vmo_store::Options{});
    }
  }

  // ddk::SdmmcProtocol implementation
  zx_status_t SdmmcHostInfo(sdmmc_host_info_t* out_info);

  zx_status_t SdmmcSetSignalVoltage(sdmmc_voltage_t voltage) {
    signal_voltage_ = voltage;
    return set_signal_voltage_status_;
  }
  zx_status_t SdmmcSetBusWidth(sdmmc_bus_width_t bus_width) {
    bus_width_ = bus_width;
    return set_bus_width_status_;
  }
  zx_status_t SdmmcSetBusFreq(uint32_t bus_freq) {
    bus_freq_ = bus_freq;
    return set_bus_freq_status_;
  }
  zx_status_t SdmmcSetTiming(sdmmc_timing_t timing) {
    timing_ = timing;
    return set_timing_status_;
  }
  zx_status_t SdmmcHwReset() { return ZX_OK; }
  zx_status_t SdmmcPerformTuning(uint32_t cmd_idx) { return perform_tuning_status_; }

  zx_status_t SdmmcRegisterVmo(uint32_t vmo_id, uint8_t client_id, zx::vmo vmo, uint64_t offset,
                               uint64_t size, uint32_t vmo_rights);
  zx_status_t SdmmcUnregisterVmo(uint32_t vmo_id, uint8_t client_id, zx::vmo* out_vmo);
  zx_status_t SdmmcRequest(const sdmmc_req_t* req, uint32_t out_response[4]);

  // fuchsia_hardware_sdmmc::Sdmmc implementation
  void HostInfo(fdf::Arena& arena, HostInfoCompleter::Sync& completer) override;
  void SetSignalVoltage(SetSignalVoltageRequestView request, fdf::Arena& arena,
                        SetSignalVoltageCompleter::Sync& completer) override;
  void SetBusWidth(SetBusWidthRequestView request, fdf::Arena& arena,
                   SetBusWidthCompleter::Sync& completer) override;
  void SetBusFreq(SetBusFreqRequestView request, fdf::Arena& arena,
                  SetBusFreqCompleter::Sync& completer) override;
  void SetTiming(SetTimingRequestView request, fdf::Arena& arena,
                 SetTimingCompleter::Sync& completer) override;
  void HwReset(fdf::Arena& arena, HwResetCompleter::Sync& completer) override;
  void PerformTuning(PerformTuningRequestView request, fdf::Arena& arena,
                     PerformTuningCompleter::Sync& completer) override;
  void RegisterInBandInterrupt(RegisterInBandInterruptRequestView request, fdf::Arena& arena,
                               RegisterInBandInterruptCompleter::Sync& completer) override;
  void AckInBandInterrupt(fdf::Arena& arena, AckInBandInterruptCompleter::Sync& completer) override;
  void RegisterVmo(RegisterVmoRequestView request, fdf::Arena& arena,
                   RegisterVmoCompleter::Sync& completer) override;
  void UnregisterVmo(UnregisterVmoRequestView request, fdf::Arena& arena,
                     UnregisterVmoCompleter::Sync& completer) override;
  void Request(RequestRequestView request, fdf::Arena& arena,
               RequestCompleter::Sync& completer) override;

  void EnableCqhci(fdf::Arena& arena, EnableCqhciCompleter::Sync& completer) override;
  void DisableCqhci(fdf::Arena& arena, DisableCqhciCompleter::Sync& completer) override;
  void InitializeCommandQueueing(InitializeCommandQueueingRequestView request, fdf::Arena& arena,
                                 InitializeCommandQueueingCompleter::Sync& completer) override;

  std::vector<uint8_t> Read(size_t address, size_t size, uint8_t func = 0);
  void Write(size_t address, cpp20::span<const uint8_t> data, uint8_t func = 0);
  template <typename T>
  void Write(size_t address, const T& data, uint8_t func = 0) {
    Write(address, cpp20::span<const uint8_t>(data.data(), data.size() * sizeof(data[0])), func);
  }
  void Erase(size_t address, size_t size, uint8_t func = 0);

  zx_status_t TriggerInBandInterrupt() const;

  void set_command_callback(Command command, CommandCallback callback) {
    command_callbacks_[command] = std::move(callback);
  }

  void set_set_signal_voltage_status(zx_status_t status) { set_signal_voltage_status_ = status; }
  void set_set_bus_width_status(zx_status_t status) { set_bus_width_status_ = status; }
  void set_set_bus_freq_status(zx_status_t status) { set_bus_freq_status_ = status; }
  void set_set_timing_status(zx_status_t status) { set_timing_status_ = status; }
  void set_perform_tuning_status(zx_status_t status) { perform_tuning_status_ = status; }
  void set_in_band_interrupt_supported(bool supported) { in_band_interrupt_supported_ = supported; }

  sdmmc_voltage_t signal_voltage() const { return signal_voltage_; }
  sdmmc_bus_width_t bus_width() const { return bus_width_; }
  uint32_t bus_freq() const { return bus_freq_; }
  sdmmc_timing_t timing() const { return timing_; }

 private:
  struct OwnedVmoInfo {
    uint64_t offset;
    uint64_t size;
  };

  using SdmmcVmoStore = vmo_store::VmoStore<vmo_store::HashTableStorage<uint32_t, OwnedVmoInfo>>;

  zx_status_t SdmmcRequestInternal(const sdmmc_req_t& req, uint32_t out_response[4],
                                   cpp20::span<uint8_t> out_data);

  static zx_status_t CopySdmmcRegions(cpp20::span<const sdmmc_buffer_region_t> regions,
                                      SdmmcVmoStore& vmos, uint8_t* buffer, bool copy_to_regions);

  const sdmmc_protocol_t proto_;
  sdmmc_host_info_t host_info_;
  std::array<std::map<size_t, std::unique_ptr<uint8_t[]>>, SDIO_MAX_FUNCS> sectors_;
  std::map<Command, uint32_t> command_counts_;
  std::map<Command, CommandCallback> command_callbacks_;
  std::vector<sdmmc_req_t> requests_;
  fdf::WireSyncClient<fuchsia_hardware_sdmmc::InBandInterrupt> interrupt_cb_;
  zx_status_t set_signal_voltage_status_ = ZX_OK;
  zx_status_t set_bus_width_status_ = ZX_OK;
  zx_status_t set_bus_freq_status_ = ZX_OK;
  zx_status_t set_timing_status_ = ZX_OK;
  zx_status_t perform_tuning_status_ = ZX_OK;
  sdmmc_voltage_t signal_voltage_ = SDMMC_VOLTAGE_MAX;
  sdmmc_bus_width_t bus_width_ = SDMMC_BUS_WIDTH_ONE;
  uint32_t bus_freq_ = 0;
  sdmmc_timing_t timing_ = SDMMC_TIMING_MAX;
  std::optional<uint32_t> erase_group_start_;
  std::optional<uint32_t> erase_group_end_;
  std::optional<SdmmcVmoStore> registered_vmos_[SDMMC_MAX_CLIENT_ID + 1];
  bool in_band_interrupt_supported_ = true;
};

}  // namespace sdmmc

#endif  // SRC_DEVICES_BLOCK_DRIVERS_SDMMC_FAKE_SDMMC_DEVICE_H_
