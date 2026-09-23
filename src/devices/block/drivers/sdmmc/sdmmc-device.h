// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_SDMMC_SDMMC_DEVICE_H_
#define SRC_DEVICES_BLOCK_DRIVERS_SDMMC_SDMMC_DEVICE_H_

#include <fidl/fuchsia.hardware.sdmmc/cpp/driver/fidl.h>
#include <fuchsia/hardware/sdmmc/cpp/banjo.h>
#include <lib/sdmmc/hw.h>
#include <lib/stdcompat/span.h>
#include <lib/zx/eventpair.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>

#include <array>

#include <sdk/lib/driver/logging/cpp/logger.h>

namespace sdmmc {

class SdmmcRootDevice;

// SdmmcDevice wraps a ddk::SdmmcProtocolClient to provide helper methods to the SD/MMC and SDIO
// core drivers. It is assumed that the underlying SDMMC protocol driver can handle calls from
// different threads, although care should be taken when calling methods that update the RCA
// (SdSendRelativeAddr and MmcSetRelativeAddr) or change the signal voltage (SdSwitchUhsVoltage).
// These are typically not used outside the probe thread however, so generally no synchronization is
// required.
class SdmmcDevice {
 public:
  static constexpr uint32_t kTryAttempts = 10;  // 1 initial + 9 retries.

  explicit SdmmcDevice(SdmmcRootDevice* root_device,
                       const fuchsia_hardware_sdmmc::SdmmcMetadata& metadata)
      : root_device_(root_device), max_frequency_(metadata.max_frequency().value()) {}

  // For testing using Banjo.
  explicit SdmmcDevice(SdmmcRootDevice* root_device, const ddk::SdmmcProtocolClient& host)
      : root_device_(root_device), host_(host) {}
  // For testing using FIDL.
  explicit SdmmcDevice(SdmmcRootDevice* root_device,
                       fdf::ClientEnd<fuchsia_hardware_sdmmc::Sdmmc> client_end)
      : root_device_(root_device) {
    client_.Bind(std::move(client_end), fdf::Dispatcher::GetCurrent()->get());
    using_fidl_ = true;
  }

  zx_status_t Init(bool use_fidl);

  bool using_fidl() const { return using_fidl_; }
  const sdmmc_host_info_t& host_info() const { return host_info_; }

  bool UseDma() const { return host_info_.caps & SDMMC_HOST_CAP_DMA; }

  // Update the current voltage field, e.g. after reading the card status registers.
  void SetCurrentVoltage(sdmmc_voltage_t new_voltage) { signal_voltage_ = new_voltage; }

  void SetRequestRetries(uint32_t retries) { retries_ = retries; }

  // SD/MMC shared ops
  zx_status_t SdmmcGoIdle();
  zx_status_t SdmmcSendStatus(uint32_t* status);
  zx_status_t SdmmcStopTransmission(uint32_t* status = nullptr);
  zx_status_t SdmmcWaitForState(uint32_t desired_state);
  // Issues a collection of IO requests. STOP_TRANSMISSION is issued if the request(s) fail.
  // |buffer_region_ptr| is used for FIDL-to-Banjo request translation.
  zx_status_t SdmmcIoRequest(fdf::Arena arena,
                             fidl::VectorView<fuchsia_hardware_sdmmc::wire::SdmmcReq> reqs,
                             sdmmc_buffer_region_t* buffer_region_ptr);

  // SD ops
  zx_status_t SdSendOpCond(uint32_t flags, uint32_t* ocr);
  zx_status_t SdSendIfCond();
  zx_status_t SdSelectCard();
  zx_status_t SdSendScr(std::array<uint8_t, 8>& scr);
  zx_status_t SdSetBusWidth(sdmmc_bus_width_t width);

  // SD/SDIO shared ops
  zx_status_t SdSwitchUhsVoltage(uint32_t ocr);
  zx_status_t SdSendRelativeAddr(uint16_t* card_status);

  // SDIO ops
  zx_status_t SdioSendOpCond(uint32_t ocr, uint32_t* rocr);
  zx_status_t SdioIoRwDirect(bool write, uint32_t fn_idx, uint32_t reg_addr, uint8_t write_byte,
                             uint8_t* read_byte);
  // Read
  zx::result<uint8_t> SdioIoRwDirect(uint32_t function, uint32_t address,
                                     bool suppress_error_messages);
  // Write
  zx::result<uint8_t> SdioIoRwDirect(uint32_t function, uint32_t address, uint8_t byte,
                                     bool read_after_write, bool suppress_error_messages);

