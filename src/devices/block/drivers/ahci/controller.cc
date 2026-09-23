// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "controller.h"

#include <inttypes.h>
#include <lib/zx/clock.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/param.h>
#include <unistd.h>
#include <zircon/assert.h>
#include <zircon/listnode.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <atomic>
#include <memory>
#include <mutex>

#include <fbl/alloc_checker.h>

#include "pci-bus.h"
#include "sata.h"

namespace ahci {

//clang-format on

// TODO(sron): Check return values from bus_->RegRead() and RegWrite().
// Handle properly for buses that may by unplugged at runtime.
uint32_t Controller::RegRead(size_t offset) {
  uint32_t val = 0;
  bus_->RegRead(offset, &val);
  return val;
}

zx_status_t Controller::RegWrite(size_t offset, uint32_t val) {
  return bus_->RegWrite(offset, val);
}

void Controller::AhciEnable() {
  uint32_t ghc = RegRead(kHbaGlobalHostControl);
  if (ghc & AHCI_GHC_AE)
    return;
  for (int i = 0; i < 5; i++) {
    ghc |= AHCI_GHC_AE;
    RegWrite(kHbaGlobalHostControl, ghc);
    ghc = RegRead(kHbaGlobalHostControl);
    if (ghc & AHCI_GHC_AE)
      return;
    usleep(10 * 1000);
  }
}

zx_status_t Controller::HbaReset() {
  // AHCI 1.3: Software may perform an HBA reset prior to initializing the controller
  uint32_t ghc = RegRead(kHbaGlobalHostControl);
  ghc |= AHCI_GHC_AE;
  RegWrite(kHbaGlobalHostControl, ghc);
  ghc |= AHCI_GHC_HR;
  RegWrite(kHbaGlobalHostControl, ghc);
  // reset should complete within 1 second
  zx_status_t status = bus_->WaitForClear(kHbaGlobalHostControl, AHCI_GHC_HR, zx::sec(1));
  if (status) {
    fdf::error("HBA reset timed out");
  }
  return status;
}

zx_status_t Controller::SetDevInfo(uint32_t portnr, SataDeviceInfo* devinfo) {
  if (portnr >= AHCI_MAX_PORTS) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  ports_[portnr].SetDevInfo(devinfo);
  return ZX_OK;
}

void Controller::Queue(uint32_t portnr, SataTransaction* txn) {
  ZX_DEBUG_ASSERT(portnr < AHCI_MAX_PORTS);
  Port* port = &ports_[portnr];
  zx_status_t status = port->Queue(txn);
  if (status == ZX_OK) {
    uint64_t lba = 0;
    uint32_t count = 0;
    if (txn->operation.tag == block_server::Operation::Tag::Read) {
      lba = txn->operation.read.device_block_offset;
      count = txn->operation.read.block_count;
    } else if (txn->operation.tag == block_server::Operation::Tag::Write) {
      lba = txn->operation.write.device_block_offset;
      count = txn->operation.write.block_count;
    }

    fdf::trace("ahci.{}: Queue txn {} tag {} offset_dev 0x{:x} length 0x{:x}", port->num(),
               static_cast<const void*>(txn), static_cast<uint32_t>(txn->operation.tag), lba,
               count);
    // hit the worker loop
    worker_event_completion_.Signal();
  } else {
    fdf::info("ahci.{}: Failed to queue txn {}: {}", port->num(), static_cast<const void*>(txn),
              zx_status_get_string(status));
    txn->Complete(status);
  }
}

void Controller::Stop(fdf::StopCompleter completer) {
  if (sata_devices_.empty()) {
    Shutdown();
    completer(zx::ok());
    return;
  }

  auto shared_completer = std::make_shared<fdf::StopCompleter>(std::move(completer));
  auto count = std::make_shared<std::atomic<size_t>>(sata_devices_.size());

  for (auto& device : sata_devices_) {
    device->Shutdown([this, shared_completer, count]() {
      if (count->fetch_sub(1) == 1) {
        sata_devices_.clear();
        Shutdown();
        (*shared_completer)(zx::ok());
      }
    });
  }
}

bool Controller::ShouldExit() {
  std::lock_guard<std::mutex> lock(lock_);
  return shutdown_;
}

void Controller::WorkerLoop() {
  Port* port;
  for (;;) {
    // iterate all the ports and run or complete commands
    bool port_active = false;
    for (uint32_t i = 0; i < AHCI_MAX_PORTS; i++) {
      port = &ports_[i];

      // Complete commands first.
      bool txns_in_progress = port->Complete();
      // Process queued txns.
      bool txns_added = port->ProcessQueued();
      port_active |= txns_in_progress || txns_added;
    }

    // Exit only when there are no more transactions in flight.
    if ((!port_active) && ShouldExit()) {
      return;
    }

    // Wait here until more commands are queued, or a port becomes idle.
    worker_event_completion_.Wait();
    worker_event_completion_.Reset();
  }
}

void Controller::IrqLoop() {
  for (;;) {
    zx_status_t status = bus_->InterruptWait();
    if (status != ZX_OK) {
      if (!ShouldExit()) {
        fdf::error("Error waiting for interrupt: {}", zx_status_get_string(status));
      }
      return;
    }
    // mask hba interrupts while interrupts are being handled
    uint32_t ghc = RegRead(kHbaGlobalHostControl);
    RegWrite(kHbaGlobalHostControl, ghc & ~AHCI_GHC_IE);

    // handle interrupt for each port
    uint32_t is = RegRead(kHbaInterruptStatus);
    RegWrite(kHbaInterruptStatus, is);
    for (uint32_t i = 0; is && i < AHCI_MAX_PORTS; i++) {
      if (is & 0x1) {
        bool txn_handled = ports_[i].HandleIrq();
        if (txn_handled) {
          // hit the worker loop to complete commands
          worker_event_completion_.Signal();
        }
      }
      is >>= 1;
    }

    // unmask hba interrupts
    ghc = RegRead(kHbaGlobalHostControl);
    RegWrite(kHbaGlobalHostControl, ghc | AHCI_GHC_IE);
  }
}

// implement device protocol:

zx_status_t Controller::Init() {
  zx_status_t status;
  if ((status = LaunchIrqAndWorkerDispatchers()) != ZX_OK) {
    fdf::error("Failed to start controller irq and worker threads: {}",
               zx_status_get_string(status));
    return status;
  }

  // reset
  HbaReset();

  // enable ahci mode
  AhciEnable();

  const uint32_t capabilities = RegRead(kHbaCapabilities);
  const bool use_command_queue = capabilities & AHCI_CAP_NCQ;
  const uint32_t max_command_tag = (capabilities >> 8) & 0x1f;
  if (component_inspector_) {
    inspect_node_ = component_inspector_->root().CreateChild(kDriverName);
  }
  inspect_node_.RecordBool("native_command_queuing", use_command_queue);
  inspect_node_.RecordUint("max_command_tag", max_command_tag);

  // count number of ports
  uint32_t port_map = RegRead(kHbaPortsImplemented);

  // initialize ports
  for (uint32_t i = 0; i < AHCI_MAX_PORTS; i++) {
    if (!(port_map & (1u << i)))
      continue;  // port not implemented
    status = ports_[i].Configure(i, bus_.get(), kHbaPorts, max_command_tag);
    if (status != ZX_OK) {
      fdf::error("Failed to configure port {}: {}", i, zx_status_get_string(status));
      return status;
    }
  }

  // clear hba interrupts
  RegWrite(kHbaInterruptStatus, RegRead(kHbaInterruptStatus));

  // enable hba interrupts
  uint32_t ghc = RegRead(kHbaGlobalHostControl);
  ghc |= AHCI_GHC_IE;
  RegWrite(kHbaGlobalHostControl, ghc);

  // this part of port init happens after enabling interrupts in ghc
  for (uint32_t i = 0; i < AHCI_MAX_PORTS; i++) {
    Port* port = &ports_[i];
    if (!(port->port_implemented()))
      continue;

    // enable port
    port->Enable();

    // enable interrupts
    port->RegWrite(kPortInterruptEnable, AHCI_PORT_INT_MASK);

    // reset port
    port->Reset();

    // FIXME proper layering?
    if (port->RegRead(kPortSataStatus) & AHCI_PORT_SSTS_DET_PRESENT) {
      port->set_device_present(true);
      if (port->RegRead(kPortSignature) == AHCI_PORT_SIG_SATA) {
        zx::result<std::unique_ptr<SataDevice>> device =
            SataDevice::Bind(this, port->num(), use_command_queue);
        if (device.is_error()) {
          fdf::error("Failed to add SATA device at port {}: {}", port->num(),
                     device.status_string());
          return device.status_value();
        }
        sata_devices_.push_back(*std::move(device));
      }
    }
  }

  return ZX_OK;
}

zx_status_t Controller::LaunchIrqAndWorkerDispatchers() {
  auto dispatcher = fdf::SynchronizedDispatcher::Create(
      fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "ahci-irq",
      [&](fdf_dispatcher_t*) { irq_shutdown_completion_.Signal(); });
  if (dispatcher.is_error()) {
    fdf::error("Failed to create irq dispatcher: {}",
               zx_status_get_string(dispatcher.status_value()));
    return dispatcher.status_value();
  }
  irq_dispatcher_ = *std::move(dispatcher);

  zx_status_t status = async::PostTask(irq_dispatcher_.async_dispatcher(), [this] { IrqLoop(); });
  if (status != ZX_OK) {
    fdf::error("Error creating irq loop: {}", zx_status_get_string(status));
    return status;
  }

  worker_shutdown_.store(false);
  dispatcher = fdf::SynchronizedDispatcher::Create(
      fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "ahci-worker",
      [&](fdf_dispatcher_t*) { worker_shutdown_.store(true); });
  if (dispatcher.is_error()) {
    fdf::error("Failed to create dispatcher: {}", zx_status_get_string(dispatcher.status_value()));
    return dispatcher.status_value();
  }
  worker_dispatcher_ = *std::move(dispatcher);

  status = async::PostTask(worker_dispatcher_.async_dispatcher(), [this] { WorkerLoop(); });
  if (status != ZX_OK) {
    fdf::error("Error creating worker loop: {}", zx_status_get_string(status));
    return status;
  }
  return ZX_OK;
}

void Controller::Shutdown() {
  {
    std::lock_guard<std::mutex> lock(lock_);
    shutdown_ = true;
  }

  for (uint32_t i = 0; i < AHCI_MAX_PORTS; i++) {
    if (ports_[i].port_implemented()) {
      ports_[i].Disable();
    }
  }

  if (worker_dispatcher_.get() && !worker_shutdown_.load()) {
    worker_dispatcher_.ShutdownAsync();
    // TODO(https://fxbug.dev/42061061): This driver may be missing a watchdog-like capability to
    // check in on in-flight device requests to detect timeouts. The polling here implements the
    // watchdog only for the shutdown case, but a more general solution may be needed.
    while (!worker_shutdown_.load()) {
      worker_event_completion_.Signal();
      zx::nanosleep(zx::deadline_after(zx::msec(10)));
    }
  }

  // Signal the interrupt loop to exit.
  bus_->InterruptCancel();
  if (irq_dispatcher_.get() && !irq_shutdown_completion_.signaled()) {
    irq_dispatcher_.ShutdownAsync();
    irq_shutdown_completion_.Wait();
  }
}

zx::result<std::unique_ptr<Bus>> Controller::CreateBus() {
  auto pci_client_end = incoming_->Connect<fuchsia_hardware_pci::Service::Device>("pci");
  if (!pci_client_end.is_ok()) {
    fdf::error("Failed to connect to PCI device service: {}", pci_client_end);
    return pci_client_end.take_error();
  }
  auto pci = fidl::WireSyncClient(*std::move(pci_client_end));

  fbl::AllocChecker ac;
  auto bus = fbl::make_unique_checked<PciBus>(&ac, std::move(pci));
  if (!ac.check()) {
    fdf::error("Failed to allocate memory for bus.");
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  return zx::ok(std::move(bus));
}

zx::result<> Controller::Start(fdf::DriverContext context) {
  incoming_ = std::shared_ptr<fdf::Namespace>(context.take_incoming());
  node_name_ = context.node_name();
  node_token_ = context.take_node_token();

  if (AHCI_PAGE_SIZE != zx_system_get_page_size()) {
    fdf::error("System page size of {} does not match expected page size of {}\n",
               zx_system_get_page_size(), AHCI_PAGE_SIZE);
    return zx::error(ZX_ERR_INTERNAL);
  }

  zx::result<std::unique_ptr<Bus>> bus = CreateBus();
  if (bus.is_error()) {
    return bus.take_error();
  }
  bus_ = *std::move(bus);

  zx_status_t status = bus_->Configure();
  if (status != ZX_OK) {
    fdf::error("Failed to configure host bus");
    return zx::error(status);
  }

  auto [controller_client_end, controller_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::NodeController>::Create();
  auto [node_client_end, node_server_end] =
      fidl::Endpoints<fuchsia_driver_framework::Node>::Create();

  node_controller_.Bind(std::move(controller_client_end));
  root_node_.Bind(std::move(node_client_end));

  fidl::Arena arena;

  const auto args =
      fuchsia_driver_framework::wire::NodeAddArgs::Builder(arena).name(arena, name()).Build();

  auto result = fidl::WireCall(node().borrow())
                    ->AddChild(args, std::move(controller_server_end), std::move(node_server_end));
  if (!result.ok()) {
    fdf::error("Failed to add child: {}", result.status_string());
    return zx::error(result.status());
  }

  auto connect_result = incoming_->Connect<fuchsia_inspect::InspectSink>();
  if (connect_result.is_ok()) {
    component_inspector_.emplace(dispatcher(), inspect::PublishOptions{
                                                   .tree_name = "ahci",
                                                   .client_end = std::move(connect_result.value()),
                                               });
  } else {
    fdf::warn("Failed to connect to InspectSink: {}", connect_result.status_string());
  }

  status = Init();
  if (status != ZX_OK) {
    fdf::error("Driver initialization failed: {}", zx_status_get_string(status));
    return zx::error(status);
  }
  return zx::ok();
}

}  // namespace ahci
