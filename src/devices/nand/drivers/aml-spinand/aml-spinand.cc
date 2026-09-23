// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-spinand.h"

#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_properties.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/platform-device/cpp/pdev.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <cstddef>

namespace amlspinand {

AmlSpiNand::AmlSpiNand() : fdf::DriverBase2("aml-spinand") {}

zx::result<> AmlSpiNand::Start(fdf::DriverContext context) {
  incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());

  zx::result pdev_client_end =
      incoming()->Connect<fuchsia_hardware_platform_device::Service::Device>();
  if (pdev_client_end.is_error()) {
    FDF_LOG(ERROR, "Failed to connect to platform device: %s", pdev_client_end.status_string());
    return pdev_client_end.take_error();
  }
  fdf::PDev pdev{std::move(pdev_client_end.value())};

  zx::result controller_mmio = pdev.MapMmio(0);
  if (controller_mmio.is_error()) {
    FDF_LOG(ERROR, "Failed to map controller mmio: %s", controller_mmio.status_string());
    return controller_mmio.take_error();
  }

  zx::result clk_mmio = pdev.MapMmio(1);
  if (clk_mmio.is_error()) {
    FDF_LOG(ERROR, "Failed to map clock mmio: %s", clk_mmio.status_string());
    return clk_mmio.take_error();
  }

  flash_controller_ = std::make_unique<AmlSpiFlashController>(std::move(controller_mmio.value()),
                                                              std::move(clk_mmio.value()));

  zx_status_t status = Init();
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "AML spi nand init failed: %s", zx_status_get_string(status));
    return zx::error(status);
  }

  // detect the nand chip
  flash_chip_ = DetermineFlashChip();
  if (!flash_chip_.has_value()) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  auto data_len = flash_chip_->mem_org.pagesize + flash_chip_->mem_org.oobsize;
  nand_dev_.databuf = std::make_unique<uint8_t[]>(data_len);
  nand_dev_.oobbuf = nand_dev_.databuf.get() + flash_chip_->mem_org.pagesize;
  nand_dev_.flags = flash_chip_->flags;

  status = SpiNandInitCfgCache();
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandInitCfgCache failed");
    return zx::error(status);
  }
  status = SpiNandInitQuadEnable();
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandInitQuadEnable failed");
    return zx::error(status);
  }
  status = SpiNandUpdateCfg(kCfgOtpEnable, 0);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandUpdateCfg failed");
    return zx::error(status);
  }
  status = SpiNandWriteRegOp(kBlockLockReg, kBlockAllUnlocked);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandWriteRegOp failed");
    return zx::error(status);
  }

  // Initialize compat server
  compat::DeviceServer::BanjoConfig banjo_config;
  banjo_config.callbacks[ZX_PROTOCOL_RAW_NAND] = [this]() {
    return compat::DeviceServer::GenericProtocol{
        .ops = &raw_nand_protocol_ops_,
        .ctx = this,
    };
  };

  auto compat_status =
      compat_server_.Initialize(incoming(), outgoing(), context.node_name(), name(),
                                compat::ForwardMetadata::None(), std::move(banjo_config));
  if (compat_status.is_error()) {
    return compat_status.take_error();
  }

  auto offers = compat_server_.CreateOffers2();
  std::vector<fuchsia_driver_framework::NodeProperty2> properties = {
      fdf::MakeProperty2("fuchsia.BIND_PROTOCOL", static_cast<uint32_t>(ZX_PROTOCOL_RAW_NAND)),
  };
  auto child_result = AddChild(name(), properties, offers);
  if (child_result.is_error()) {
    FDF_LOG(ERROR, "Failed to add child node: %s", child_result.status_string());
    return child_result.take_error();
  }
  child_node_controller_ = std::move(child_result.value());

  return zx::ok();
}

zx_status_t AmlSpiNand::RawNandGetNandInfo(nand_info_t *nand_info) {
  nand_info->page_size = flash_chip_->mem_org.pagesize;
  nand_info->pages_per_block = flash_chip_->mem_org.pages_per_eraseblock;
  nand_info->num_blocks = flash_chip_->mem_org.ntargets * flash_chip_->mem_org.luns_per_target *
                          flash_chip_->mem_org.planes_per_lun *
                          flash_chip_->mem_org.eraseblocks_per_lun;
  nand_info->oob_size = flash_chip_->mem_org.oobsize;
  nand_info->ecc_bits = flash_chip_->ecc_req.strength;
  nand_info->nand_class = NAND_CLASS_PARTMAP;
  memset(&nand_info->partition_guid, 0, sizeof(nand_info->partition_guid));

  return ZX_OK;
}

