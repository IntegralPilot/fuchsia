// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_SDMMC_SDIO_CONTROLLER_DEVICE_H_
#define SRC_DEVICES_BLOCK_DRIVERS_SDMMC_SDIO_CONTROLLER_DEVICE_H_

#include <fidl/fuchsia.hardware.sdmmc/cpp/wire.h>
#include <fuchsia/hardware/sdio/c/banjo.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/power/cpp/suspend.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/sync/completion.h>
#include <lib/sync/cpp/completion.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <lib/zx/result.h>

#include <array>
#include <atomic>
#include <bitset>
#include <memory>
#include <mutex>

#include <fbl/algorithm.h>
#include <fbl/array.h>

#include "sdio-function-device.h"
#include "sdmmc-device.h"

namespace sdmmc {

class SdmmcRootDevice;

class SdioControllerDevice : public fdf::WireServer<fuchsia_hardware_sdmmc::InBandInterrupt> {
 public:
  static constexpr char kDeviceName[] = "sdmmc-sdio";

  template <typename T>
  struct SdioRwTxn {
    uint32_t addr;
    bool incr;
    bool write;
    cpp20::span<const T> buffers;
  };

  SdioControllerDevice(SdmmcRootDevice* parent, std::unique_ptr<SdmmcDevice> sdmmc)
      : parent_(parent), sdmmc_(std::move(sdmmc)) {
    for (size_t i = 0; i < funcs_.size(); i++) {
      funcs_[i] = {};
    }
  }

  static zx_status_t Create(SdmmcRootDevice* parent, std::unique_ptr<SdmmcDevice> sdmmc,
                            std::unique_ptr<SdioControllerDevice>* out_dev);
  // Returns the SdmmcDevice. Used if this SdioControllerDevice fails to probe (i.e., no eligible
  // device present).
  std::unique_ptr<SdmmcDevice> TakeSdmmcDevice() { return std::move(sdmmc_); }

  void Suspend(fdf_power::SuspendCompleter completer) { completer(); }
  void Resume(fdf_power::ResumeCompleter completer) { completer(); }
  bool SuspendEnabled() { return false; }

  zx_status_t Probe(const fuchsia_hardware_sdmmc::SdmmcMetadata& metadata);
  zx_status_t AddDevice();

  zx_status_t SdioGetDevHwInfo(uint8_t fn_idx, sdio_hw_info_t* out_hw_info) TA_EXCL(lock_);
  zx_status_t SdioEnableFn(uint8_t fn_idx) TA_EXCL(lock_);
  zx_status_t SdioDisableFn(uint8_t fn_idx) TA_EXCL(lock_);
  zx_status_t SdioIoReady(uint8_t fn_idx, bool* out_ready) TA_EXCL(lock_);
  zx_status_t SdioEnableFnIntr(uint8_t fn_idx) TA_EXCL(lock_);
  zx_status_t SdioDisableFnIntr(uint8_t fn_idx) TA_EXCL(lock_);
  zx_status_t SdioUpdateBlockSize(uint8_t fn_idx, uint16_t blk_sz, bool deflt) TA_EXCL(lock_);
  zx_status_t SdioGetBlockSize(uint8_t fn_idx, uint16_t* out_cur_blk_size);
  zx_status_t SdioDoRwByte(bool write, uint8_t fn_idx, uint32_t addr, uint8_t write_byte,
                           uint8_t* out_read_byte) TA_EXCL(lock_);
  zx::result<uint8_t> SdioReadByte(uint8_t function, uint32_t address, bool suppress_error_messages)
      TA_EXCL(lock_);
  zx::result<uint8_t> SdioWriteByte(uint8_t function, uint32_t address, uint8_t byte,
                                    bool read_after_write, bool suppress_error_messages)
      TA_EXCL(lock_);
  zx_status_t SdioGetInBandIntr(uint8_t fn_idx, zx::interrupt* out_irq);
  void SdioAckInBandIntr(uint8_t fn_idx);
  zx_status_t SdioIoAbort(uint8_t fn_idx) TA_EXCL(lock_);
  zx_status_t SdioIntrPending(uint8_t fn_idx, bool* out_pending);
  zx_status_t SdioDoVendorControlRwByte(bool write, uint8_t addr, uint8_t write_byte,
                                        uint8_t* out_read_byte);
  zx_status_t SdioRegisterVmo(uint8_t fn_idx, uint32_t vmo_id, zx::vmo vmo, uint64_t offset,
                              uint64_t size, uint32_t vmo_rights);
  zx_status_t SdioUnregisterVmo(uint8_t fn_idx, uint32_t vmo_id, zx::vmo* out_vmo);
  // TODO(b/309864701): Remove templating when Banjo support has been dropped.
  template <typename T>
  zx_status_t SdioDoRwTxn(uint8_t fn_idx, const SdioRwTxn<T>& txn);

