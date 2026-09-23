// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_DEVICES_NAND_DRIVERS_AML_SPINAND_AML_SPINAND_H_
#define SRC_DEVICES_NAND_DRIVERS_AML_SPINAND_AML_SPINAND_H_

#include <fuchsia/hardware/rawnand/cpp/banjo.h>
#include <lib/driver/compat/cpp/device_server.h>
#include <lib/driver/component/cpp/driver_base2.h>

#include "aml-spifc.h"
#include "flash-chips.h"

namespace amlspinand {

constexpr uint32_t kDefaultSpiNandFreq = 96'000'000;
constexpr uint32_t kDefaultTxBusWidth = 4;
constexpr uint32_t kDefaultRXBusWidth = 4;

constexpr uint32_t kSpinandMaxIdLen = 4;
constexpr uint32_t kSpinandHasQeBit = 1;

class AmlSpiNand : public fdf::DriverBase2, public ddk::RawNandProtocol<AmlSpiNand> {
 public:
  AmlSpiNand();

  virtual ~AmlSpiNand() = default;

  zx::result<> Start(fdf::DriverContext context) override;

  // RawNand protocol implementation.
  zx_status_t RawNandReadPageHwecc(uint32_t nand_page, uint8_t *data, size_t data_size,
                                   size_t *data_actual, uint8_t *oob, size_t oob_size,
                                   size_t *oob_actual, uint32_t *ecc_correct);
  zx_status_t RawNandWritePageHwecc(const uint8_t *data, size_t data_size, const uint8_t *oob,
                                    size_t oob_size, uint32_t nand_page);
  zx_status_t RawNandEraseBlock(uint32_t nand_page);
  zx_status_t RawNandGetNandInfo(nand_info_t *nand_info);

  zx_status_t Bind();
  zx_status_t Init();

 private:
  // platform data for device
  struct PlatData {
    uint32_t max_hz;
    SpiRxMode rx_mode;
    SpiTxMode tx_mode;
  };

  struct SpiNandId {
    uint8_t data[kSpinandMaxIdLen];
    uint32_t len;
  };

  struct SpiNandDev {
    SpiNandId id;
    uint32_t flags;

    uint8_t cfg_cache;
    std::unique_ptr<uint8_t[]> databuf;
    // Points to the region owned by the above databuf(page + oob),
    // No needs to be wrapped in a smart pointer.
    uint8_t *oobbuf;
  };

  enum class OobOps {
    // OOB data are placed at the given offset (default)
    kOpsPlaceOob,
    // OOB data are automatically placed at the free areas which are defined by the internal
    // ecclayout
    kOpsAutoOob,
    // data are transferred as-is, with no error correction
    kOpsRawOob,
  };

  std::optional<FlashChipInfo> DetermineFlashChip();
  uint32_t GetFlashSizeInPages() const;

  // Covert the page index to row address
  uint32_t PageToRow(uint32_t page_idx);
  // Initialize the platform data
  void PlatDataInit(uint32_t speed, uint32_t tx_bus_width, uint32_t rx_bus_width);
  // Execute a SPI operation: opcode + address + placeholder + data cycles
  zx_status_t SpiNandExecOp(const SpiOp op);
  // Get the feature setting or status of the device by the "Get Feature" operation
  zx_status_t SpiNandReadRegOp(uint8_t reg, uint8_t *val);
  // Set the individual feature by the "Set Feature" operation
  zx_status_t SpiNandWriteRegOp(uint8_t reg, uint8_t val);
  // Read the status from the feature table(0ffset 0xC0)
  zx_status_t SpiNandReadStatus(uint8_t *status);
  // Check the device status and wait the operation completed
  zx_status_t SpiNandWait(uint8_t *data);
  // Reset operation by the reset command 0xFF or 0xFE
  zx_status_t SpiNandResetOp();
  // Read the id information by the command 0x9F
  zx_status_t SpiNandReadIdOp(uint8_t *buf);
  // Allocate a buffer as cache to save the configuration register
  zx_status_t SpiNandInitCfgCache();
  // Get the configuration register from the cache buffer
  zx_status_t SpiNandGetCfg(uint8_t *cfg);
  // Set the configuration register
  zx_status_t SpiNandSetCfg(uint8_t cfg);
  // Get and then update the configuration register
  zx_status_t SpiNandUpdateCfg(uint8_t mask, uint8_t val);
  // Detect the flash chip
  zx_status_t SpiNandDetect();
  // Enable the SPI quad mode
  zx_status_t SpiNandInitQuadEnable();
  // Read the data from the cell array to the internal data buffer
  zx_status_t SpiNandLoadPageOp(uint32_t addr);
  // To output the data from the internal data buffer to user buffer
  zx_status_t SpiNandReadFromCacheOp(OobOps mode, uint8_t *data, size_t data_size,
                                     size_t *data_actual, uint8_t *oob, size_t oob_size,
                                     size_t *oob_actual);
  // Set or reset the WEL(Write Enable Latch) bit in the feature table
  zx_status_t SpiNandWriteEnableOp();
  // To transfer data to the internal data buffer
  zx_status_t SpiNandWriteToCacheOp(OobOps mode, const uint8_t *data, size_t data_size,
                                    const uint8_t *oob, size_t oob_size);
  // To program data from the buffer to the cell array
  zx_status_t SpiNandProgramOp(uint32_t addr);
  // To erase data in the block
  zx_status_t SpiNandEraseOp(uint32_t addr);
  // Check the interal ecc status
  zx_status_t SpiNandGetEccBitflips(uint8_t status, uint8_t *out_bitflips);
  // Read one page as follows:
  // 1. Read Cell Array
  // 2. Get Feature
  // 3. Read Buffer
  zx_status_t SpiNandWritePage(OobOps mode, uint32_t nand_page, const uint8_t *data,
                               size_t data_size, const uint8_t *oob, size_t oob_size);
  // Write one page as follows:
  // 1. Write Enable
  // 2. Program Load
  // 3. Program Execute
  // 4. Get Feature
  zx_status_t SpiNandReadPage(OobOps mode, uint32_t nand_page, uint8_t *data, size_t data_size,
                              size_t *data_actual, uint8_t *oob, size_t oob_size,
                              size_t *oob_actual, uint8_t *out_bitflips);

  std::unique_ptr<AmlSpiFlashController> flash_controller_;
  std::optional<FlashChipInfo> flash_chip_;
  SpiNandDev nand_dev_;
  PlatData device_config_;

  compat::SyncInitializedDeviceServer compat_server_;
  fidl::ClientEnd<fuchsia_driver_framework::NodeController> child_node_controller_;

  std::shared_ptr<fdf::Namespace> incoming_;
  const std::shared_ptr<fdf::Namespace> &incoming() const { return incoming_; }
};

}  // namespace amlspinand

#endif  // SRC_DEVICES_NAND_DRIVERS_AML_SPINAND_AML_SPINAND_H_