zx_status_t AmlSpiNand::RawNandEraseBlock(uint32_t nand_page) {
  uint32_t flash_size = GetFlashSizeInPages();
  if (nand_page >= flash_size) {
    FDF_LOG(ERROR, "Erase block 0x%x goes beyond chip size of 0x%x", nand_page, flash_size);
    return ZX_ERR_OUT_OF_RANGE;
  }

  if (nand_page % flash_chip_->mem_org.pages_per_eraseblock) {
    FDF_LOG(ERROR, "NAND block %u must be a erasesize_pages (%u) multiple", nand_page,
            flash_chip_->mem_org.pages_per_eraseblock);
    return ZX_ERR_INVALID_ARGS;
  }

  zx_status_t status;
  status = SpiNandWriteEnableOp();
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandErase->SpiNandWriteEnableOp failed");
    return status;
  }

  status = SpiNandEraseOp(nand_page);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandErase->SpiNandEraseOp failed");
    return status;
  }

  uint8_t result;
  status = SpiNandWait(&result);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandErase->SpiNandWait failed");
  } else if (result & kStatusEraseFailed) {
    FDF_LOG(ERROR, "SpiNandErase->SpiNandEraseOp failed");
    status = ZX_ERR_IO;
  }

  return status;
}

zx_status_t AmlSpiNand::RawNandWritePageHwecc(const uint8_t *data, size_t data_size,
                                              const uint8_t *oob, size_t oob_size,
                                              uint32_t nand_page) {
  uint32_t flash_size = GetFlashSizeInPages();
  if (nand_page >= flash_size) {
    FDF_LOG(ERROR, "Write page 0x%x goes beyond chip size of 0x%x", nand_page, flash_size);
    return ZX_ERR_OUT_OF_RANGE;
  }

  zx_status_t status;
  // Enable ecc function
  status = SpiNandUpdateCfg(kCfgEccEnable, kCfgEccEnable);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandUpdateCfg failed");
    return status;
  }

  status = SpiNandWritePage(OobOps::kOpsAutoOob, nand_page, data, data_size, oob, oob_size);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandWritePage failed");
  }

  return status;
}

zx_status_t AmlSpiNand::RawNandReadPageHwecc(uint32_t nand_page, uint8_t *data, size_t data_size,
                                             size_t *data_actual, uint8_t *oob, size_t oob_size,
                                             size_t *oob_actual, uint32_t *ecc_correct) {
  uint32_t flash_size = GetFlashSizeInPages();
  if (nand_page >= flash_size) {
    FDF_LOG(ERROR, "Read page 0x%x goes beyond chip size of 0x%x", nand_page, flash_size);
    return ZX_ERR_OUT_OF_RANGE;
  }

  zx_status_t status;
  // Enable ecc function
  status = SpiNandUpdateCfg(kCfgEccEnable, kCfgEccEnable);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandUpdateCfg failed");
    return status;
  }

  uint8_t ecc_bitflips = 0;
  status = SpiNandReadPage(OobOps::kOpsAutoOob, nand_page, data, data_size, data_actual, oob,
                           oob_size, oob_actual, &ecc_bitflips);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandReadPage failed");
    return status;
  }

  if (ecc_correct != nullptr) {
    *ecc_correct = ecc_bitflips;
  }
  return status;
}

uint32_t AmlSpiNand::PageToRow(uint32_t page_idx) {
  auto block_idx = page_idx / flash_chip_->mem_org.pages_per_eraseblock;
  auto block_offset = page_idx % flash_chip_->mem_org.pages_per_eraseblock;

  auto row = (block_idx << flash_chip_->block_addr_shift | block_offset);

  return row;
}

zx_status_t AmlSpiNand::SpiNandExecOp(const SpiOp op) {
  zx_status_t status;
  uint8_t *tx_buf = nullptr;
  uint8_t *rx_buf = nullptr;

  TransferMsg msg;
  msg.rx_mode = device_config_.rx_mode;
  msg.tx_mode = device_config_.tx_mode;

  msg.cmd = op.cmd.opcode;
  msg.addr = op.addr.val;
  msg.addr_len = op.addr.nbytes;

  if (op.placeholder.nbytes > 0) {
    msg.placeholder = true;
  } else {
    msg.placeholder = false;
  }

  if (op.data.nbytes) {
    if (op.data.dir == SpiDataDir::kSpiDataIn) {
      rx_buf = op.data.buf_in;
    } else {
      tx_buf = op.data.buf_out;
    }
  }

  status = flash_controller_->Xfer(msg, op.data.nbytes, tx_buf, rx_buf);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "spi transfer failed");
  }

  return status;
}