  zx_status_t SdioRequestCardReset();
  zx_status_t SdioPerformTuning();

  void Callback(fdf::Arena& arena, CallbackCompleter::Sync& completer) override;

  zx_status_t SdioDoRwTxn(uint8_t fn_idx, const sdio_rw_txn_t* txn) {
    return SdioDoRwTxn(fn_idx, SdioRwTxn<sdmmc_buffer_region_t>{
                                   .addr = txn->addr,
                                   .incr = txn->incr,
                                   .write = txn->write,
                                   .buffers = {txn->buffers_list, txn->buffers_count},
                               });
  }

  void FunctionPowerOn(uint8_t fn_idx);
  void FunctionPowerOff(uint8_t fn_idx);

  // Called by children of this device.
  fidl::WireSyncClient<fuchsia_driver_framework::Node>& sdio_controller_node() {
    return sdio_controller_node_;
  }
  SdmmcRootDevice* parent() { return parent_; }

  zx_status_t StartSdioIrqDispatcherIfNeeded() TA_EXCL(irq_dispatcher_lock_);
  void StopSdioIrqDispatcher(std::optional<fdf::StopCompleter> completer = std::nullopt)
      TA_EXCL(irq_dispatcher_lock_);

  fdf::Logger& logger();

 private:
  struct SdioFuncTuple {
    uint8_t tuple_code;
    uint8_t tuple_body_size;
    uint8_t tuple_body[UINT8_MAX];
  };

  // SDIO cards support one common function and up to seven I/O functions. This struct is used to
  // keep track of each function's state as they can be configured independently.
  struct SdioFunction {
    sdio_func_hw_info_t hw_info;
    uint16_t cur_blk_size;
    bool enabled;
    bool intr_enabled;
  };

  zx_status_t ProbeLocked() TA_REQ(lock_);

  zx_status_t SdioReset() TA_REQ(lock_);
  // Reads the card common control registers (CCCR) to enumerate the card's capabilities.
  zx_status_t ProcessCccr() TA_REQ(lock_);
  // Reads the card information structure (CIS) for the given function to get the manufacturer
  // identification and function extensions tuples.
  zx_status_t ProcessCis(uint8_t fn_idx) TA_REQ(lock_);
  // Parses a tuple read from the CIS.
  zx_status_t ParseFnTuple(uint8_t fn_idx, const SdioFuncTuple& tup) TA_REQ(lock_);
  // Parses the manufacturer ID tuple and saves it in the given function's struct.
  zx_status_t ParseMfidTuple(uint8_t fn_idx, const SdioFuncTuple& tup) TA_REQ(lock_);
  // Parses the function extensions tuple and saves it in the given function's struct.
  zx_status_t ParseFuncExtTuple(uint8_t fn_idx, const SdioFuncTuple& tup) TA_REQ(lock_);
  // Reads the I/O function code and saves it in the given function's struct.
  zx_status_t ProcessFbr(uint8_t fn_idx) TA_REQ(lock_);
  // Popluates the given function's struct by calling the methods above. Also enables the
  // function and sets its default block size.
  zx_status_t InitFunc(uint8_t fn_idx) TA_REQ(lock_);

