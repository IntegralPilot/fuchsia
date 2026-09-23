// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/pci/drivers/pci/bus.h"

#include <fidl/fuchsia.hardware.pci/cpp/natural_types.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fuchsia/hardware/pciroot/c/banjo.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/pci/constants.h>
#include <lib/pci/hw.h>
#include <lib/stdcompat/span.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/syscalls/port.h>

#include <list>

#include <fbl/alloc_checker.h>
#include <fbl/array.h>
#include <fbl/auto_lock.h>
#include <fbl/vector.h>

#include "src/devices/pci/drivers/pci/bridge.h"
#include "src/devices/pci/drivers/pci/config.h"
#include "src/devices/pci/drivers/pci/device.h"

namespace pci {

zx_status_t pci_bus_bind(void* ctx, zx_device_t* parent) {
  pciroot_protocol_t pciroot = {};
  zx_status_t status = device_get_protocol(parent, ZX_PROTOCOL_PCIROOT, &pciroot);
  if (status != ZX_OK) {
    zxlogf(ERROR, "failed to obtain pciroot protocol: %s", zx_status_get_string(status));
    return status;
  }

  pci_platform_info_t info = {};
  status = pciroot_get_pci_platform_info(&pciroot, &info);
  if (status != ZX_OK) {
    zxlogf(ERROR, "failed to obtain platform information: %s!", zx_status_get_string(status));
    return status;
  }

  // A PCI bus should have an ecam, but they are not mandatory per spec depending on the
  // platform tables offered to us.
  std::optional<fdf::MmioBuffer> ecam;
  if (info.cam.vmo != ZX_HANDLE_INVALID) {
    if (auto result = pci::Bus::MapConfigRegion(zx::vmo(info.cam.vmo)); result.is_ok()) {
      ecam = std::move(result.value());
    } else {
      return result.status_value();
    }
  }

  fbl::AllocChecker ac;
  auto bus = std::unique_ptr<pci::Bus>(new (&ac) pci::Bus(parent, &pciroot, info, std::move(ecam)));
  if (!ac.check()) {
    zxlogf(ERROR, "failed to allocate bus object");
    return ZX_ERR_NO_MEMORY;
  }

  if (zx_status_t status = bus->Initialize(); status != ZX_OK) {
    zxlogf(ERROR, "failed to initialize driver: %s", zx_status_get_string(status));
    if (bus->zxdev() != nullptr) {
      bus->DdkAsyncRemove();
    } else {
      zxlogf(INFO, "initialization never assigned device, skipping removal");
    }
    return status;
  }

  // The DDK owns the object if we've made it this far.
  [[maybe_unused]] void* released_ptr = bus.release();
  return ZX_OK;
}

zx_status_t Bus::Initialize() {
  zx_status_t status = DdkAdd(ddk::DeviceAddArgs("bus").set_flags(DEVICE_ADD_NON_BINDABLE));
  if (status != ZX_OK) {
    zxlogf(ERROR, "failed to add bus driver: %s", zx_status_get_string(status));
    return status;
  }

  dispatcher_ = fdf::Dispatcher::GetCurrent()->async_dispatcher();
  zx::result result = DdkAddService<fuchsia_hardware_pci::BusService>(
      fuchsia_hardware_pci::BusService::InstanceHandler({
          .bus = bindings_.CreateHandler(this, dispatcher_, fidl::kIgnoreBindingClosure),
      }));
  if (result.is_error()) {
    zxlogf(ERROR, "failed to add BusService: %s", result.status_string());
    return result.status_value();
  }

  if (zx::result<PciFidl::BoardConfiguration> result =
          ddk::GetMetadata<PciFidl::BoardConfiguration>(parent_);
      result.is_ok()) {
    board_config_ = std::move(result.value());
  }

  // Stash the ops/ctx pointers for the pciroot protocol so we can pass
  // them to the allocators provided by Pci(e)Root. The initial root is
  // created to manage the start of the bus id range given to use by the
  // pciroot protocol.
  fbl::AllocChecker ac;
  root_ = std::unique_ptr<PciRoot>(new (&ac) PciRoot(info_.start_bus_num, pciroot_));
  if (!ac.check()) {
    zxlogf(ERROR, "failed to allocate root");
    return ZX_ERR_NO_MEMORY;
  }

  acpi_devices_ = cpp20::span<const pci_bdf_t>(info_.acpi_bdfs_list, info_.acpi_bdfs_count);
  devicetree_devices_ =
      cpp20::span<const pci_bdf_t>(info_.devicetree_bdfs_list, info_.devicetree_bdfs_count);
  irqs_ = cpp20::span<const pci_legacy_irq>(info_.legacy_irqs_list, info_.legacy_irqs_count);
  irq_routing_entries_ =
      cpp20::span<const pci_irq_routing_entry_t>(info_.irq_routing_list, info_.irq_routing_count);

  // Begin our bus scan starting at our root
  ScanDownstream();
  status = ConfigureLegacyIrqs();
  if (status != ZX_OK) {
    zxlogf(ERROR, "error configuring legacy IRQs, they will be unavailable: %s",
           zx_status_get_string(status));
    return status;
  }
  root_->ConfigureDownstreamDevices();

  return ZX_OK;
}

// Maps a vmo as an mmio_buffer to be used as this Bus driver's ECAM region
// for config space access.
zx::result<fdf::MmioBuffer> Bus::MapConfigRegion(zx::vmo cam_vmo) {
  size_t size;
  zx_status_t status = cam_vmo.get_size(&size);
  if (status != ZX_OK) {
    zxlogf(ERROR, "couldn't get ecam vmo size: %d!", status);
    return zx::error(status);
  }

  zx::result<fdf::MmioBuffer> result =
      fdf::MmioBuffer::Create(0, size, std::move(cam_vmo), ZX_CACHE_POLICY_UNCACHED_DEVICE);
  if (result.is_error()) {
    zxlogf(ERROR, "couldn't map ecam vmo: %s!", result.status_string());
    return result.take_error();
  }

  zxlogf(DEBUG, "ecam for mapped at %p (size: %#zx)", result->get(), result->get_size());
  return zx::ok(std::move(*result));
}

zx::result<std::unique_ptr<Config>> Bus::MakeConfig(pci_bdf_t bdf) {
  if (ecam_) {
    zx::result result = MmioConfig::Create(bdf, ecam_.value(), info_.start_bus_num,
                                           info_.end_bus_num, info_.cam.is_extended);
    if (result.is_error()) {
      return result.take_error();
    }
    return zx::ok(std::move(result.value()));
  }

  return ProxyConfig::Create(bdf, &pciroot_);
}

// Scan downstream starting at the bus id managed by the Bus's Root.
// In the process of scanning, take note of bridges found and configure any that are
// unconfigured. In the end the Bus should have a list of all devides, and all bridges
// should have a list of pointers to their own downstream devices.
zx_status_t Bus::ScanDownstream() {
  std::list<BusScanEntry> scan_list;
  // First scan the bus id associated with our root.
  BusScanEntry entry = {{static_cast<uint8_t>(root_->managed_bus_id()), 0, 0}, root_.get()};
  entry.upstream = root_.get();
  scan_list.push_back(entry);

  // Process any bridges found under the root, any bridges under those bridges, etc...
  // It's important that we scan in the order we discover bridges (DFS) because
  // when we implement bus id assignment it will affect the overall numbering
  // scheme of the bus topology.
  while (!scan_list.empty()) {
    auto entry = scan_list.back();
    zxlogf(TRACE, "scanning from %02x:%02x.%01x upstream: %s", entry.bdf.bus_id,
           entry.bdf.device_id, entry.bdf.function_id,
           (entry.upstream->type() == UpstreamNode::Type::ROOT)
               ? "root"
               : static_cast<Bridge*>(entry.upstream)->config()->addr());
    // Remove this entry, otherwise we'll pop the wrong child off if the scan
    // adds any new bridges / resume points.
    scan_list.pop_back();
    ScanBus(entry, &scan_list);
  }

  return ZX_OK;
}

void Bus::ScanBus(BusScanEntry entry, std::list<BusScanEntry>* scan_list) {
  uint32_t bus_id = entry.bdf.bus_id;  // 32bit so bus_id won't overflow 8bit in the loop
  uint8_t _dev_id = entry.bdf.device_id;
  uint8_t _func_id = entry.bdf.function_id;
  uint8_t _max_functions = entry.max_functions;
  UpstreamNode* upstream = entry.upstream;
  for (uint8_t dev_id = _dev_id; dev_id < kMaxDevicesPerBus; dev_id++) {
    uint8_t max_functions = _max_functions;
    for (uint8_t func_id = _func_id; func_id < max_functions; func_id++) {
      pci_bdf_t bdf = {static_cast<uint8_t>(bus_id), dev_id, func_id};
      zx::result<std::unique_ptr<Config>> config = MakeConfig(bdf);
      if (config.is_error()) {
        continue;
      }

      if (func_id == 0) {
        // Check if this device is multi-function. If it is, scan all 8 functions, otherwise, just
        // scan 1.
        if (config->Read(Config::kHeaderType) & kHeaderTypeMultiFn) {
          max_functions = kMaxFunctionsPerDevice;
        }
      }

      // Check that the device is valid by verifying the vendor and device ids.
      if (config->Read(Config::kVendorId) == kInvalidVendorId) {
        continue;
      }

      bool is_bridge =
          ((config->Read(Config::kHeaderType) & kHeaderTypeMask) == kHeaderTypePciBridge);
      zxlogf(TRACE, "\tfound %s at %02x:%02x.%1x VID %#04x DID %#04x",
             (is_bridge) ? "bridge" : "device", bus_id, dev_id, func_id,
             config->Read(Config::kVendorId), config->Read(Config::kDeviceId));

      // If we found a bridge, add it to our bridge list and initialize /
      // enumerate it after we finish scanning this bus
      if (is_bridge) {
        fbl::RefPtr<Bridge> bridge;
        uint8_t mbus_id = config->Read(Config::kSecondaryBusId);
        zx_status_t status =
            Bridge::Create(zxdev(), std::move(config.value()), upstream, this, mbus_id, &bridge);
        if (status != ZX_OK) {
          zxlogf(ERROR, "failed to create Bridge at %s: %s", config->addr(),
                 zx_status_get_string(status));
          continue;
        }

        // Create scan entries for the next device we would have looked
        // at in the current level of the tree, as well as the new
        // bridge. Since we always work our way from the top of the scan
        // stack we effectively scan the bus in a DFS manner. |func_id|
        // is always incremented by one to ensure we don't scan this
        // same bdf again. If the incremented value is invalid then the
        // device_id loop will iterate and we'll be in a good state
        // again.
        BusScanEntry resume_entry{};
        resume_entry.bdf.bus_id = static_cast<uint8_t>(bus_id);
        resume_entry.bdf.device_id = dev_id;
        resume_entry.bdf.function_id = static_cast<uint8_t>(func_id + 1);
        resume_entry.upstream = upstream;
        resume_entry.max_functions = max_functions;
        scan_list->push_back(resume_entry);

        BusScanEntry bridge_entry{};
        bridge_entry.bdf.bus_id = static_cast<uint8_t>(bridge->managed_bus_id());
        bridge_entry.upstream = bridge.get();  // the new bridge will be this scan's upstream
        scan_list->push_back(bridge_entry);
        // Quit this scan and pick up again based on the scan entries found.
        return;
      }

      // We're at a leaf node in the topology so create a normal device.
      char addr[ZX_MAX_NAME_LEN];
      strncpy(addr, config->addr(), sizeof(addr));
      zx_status_t status =
          pci::Device::Create(zxdev(), std::move(config.value()), upstream, this,
                              /*has_acpi=*/DeviceHasAcpi(config->bdf()),
                              /*has_devicetree=*/DeviceHasDevicetree(config->bdf()));
      if (status != ZX_OK) {
        zxlogf(ERROR, "failed to create device at %s: %s", addr, zx_status_get_string(status));
      }
    }

    // Reset _func_id and _max_functions so the next device will be scanned starting from function 0
    // with the default single-function assumption
    _func_id = 0;
    _max_functions = BusScanEntry::kDefaultMaxFunctions;
  }
}

bool Bus::DeviceHasAcpi(pci_bdf_t bdf) {
  auto find_fn = [&bdf](const pci_bdf_t& entry) -> bool {
    return bdf.bus_id == entry.bus_id && bdf.device_id == entry.device_id &&
           bdf.function_id == entry.function_id;
  };
  return std::find_if(acpi_devices_.begin(), acpi_devices_.end(), find_fn) !=
         std::end(acpi_devices_);
}

bool Bus::DeviceHasDevicetree(pci_bdf_t bdf) {
  auto find_fn = [&bdf](const pci_bdf_t& entry) -> bool {
    return bdf.bus_id == entry.bus_id && bdf.device_id == entry.device_id &&
           bdf.function_id == entry.function_id;
  };
  return std::find_if(devicetree_devices_.begin(), devicetree_devices_.end(), find_fn) !=
         std::end(devicetree_devices_);
}

zx_status_t Bus::SetUpLegacyIrqHandlers() {
  // Most cases devices will use MSI / MSI-X anyway so warnings are sufficient on bind failure.
  for (auto& irq : irqs_) {
    zx::interrupt interrupt(irq.interrupt);
    fbl::AllocChecker ac;
    auto shared_vector = std::unique_ptr<SharedVector>(new (&ac) SharedVector());
    if (!ac.check()) {
      zxlogf(ERROR, "failed to allocate shared vector for irq %#x", irq.vector);
      return ZX_ERR_NO_MEMORY;
    }
    shared_vector->interrupt = std::move(interrupt);
    shared_vector->irq_handler.set_object(shared_vector->interrupt.get());
    shared_vector->irq_handler.set_handler(
        [this, vector = irq.vector](async_dispatcher_t* dispatcher, async::Irq* irq,
                                    zx_status_t status, const zx_packet_interrupt_t* interrupt) {
          HandleLegacyIrq(dispatcher, irq, status, interrupt, vector);
        });

    zx_status_t status = shared_vector->irq_handler.Begin(dispatcher_);
    if (status != ZX_OK) {
      zxlogf(WARNING, "failed to bind irq %#x to dispatcher: %s", irq.vector,
             zx_status_get_string(status));
      return status;
    }

    // Every vector has a list of devices associated with it that are wired to that IRQ.
    shared_irqs_[irq.vector] = std::move(shared_vector);
  }

  return ZX_OK;
}

zx_status_t Bus::ConfigureLegacyIrqs() {
  fbl::AutoLock devices_lock(&devices_lock_);
  zx_status_t status = SetUpLegacyIrqHandlers();
  if (status != ZX_OK) {
    return status;
  }

  // Scan all the devices found and figure out their interrupt pin based on the
  // routing table provided by the platform. While we hold the devices_lock no
  // changes can be made to the Bus topology, ensuring the lifetimes of the
  // upstream paths and config accesses.
  for (const auto& [bdf, device] : devices_) {
    uint8_t pin = device->config()->Read(Config::kInterruptPin);
    // If a device has no pin configured in the InterruptPin register then it
    // has no legacy interrupt. PCI Local Bus Spec v3 Section 2.2.6.
    if (pin == 0 && board_config_.use_intx_workaround().has_value()) {
      pin = 1;  // TODO(365837900): Re-add 'continue' here when crosvm Interrupt Pin is populated.
    }

    // To avoid devices all ending up on the same pin the PCI bridge spec
    // defines a transformation per pin based on the device id of a given function
    // and pin. This transformation is applied at every transition from a
    // secondary bus to a primary bus up to the root. In effect, we swizzle the
    // pin every time we find a bridge working our way back up to the root. At
    // the same time, we also want to record the bridge closest to the root in
    // case it is a root port so that we can check the correct irq routing table
    // entries.
    //
    // Pci Bridge to Bridge spec r1.2 Table 9-1
    // PCI Express Base Specification r4.0 Table 2-19
    UpstreamNode* upstream = device->upstream();
    std::optional<pci_bdf_t> port;
    while (upstream && upstream->type() == UpstreamNode::Type::BRIDGE) {
      pin = (pin + device->dev_id()) % kMaxLegacyIrqPins;
      auto bridge = static_cast<pci::Bridge*>(upstream);
      port = bridge->config()->bdf();
      upstream = bridge->upstream();
    }
    ZX_DEBUG_ASSERT(upstream);
    ZX_DEBUG_ASSERT(upstream->type() == UpstreamNode::Type::ROOT);

    // If we didn't find a parent then the device must be a root complex endpoint.
    if (!port) {
      port = {.device_id = PCI_IRQ_ROUTING_NO_PARENT, .function_id = PCI_IRQ_ROUTING_NO_PARENT};
    }

    // There must be a routing entry for a given device / root port combination
    // in order for a device's legacy IRQ to work. Attempt to find it and use
    // the newly swizzled pin value to find the hardware vector.
    auto find_fn = [&device, port](auto& entry) -> bool {
      return entry.port_device_id == port->device_id &&
             entry.port_function_id == port->function_id && entry.device_id == device->dev_id();
    };

    auto found = std::find_if(irq_routing_entries_.begin(), irq_routing_entries_.end(), find_fn);
    if (found != std::end(irq_routing_entries_)) {
      uint8_t vector = found->pins[pin - 1];
      device->config()->Write(Config::kInterruptLine, vector);
      zxlogf(DEBUG, "[%s] pin %u mapped to %#x", device->config()->addr(), pin, vector);
    } else {
      zxlogf(DEBUG, "[%s] no legacy routing entry found for device", device->config()->addr());
    }
  }

  return ZX_OK;
}

Bus::~Bus() {
  if (root_) {
    root_->DisableDownstream();
    root_->UnplugDownstream();
  }
  // Pending async::Irq and async::Wait objects are automatically canceled
  // by their respective destructors as shared_irqs_ is cleared.
}

void Bus::HandleDeviceLegacyIrqUntriggered(async_dispatcher_t* /*dispatcher*/,
                                           async::Wait* /*wait*/, zx_status_t status,
                                           const zx_packet_signal_t* /*signal*/,
                                           pci::Device* device) {
  if (status == ZX_ERR_CANCELED) {
    return;
  }
  if (status != ZX_OK) {
    zxlogf(ERROR, "Error waiting on legacy irq untriggered for %s: %s", device->config()->addr(),
           zx_status_get_string(status));
    return;
  }
  fbl::AutoLock dev_lock(device->dev_lock());
  if (device->irqs().mode != fuchsia_hardware_pci::InterruptMode::kLegacy) {
    return;
  }
  device->EnableLegacyIrq();
}

void Bus::HandleLegacyIrq(async_dispatcher_t* dispatcher, async::Irq* irq, zx_status_t status,
                          const zx_packet_interrupt_t* interrupt, uint32_t vector) {
  if (status == ZX_ERR_CANCELED) {
    return;
  }
  if (status != ZX_OK || !interrupt) {
    zxlogf(ERROR, "Unexpected error in IRQ handling for vector %#x, status = %s", vector,
           zx_status_get_string(status));
    return;
  }

  // This is effectively our 'fast path'. We've received an interrupt packet
  // and we need to scan the list for devices mapped to that vector to see
  // which ones have an interrupt asserted in their status register. In a
  // typical situation a bus driver is required to check if a driver has
  // interrupts enabled and if the status bit is asserted. However, in our
  // case if a device exists in this list it was only through enabling
  // legacy IRQs, ensuring that interrupts are enabled. We can save a config
  // read and just check status thanks to this.
  fbl::AutoLock devices_lock(&devices_lock_);
  auto result = shared_irqs_.find(vector);
  if (result == shared_irqs_.end()) {
    zxlogf(ERROR, "Received IRQ for untracked vector %#x", vector);
    return;
  }
  SharedVector& shared_vector = *result->second;
  SharedDeviceList& list = shared_vector.list;
  for (auto& entry : list) {
    pci::Device* device = entry->device;
    fbl::AutoLock device_lock(device->dev_lock());
    if (entry->wait.is_pending() || device->irqs().legacy_disabled) {
      continue;
    }
    config::Status dev_status = {.value = device->config()->Read(Config::kStatus)};
    if (dev_status.interrupt_status() || board_config_.use_intx_workaround()) {
      // Legacy (INTx) interrupts are level-triggered: the device holds its
      // interrupt line asserted until its driver services the device and
      // clears the source. Mask the device's interrupt (set INT_DISABLE) so
      // the line deasserts before we re-arm the physical interrupt below,
      // preventing interrupt storms.
      device->DisableLegacyIrq();

      // Trigger the virtual interrupt the device driver is using by proxy.
      zx_status_t signal_status = device->SignalLegacyIrq(interrupt->timestamp);
      if (signal_status != ZX_OK) {
        zxlogf(ERROR, "failed to signal vector %#x for device %s: %s", vector,
               device->config()->addr(), zx_status_get_string(signal_status));
        // If signaling fails (e.g. the client driver closed or destroyed its virtual
        // interrupt handle), the device intentionally remains masked to prevent
        // physical interrupt storms.
        continue;
      }

      zx_status_t wait_status = entry->wait.Begin(dispatcher_);
      if (wait_status != ZX_OK) {
        zxlogf(ERROR, "failed to begin wait on untriggered for %s: %s", device->config()->addr(),
               zx_status_get_string(wait_status));
      }
    }
  }

  // Re-arm the physical interrupt for subsequent deliveries now that all asserting
  // devices sharing this vector have been masked.
  zx_status_t ack_status = shared_vector.interrupt.ack();
  if (ack_status != ZX_OK) {
    zxlogf(ERROR, "Failed to ack vector %#x after servicing device: %s", vector,
           zx_status_get_string(ack_status));
  }
}

}  // namespace pci