zx_status_t AmlSpiNand::SpiNandReadRegOp(uint8_t reg, uint8_t *val) {
  SpiOp op;
  zx_status_t status;

  status = SpiNandExecOp(op.GetFeature(reg, val));
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandExecOp failed");
  }

  return status;
}

zx_status_t AmlSpiNand::SpiNandWriteRegOp(uint8_t reg, uint8_t val) {
  SpiOp op;

  return SpiNandExecOp(op.SetFeature(reg, &val));
}

zx_status_t AmlSpiNand::SpiNandReadStatus(uint8_t *status) {
  return SpiNandReadRegOp(kStatusReg, status);
}

zx_status_t AmlSpiNand::SpiNandWait(uint8_t *data) {
  zx_status_t status;
  uint8_t reg_val;

  uint32_t retry = 400;
  do {
    status = SpiNandReadStatus(&reg_val);
    if (status != ZX_OK) {
      FDF_LOG(ERROR, "SpiNandReadStatus failed");
      return status;
    }
    if (!(reg_val & kStatusBusy)) {
      break;
    }
  } while (retry--);

  if (data != nullptr) {
    *data = reg_val;
  }

  return reg_val & kStatusBusy ? ZX_ERR_TIMED_OUT : ZX_OK;
}

zx_status_t AmlSpiNand::SpiNandResetOp() {
  zx_status_t status;

  SpiOp op;
  status = SpiNandExecOp(op.Reset());
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandExecOp failed");
    return status;
  }

  return SpiNandWait(nullptr);
}

zx_status_t AmlSpiNand::SpiNandReadIdOp(uint8_t *buf) {
  SpiOp op;
  zx_status_t status;

  status = SpiNandExecOp(op.ReadId(0, buf, kSpinandMaxIdLen));
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandExecOp(ReadId) failed");
  }

  return status;
}

zx_status_t AmlSpiNand::SpiNandInitCfgCache() {
  zx_status_t status;

  status = SpiNandReadRegOp(kCfgReg, &nand_dev_.cfg_cache);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandReadRegOp failed");
  }

  return status;
}

zx_status_t AmlSpiNand::SpiNandGetCfg(uint8_t *cfg) {
  *cfg = nand_dev_.cfg_cache;

  return ZX_OK;
}

zx_status_t AmlSpiNand::SpiNandSetCfg(uint8_t cfg) {
  zx_status_t status;

  if (nand_dev_.cfg_cache == cfg) {
    return ZX_OK;
  }

  status = SpiNandWriteRegOp(kCfgReg, cfg);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandSetCfg failed");
    return status;
  }

  nand_dev_.cfg_cache = cfg;
  return ZX_OK;
}

zx_status_t AmlSpiNand::SpiNandUpdateCfg(uint8_t mask, uint8_t val) {
  zx_status_t status;
  uint8_t cfg;

  status = SpiNandGetCfg(&cfg);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandGetCfg failed");
    return status;
  }

  cfg &= ~mask;
  cfg |= val;

  return SpiNandSetCfg(cfg);
}

zx_status_t AmlSpiNand::SpiNandDetect() {
  zx_status_t status;

  status = SpiNandResetOp();
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandResetOp failed");
    return status;
  }

  status = SpiNandReadIdOp(nand_dev_.id.data);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandReadIdOp failed");
    return status;
  }
  nand_dev_.id.len = kSpinandMaxIdLen;

  return status;
}

zx_status_t AmlSpiNand::SpiNandInitQuadEnable() {
  if (!(nand_dev_.flags & kSpinandHasQeBit)) {
    return ZX_OK;
  }

  bool enable = false;
  if (flash_chip_->read_op.data.buswidth == 4 || flash_chip_->write_op.data.buswidth == 4) {
    enable = true;
  }

  return SpiNandUpdateCfg(kCfgQuadEnable, enable ? kCfgQuadEnable : 0);
}

zx_status_t AmlSpiNand::SpiNandLoadPageOp(uint32_t addr) {
  auto row = PageToRow(addr);
  SpiOp op;

  return SpiNandExecOp(op.PageRead(row));
}