  zx_status_t SwitchFreq(uint32_t new_freq) TA_REQ(lock_);
  zx_status_t TrySwitchHs() TA_REQ(lock_);
  zx_status_t TrySwitchUhs() TA_REQ(lock_);
  zx_status_t Enable4BitBus() TA_REQ(lock_);
  zx_status_t SwitchBusWidth(uint32_t bw) TA_REQ(lock_);

  zx_status_t ReadData16(uint8_t fn_idx, uint32_t addr, uint16_t* word) TA_REQ(lock_);
  zx_status_t WriteData16(uint8_t fn_idx, uint32_t addr, uint16_t word) TA_REQ(lock_);

  zx_status_t SdioEnableFnLocked(uint8_t fn_idx) TA_REQ(lock_);
  zx_status_t SdioUpdateBlockSizeLocked(uint8_t fn_idx, uint16_t blk_sz, bool deflt) TA_REQ(lock_);
  zx_status_t SdioDoRwByteLocked(bool write, uint8_t fn_idx, uint32_t addr, uint8_t write_byte,
                                 uint8_t* out_read_byte) TA_REQ(lock_);

  zx::result<uint8_t> ReadCccrByte(uint32_t addr) TA_REQ(lock_);

  zx_status_t PowerOnReset() TA_EXCL(lock_);

  template <typename T>
  struct SdioTxnPosition {
    cpp20::span<const T> buffers;  // The buffers remaining to be processed.
    uint64_t first_buffer_offset;  // The offset into the first buffer.
    uint32_t address;              // The current SDIO address, fixed if txn.incr is false.
  };

  // Returns an SdioTxnPosition representing the new position in the buffers list.
  template <typename T>
  zx::result<SdioTxnPosition<T>> DoOneRwTxnRequest(uint8_t fn_idx, const SdioRwTxn<T>&,
                                                   SdioTxnPosition<T> current_position)
      TA_REQ(lock_);

  void SdioIrqHandler();
  uint8_t interrupt_enabled_mask_ TA_GUARDED(lock_) = UINT8_MAX;

  std::mutex irq_dispatcher_lock_;  // Used to make dispatcher creation and shutdown atomic.
  fdf::Dispatcher irq_dispatcher_;
  libsync::Completion irq_shutdown_completion_;

  std::mutex lock_;
  SdmmcRootDevice* const parent_;
  std::unique_ptr<SdmmcDevice> sdmmc_;
  std::atomic<bool> shutdown_ = false;
  std::array<zx::interrupt, SDIO_MAX_FUNCS> sdio_irqs_;
  std::array<SdioFunction, SDIO_MAX_FUNCS> funcs_ TA_GUARDED(lock_);
  sdio_device_hw_info_t hw_info_ = {};
  bool tuned_ = false;
  std::atomic<bool> tuning_in_progress_ = false;
  inspect::Node root_;
  inspect::UintProperty tx_errors_;
  inspect::UintProperty rx_errors_;

  fidl::WireSyncClient<fuchsia_driver_framework::Node> sdio_controller_node_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;

  std::array<std::unique_ptr<SdioFunctionDevice>, SDIO_MAX_FUNCS> child_sdio_function_devices_ = {};

  bool in_band_interrupt_supported_ = true;

  // A set bit means that the corresponding function's power element is in the ON state. Function 0
  // is always considered to be powered on if at least one I/O function is on. Default to all
  // functions on so that probe works.
  std::bitset<SDIO_MAX_FUNCS> function_power_on_{UINT8_MAX};

  // This value comes from metadata. If true, the device must be re-initialized after leaving the
  // OFF state.
  bool vccq_off_with_controller_off_ = false;

  std::optional<fdf::ServerBinding<fuchsia_hardware_sdmmc::InBandInterrupt>>
      in_band_interrupt_binding_;
};

}  // namespace sdmmc

#endif  // SRC_DEVICES_BLOCK_DRIVERS_SDMMC_SDIO_CONTROLLER_DEVICE_H_
