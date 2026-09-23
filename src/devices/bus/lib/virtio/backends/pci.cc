// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <lib/virtio/backends/pci.h>
#include <lib/zx/handle.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/port.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/syscalls/port.h>

#include <algorithm>
#include <utility>

#include <virtio/virtio.h>

#include "src/graphics/display/lib/driver-framework-migration-utils/logging/zxlogf.h"

namespace virtio {

namespace fpci = fuchsia_hardware_pci;

PciBackend::PciBackend(fidl::ClientEnd<fuchsia_hardware_pci::Device> pci,
                       fuchsia_hardware_pci::DeviceInfo info)
    : pci_(std::move(pci)), info_(std::move(info)) {
  snprintf(tag_, sizeof(tag_), "pci[%02x:%02x.%1x]", info_.bus_id(), info_.dev_id(),
           info_.func_id());
}

zx_status_t PciBackend::Bind() {
  zx::interrupt interrupt;
  zx_status_t status = zx::port::create(/*options=*/ZX_PORT_BIND_TO_INTERRUPT, &wait_port_);
  if (status != ZX_OK) {
    zxlogf(ERROR, "cannot create wait port: %s", zx_status_get_string(status));
    return status;
  }

  // enable bus mastering
  fidl::Result result = fidl::Call(pci())->SetBusMastering(true);
  if (result.is_error()) {
    zxlogf(ERROR, "cannot enable bus master: %s", result.error_value().FormatDescription().c_str());
    if (result.error_value().is_domain_error()) {
      return result.error_value().domain_error();
    } else {
      return result.error_value().framework_error().status();
    }
  }

  status = ConfigureInterruptMode();
  if (status != ZX_OK) {
    zxlogf(ERROR, "cannot configure IRQs: %s", zx_status_get_string(status));
    return status;
  }

  return Init();
}

// Virtio supports both a legacy INTx IRQ as well as MSI-X. In the former case,
// a driver is required to read the ISR_STATUS register to determine what sort
// of event has happened. This can be an expensive operation depending on the
// hypervisor / emulation environment. For MSI-X a device 'should' support 2 or
// more vector table entries, but is not required to. Since we only have one IRQ
// worker in the backends at this time it's not that important that we allocate
// a vector per ring, so for now the ideal is roughly two vectors, one being for
// config changes and the other for rings.
zx_status_t PciBackend::ConfigureInterruptMode() {
  // This looks a lot like something ConfigureInterruptMode was designed for, but
  // since we have a specific requirement to use MSI-X if and only if we have 2
  // vectors it means rolling it by hand.
  fpci::InterruptMode mode = fpci::InterruptMode::kMsiX;
  uint32_t irq_cnt = 2;
  fidl::Result result = fidl::Call(pci())->SetInterruptMode({{mode, irq_cnt}});
  if (result.is_error()) {
    mode = fpci::InterruptMode::kLegacy;
    irq_cnt = 1;
    result = fidl::Call(pci())->SetInterruptMode({{mode, irq_cnt}});
    if (result.is_error()) {
      irq_cnt = 0;
    }
  }

  if (irq_cnt == 0) {
    zxlogf(ERROR, "Failed to configure a virtio IRQ mode: %s",
           result.error_value().FormatDescription().c_str());
    if (result.error_value().is_domain_error()) {
      return result.error_value().domain_error();
    } else {
      return result.error_value().framework_error().status();
    }
  }

  // Legacy only supports 1 IRQ, but for MSI-X we only need 2
  for (uint32_t i = 0; i < irq_cnt; i++) {
    fidl::Result result = fidl::Call(pci())->MapInterrupt(i);
    if (result.is_error()) {
      zxlogf(ERROR, "Failed to map interrupt %u: %s", i,
             result.error_value().FormatDescription().c_str());
      if (result.error_value().is_domain_error()) {
        return result.error_value().domain_error();
      } else {
        return result.error_value().framework_error().status();
      }
    }
    zx::interrupt& interrupt = result.value().interrupt();

    // Use the interrupt index as the key so we can ack the correct interrupt after
    // a port wait.
    zx_status_t status = interrupt.bind(wait_port_, /*key=*/i, /*options=*/0);
    if (status != ZX_OK) {
      zxlogf(ERROR, "Failed to bind interrupt %u: %s", i, zx_status_get_string(status));
      return status;
    }
    irq_handles().push_back(std::move(interrupt));
  }
  irq_mode() = mode;
  zxlogf(DEBUG, "using %s IRQ mode (irq_cnt = %u)",
         (irq_mode() == fpci::InterruptMode::kMsiX ? "MSI-X" : "legacy"), irq_cnt);
  return ZX_OK;
}

zx::result<uint32_t> PciBackend::WaitForInterrupt() {
  zx_port_packet packet;
  zx_status_t status = wait_port_.wait(zx::deadline_after(zx::msec(100)), &packet);
  if (status != ZX_OK) {
    return zx::error(status);
  }

  return zx::ok(static_cast<uint32_t>(packet.key));
}

void PciBackend::InterruptAck(uint32_t key) {
  ZX_DEBUG_ASSERT(key < irq_handles().size());
  irq_handles()[key].ack();
  if (irq_mode() == fpci::InterruptMode::kLegacy) {
    std::ignore = fidl::Call(pci())->AckInterrupt();
  }
}

}  // namespace virtio