zx_status_t AmlSpiNand::SpiNandReadFromCacheOp(OobOps mode, uint8_t *data, size_t data_size,
                                               size_t *data_actual, uint8_t *oob, size_t oob_size,
                                               size_t *oob_actual) {
  SpiOp op = flash_chip_->read_op;
  zx_status_t status;
  size_t data_len;
  size_t oob_len;

  if (data != nullptr) {
    data_len = flash_chip_->mem_org.pagesize;
    op.data.buf_in = nand_dev_.databuf.get();
    op.data.nbytes = static_cast<uint32_t>(data_len);
  }

  if (oob != nullptr) {
    oob_len = flash_chip_->mem_org.oobsize;
    op.data.nbytes += flash_chip_->mem_org.oobsize;
    if (op.data.buf_in == nullptr) {
      op.data.buf_in = nand_dev_.oobbuf;
      op.addr.val = flash_chip_->mem_org.pagesize;
    }
  }

  status = SpiNandExecOp(op);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandReadFromCacheOp -> SpiNandExecOp failed");
    return status;
  }

  if (data != nullptr) {
    memcpy(data, nand_dev_.databuf.get(), std::min(data_size, data_len));
    if (data_actual != nullptr) {
      *data_actual = std::min(data_size, data_len);
    }
  }
  if (oob != nullptr) {
    uint32_t oob_offset = 0;
    if (mode == OobOps::kOpsAutoOob) {
      oob_offset = flash_chip_->oob_region.offset;
      oob_len = flash_chip_->oob_region.length;
    }
    memcpy(oob, nand_dev_.oobbuf + oob_offset, std::min(oob_size, oob_len));
    if (oob_actual != nullptr) {
      *oob_actual = std::min(oob_size, oob_len);
    }
  }

  return ZX_OK;
}

zx_status_t AmlSpiNand::SpiNandWriteEnableOp() {
  SpiOp op;

  return SpiNandExecOp(op.WrEnable(true));
}

zx_status_t AmlSpiNand::SpiNandWriteToCacheOp(OobOps mode, const uint8_t *data, size_t data_size,
                                              const uint8_t *oob, size_t oob_size) {
  SpiOp op = flash_chip_->write_op;
  size_t data_len = flash_chip_->mem_org.pagesize;
  size_t oob_len = flash_chip_->mem_org.oobsize;
  memset(nand_dev_.databuf.get(), 0xff, data_len + oob_len);

  if (data != nullptr) {
    memcpy(nand_dev_.databuf.get(), data, std::min(data_size, data_len));
    op.data.buf_out = nand_dev_.databuf.get();
    op.data.nbytes = static_cast<uint32_t>(data_len);
  }

  if (oob != nullptr) {
    uint32_t oob_offset = 0;
    if (mode == OobOps::kOpsAutoOob) {
      oob_offset = flash_chip_->oob_region.offset;
      oob_len = flash_chip_->oob_region.length;
    }
    memcpy(nand_dev_.oobbuf + oob_offset, oob, std::min(oob_size, oob_len));
    op.data.nbytes += flash_chip_->mem_org.oobsize;
    if (op.data.buf_out == nullptr) {
      op.data.buf_out = nand_dev_.oobbuf;
      op.addr.val = flash_chip_->mem_org.pagesize;
    }
  }

  zx_status_t status = SpiNandExecOp(op);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandWriteToCacheOp->SpiNandExecOp failed");
  }

  return status;
}

zx_status_t AmlSpiNand::SpiNandProgramOp(uint32_t addr) {
  auto row = PageToRow(addr);
  SpiOp op;

  return SpiNandExecOp(op.ProgExec(row));
}

zx_status_t AmlSpiNand::SpiNandEraseOp(uint32_t addr) {
  auto row = PageToRow(addr);
  SpiOp op;

  return SpiNandExecOp(op.BlockErase(row));
}

zx_status_t AmlSpiNand::SpiNandGetEccBitflips(uint8_t status, uint8_t *out_bitflips) {
  switch (status & kStatusEccMask) {
    case kStatusEccNoBitflips:
      *out_bitflips = 0;
      break;
    case kStatusEccHasBitflips:
      *out_bitflips = 7;
      break;
    case kToshibaStatusEcc8Bitflips:
      *out_bitflips = 8;
      break;
    case kStatusEccUncorrectErr:
      return ZX_ERR_BAD_STATE;
    default:
      return ZX_ERR_INVALID_ARGS;
  }
  return ZX_OK;
}