  zx_status_t SdioIoRwExtended(uint32_t caps, bool write, uint8_t fn_idx, uint32_t reg_addr,
                               bool incr, uint32_t blk_count, uint32_t blk_size,
                               cpp20::span<const sdmmc_buffer_region_t> buffers);

  // MMC ops
  zx::result<uint32_t> MmcSendOpCond(bool suppress_error_messages);
  zx_status_t MmcWaitForReadyState(uint32_t ocr);
  zx_status_t MmcAllSendCid(std::array<uint8_t, SDMMC_CID_SIZE>& cid);
  zx_status_t MmcSetRelativeAddr(uint16_t rca);
  zx_status_t MmcSendCsd(std::array<uint8_t, SDMMC_CSD_SIZE>& csd);
  zx_status_t MmcSendExtCsd(std::array<uint8_t, MMC_EXT_CSD_SIZE>& ext_csd);
  zx_status_t MmcSleepOrAwake(bool sleep);
  zx_status_t MmcSelectCard(bool select = true);
  zx_status_t MmcSwitch(uint8_t index, uint8_t value);

  // TODO(b/299501583): Migrate these to use FIDL calls.
  // Wraps ddk::SdmmcProtocolClient methods.
  zx_status_t HostInfo(sdmmc_host_info_t* info);
  zx_status_t SetSignalVoltage(sdmmc_voltage_t voltage);
  zx_status_t SetBusWidth(sdmmc_bus_width_t bus_width);
  zx_status_t SetBusFreq(uint32_t bus_freq);
  zx_status_t SetTiming(sdmmc_timing_t timing);
  zx_status_t HwReset();
  zx_status_t PerformTuning(uint32_t cmd_idx);
  zx_status_t RegisterInBandInterrupt(
      fdf::ClientEnd<fuchsia_hardware_sdmmc::InBandInterrupt> client_end);
  void AckInBandInterrupt();
  zx_status_t RegisterVmo(uint32_t vmo_id, uint8_t client_id, zx::vmo vmo, uint64_t offset,
                          uint64_t size, uint32_t vmo_rights);
  zx_status_t UnregisterVmo(uint32_t vmo_id, uint8_t client_id, zx::vmo* out_vmo);
  zx_status_t Request(const sdmmc_req_t* req, uint32_t out_response[4]);

  zx::result<> EnableCqhci();
  zx::result<> DisableCqhci();

  struct InitializeCommandQueueingResources {
    zx::vmo cqhci_mmio;
    zx_off_t cqhci_mmio_offset;
    zx::vmo sdhci_mmio;
    zx_off_t sdhci_mmio_offset;
    zx::bti bti;
    zx::interrupt interrupt;
  };

  zx::result<InitializeCommandQueueingResources> InitializeCommandQueueing(
      zx::interrupt virtual_interrupt, zx::eventpair virtual_interrupt_lifeline);

  uint16_t Rca() const { return rca_; }
  void ClearRca() { rca_ = 0; }

  // Visible for testing.
  zx_status_t RefreshHostInfo() { return HostInfo(&host_info_); }

  fdf::Logger& logger();

 private:
  // Retry each request retries_ times (with wait_time delay in between) by default. Requests are
  // always tried at least once.
  zx_status_t Request(const sdmmc_req_t& req, uint32_t response[4], uint32_t retries = 0,
                      zx::duration wait_time = {});
  zx_status_t RequestWithBlockRead(const sdmmc_req_t& req, uint32_t response[4],
                                   cpp20::span<uint8_t> read_data);
  zx_status_t SdSendAppCmd();
  // In case of IO failure, stop the transmission and wait for the card to go idle before retrying.
  void SdmmcStopForRetry();

  inline uint32_t RcaArg() const { return rca_ << 16; }

  bool using_fidl_ = false;
  SdmmcRootDevice* const root_device_;
  ddk::SdmmcProtocolClient host_;
  // The FIDL client to communicate with Sdmmc device.
  fdf::WireSharedClient<fuchsia_hardware_sdmmc::Sdmmc> client_;

  sdmmc_host_info_t host_info_ = {};
  sdmmc_voltage_t signal_voltage_ = SDMMC_VOLTAGE_V330;
  uint32_t max_frequency_ = UINT32_MAX;
  uint16_t rca_ = 0;  // APP_CMD requires the initial RCA to be zero.
  uint32_t retries_ = 0;
};

}  // namespace sdmmc

#endif  // SRC_DEVICES_BLOCK_DRIVERS_SDMMC_SDMMC_DEVICE_H_
