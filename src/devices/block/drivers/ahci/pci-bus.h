// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_AHCI_PCI_BUS_H_
#define SRC_DEVICES_BLOCK_DRIVERS_AHCI_PCI_BUS_H_

#include <fidl/fuchsia.hardware.pci/cpp/wire.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/zx/bti.h>
#include <lib/zx/interrupt.h>
#include <zircon/types.h>

#include "bus.h"

namespace ahci {

class PciBus : public Bus {
 public:
  explicit PciBus(fidl::WireSyncClient<fuchsia_hardware_pci::Device> pci) : pci_(std::move(pci)) {}
  ~PciBus() override;
  zx_status_t Configure() override;
  zx_status_t DmaBufferInit(std::unique_ptr<dma_buffer::ContiguousBuffer>* buffer_out, size_t size,
                            zx_paddr_t* phys_out, void** virt_out) override;
  zx_status_t BtiPin(uint32_t options, const zx::unowned_vmo& vmo, uint64_t offset, uint64_t size,
                     zx_paddr_t* addrs, size_t addrs_count, zx::pmt* pmt_out) override;

  zx_status_t RegRead(size_t offset, uint32_t* val_out) override;
  zx_status_t RegWrite(size_t offset, uint32_t val) override;

  zx_status_t InterruptWait() override;
  void InterruptCancel() override;

 private:
  fidl::WireSyncClient<fuchsia_hardware_pci::Device> pci_;
  fuchsia_hardware_pci::InterruptMode irq_mode_;
  std::optional<fdf::MmioBuffer> mmio_;
  zx::bti bti_;
  zx::interrupt irq_;
};

}  // namespace ahci

#endif  // SRC_DEVICES_BLOCK_DRIVERS_AHCI_PCI_BUS_H_