zx_status_t AmlSpiNand::SpiNandWritePage(OobOps mode, uint32_t nand_page, const uint8_t *data,
                                         size_t data_size, const uint8_t *oob, size_t oob_size) {
  zx_status_t status;

  status = SpiNandWriteEnableOp();
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandWriteEnableOp failed");
    return status;
  }

  status = SpiNandWriteToCacheOp(mode, data, data_size, oob, oob_size);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandWriteToCacheOp failed");
    return status;
  }
  status = SpiNandProgramOp(nand_page);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandProgramOp failed");
    return status;
  }
  uint8_t result;
  status = SpiNandWait(&result);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandWait failed");
  } else if (result & kStatusProgFailed) {
    FDF_LOG(ERROR, "SpiNandProgramOp failed");
    status = ZX_ERR_IO;
  }

  return status;
}

zx_status_t AmlSpiNand::SpiNandReadPage(OobOps mode, uint32_t nand_page, uint8_t *data,
                                        size_t data_size, size_t *data_actual, uint8_t *oob,
                                        size_t oob_size, size_t *oob_actual,
                                        uint8_t *out_bitflips) {
  zx_status_t status = SpiNandLoadPageOp(nand_page);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandLoadPageOp failed");
    return status;
  }

  uint8_t result;
  status = SpiNandWait(&result);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandWait failed");
    return status;
  }

  status = SpiNandReadFromCacheOp(mode, data, data_size, data_actual, oob, oob_size, oob_actual);
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandReadFromCacheOp failed");
    return status;
  }

  return SpiNandGetEccBitflips(result, out_bitflips);
}

std::optional<FlashChipInfo> AmlSpiNand::DetermineFlashChip() {
  zx_status_t status = SpiNandDetect();
  if (status != ZX_OK) {
    FDF_LOG(ERROR, "SpiNandDetect failed");
    return std::nullopt;
  }

  FDF_LOG(INFO, "Found nand device with vendor: 0x%x device: 0x%x", nand_dev_.id.data[0],
          nand_dev_.id.data[1]);
  for (const auto &device : kFlashDevices) {
    if (device.vendor_id == nand_dev_.id.data[0] && device.device_id == nand_dev_.id.data[1]) {
      FDF_LOG(INFO, "%s %s SPI NAND was found", device.name.data(), device.model.data());
      return device;
    }
  }

  return std::nullopt;
}

void AmlSpiNand::PlatDataInit(uint32_t speed, uint32_t tx_bus_width, uint32_t rx_bus_width) {
  SpiRxMode rx_mode = SpiRxMode::kSpiRxDefault;
  SpiTxMode tx_mode = SpiTxMode::kSpiTxDefault;

  device_config_.max_hz = speed;
  flash_controller_->SetSpeed(device_config_.max_hz);

  switch (tx_bus_width) {
    case 1:
      break;
    case 2:
      tx_mode = SpiTxMode::kSpiTxDual;
      break;
    case 4:
      tx_mode = SpiTxMode::kSpiTxQuad;
      break;
    default:
      FDF_LOG(INFO, "spi tx_bus_width %d not supported", tx_bus_width);
      break;
  }

  switch (rx_bus_width) {
    case 1:
      break;
    case 2:
      rx_mode = SpiRxMode::kSpiRxDual;
      break;
    case 4:
      rx_mode = SpiRxMode::kSpiRxQuad;
      break;
    default:
      FDF_LOG(INFO, "spi rx_bus_width %d not supported", rx_bus_width);
      break;
  }

  device_config_.tx_mode = tx_mode;
  device_config_.rx_mode = rx_mode;
}

uint32_t AmlSpiNand::GetFlashSizeInPages() const {
  if (!flash_chip_) {
    return 0;
  }
  return flash_chip_->mem_org.ntargets * flash_chip_->mem_org.luns_per_target *
         flash_chip_->mem_org.planes_per_lun * flash_chip_->mem_org.eraseblocks_per_lun *
         flash_chip_->mem_org.pages_per_eraseblock;
}

zx_status_t AmlSpiNand::Init() {
  // init the controller
  flash_controller_->Init();
  // init the configuration data
  PlatDataInit(kDefaultSpiNandFreq, kDefaultTxBusWidth, kDefaultRXBusWidth);

  return ZX_OK;
}

}  // namespace amlspinand

FUCHSIA_DRIVER_EXPORT2(amlspinand::AmlSpiNand);
