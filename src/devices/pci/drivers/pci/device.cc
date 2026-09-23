// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/pci/drivers/pci/device.h"

#include <assert.h>
#include <err.h>
#include <fidl/fuchsia.hardware.pci/cpp/common_types.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <inttypes.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/driver.h>
#include <lib/driver/legacy-bind-constants/legacy-bind-constants.h>
#include <lib/fit/defer.h>
#include <lib/pci/constants.h>
#include <lib/pci/hw.h>
#include <lib/zx/interrupt.h>
#include <string.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <algorithm>
#include <optional>
#include <vector>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/pci/cpp/bind.h>
#include <fbl/algorithm.h>
#include <fbl/alloc_checker.h>
#include <fbl/auto_lock.h>
#include <fbl/ref_ptr.h>
#include <fbl/string_buffer.h>

#include "src/devices/pci/drivers/pci/bus_device_interface.h"
#include "src/devices/pci/drivers/pci/capabilities/power_management.h"
#include "src/devices/pci/drivers/pci/composite.h"
#include "src/devices/pci/drivers/pci/ref_counted.h"
#include "src/devices/pci/drivers/pci/upstream_node.h"

#define RETURN_STATUS(level, status, format, ...)                                   \
  do {                                                                              \
    zx_status_t _status = (status);                                                 \
    zxlogf(level, "[%s] %s(" format ") = %s", config()->addr(),                     \
           __FUNCTION__ __VA_OPT__(, ) __VA_ARGS__, zx_status_get_string(_status)); \
    return;                                                                         \
  } while (0)

#define RETURN_DEBUG(status, ...) RETURN_STATUS(DEBUG, status, __VA_ARGS__)
#define RETURN_TRACE(status, ...) RETURN_STATUS(TRACE, status, __VA_ARGS__)

namespace fpci = ::fuchsia_hardware_pci;

namespace fdf {
using namespace fuchsia_driver_framework;
}

namespace pci {

namespace {  // anon namespace.  Externals do not need to know about DeviceImpl

class DeviceImpl : public Device {
 public:
  static zx_status_t Create(zx_device_t* parent, std::unique_ptr<Config>&& cfg,
                            UpstreamNode* upstream, BusDeviceInterface* bdi, bool has_acpi,
                            bool has_devicetree);

  // Implement ref counting, do not let derived classes override.
  PCI_IMPLEMENT_REFCOUNTED;

  // Disallow copying, assigning and moving.
  DISALLOW_COPY_ASSIGN_AND_MOVE(DeviceImpl);

 protected:
  DeviceImpl(zx_device_t* parent, std::unique_ptr<Config>&& cfg, UpstreamNode* upstream,
             BusDeviceInterface* bdi, bool has_acpi, bool has_devicetree)
      : Device(parent, std::move(cfg), upstream, bdi, /*is_bridge=*/false, has_acpi,
               has_devicetree) {}
};

zx_status_t DeviceImpl::Create(zx_device_t* parent, std::unique_ptr<Config>&& cfg,
                               UpstreamNode* upstream, BusDeviceInterface* bdi, bool has_acpi,
                               bool has_devicetree) {
  fbl::AllocChecker ac;
  auto raw_dev =
      new (&ac) DeviceImpl(parent, std::move(cfg), upstream, bdi, has_acpi, has_devicetree);
  if (!ac.check()) {
    zxlogf(ERROR, "[%s] Out of memory attempting to create PCIe device.", cfg->addr());
    return ZX_ERR_NO_MEMORY;
  }

  auto dev = fbl::AdoptRef(static_cast<Device*>(raw_dev));
  const zx_status_t status = raw_dev->Init();
  if (status != ZX_OK) {
    zxlogf(ERROR, "[%s] Failed to initialize PCIe device: %s", dev->config()->addr(),
           zx_status_get_string(status));
    return status;
  }

  bdi->LinkDevice(dev);
  return ZX_OK;
}

}  // namespace

Device::Device(zx_device_t* parent, std::unique_ptr<Config>&& config, UpstreamNode* upstream,
               BusDeviceInterface* bdi, bool is_bridge, bool has_acpi, bool has_devicetree)
    : DeviceType(parent),
      cfg_(std::move(config)),
      upstream_(upstream),
      bdi_(bdi),
      bar_count_(is_bridge ? kBarRegsPerBridge : kMaxBarCount),
      is_bridge_(is_bridge),
      has_acpi_(has_acpi),
      has_devicetree_(has_devicetree) {}

Device::~Device() {
  // We should already be unlinked from the bus's device tree.
  ZX_DEBUG_ASSERT(disabled_);
  ZX_DEBUG_ASSERT(!plugged_in_);

  // Make certain that all bus access (MMIO, PIO, Bus mastering) has been
  // disabled and disable IRQs.
  DisableInterrupts();
  SetBusMastering(false);
  ModifyCmd(/*clr_bits=*/kCommandIoEn | kCommandMemEn, /*set_bits=*/0);
  // TODO(cja/https://fxbug.dev/42108123): Remove this after porting is finished.
  zxlogf(TRACE, "%s [%s] dtor finished", is_bridge() ? "bridge" : "device", cfg_->addr());
}

zx_status_t Device::Create(zx_device_t* parent, std::unique_ptr<Config>&& config,
                           UpstreamNode* upstream, BusDeviceInterface* bdi, bool has_acpi,
                           bool has_devicetree) {
  return DeviceImpl::Create(parent, std::move(config), upstream, bdi, has_acpi, has_devicetree);
}

zx_status_t Device::Init() {
  const fbl::AutoLock dev_lock(&dev_lock_);

  const zx_status_t status = InitLocked();
  if (status != ZX_OK) {
    zxlogf(ERROR, "failed to initialize device %s: %d", cfg_->addr(), status);
    return status;
  }

  // Things went well and the device is in a good state. Flag the device as
  // plugged in and link ourselves up to the graph. This will keep the device
  // alive as long as the Bus owns it.
  upstream_->LinkDevice(this);
  plugged_in_ = true;

  return status;
}

zx_status_t Device::InitInterrupts() {
  zx_status_t status = zx::interrupt::create(*zx::unowned_resource(ZX_HANDLE_INVALID), 0,
                                             ZX_INTERRUPT_VIRTUAL, &irqs_.legacy);
  if (status != ZX_OK) {
    zxlogf(ERROR, "device %s could not create its legacy interrupt: %s", cfg_->addr(),
           zx_status_get_string(status));
    return status;
  }

  // Disable all interrupt modes until a driver enables the preferred method.
  // The legacy interrupt is disabled by hand because our Enable/Disable methods
  // for doing so need to interact with the Shared IRQ lists in Bus.
  ModifyCmdLocked(/*clr_bits=*/0, /*set_bits=*/kCommandIntDisable);
  irqs_.legacy_vector = 0;

  if (caps_.msi) {
    status = DisableMsi();
    if (status != ZX_OK) {
      zxlogf(ERROR, "failed to disable MSI: %s", zx_status_get_string(status));
      return status;
    }
  }

  if (caps_.msix) {
    status = DisableMsix();
    if (status != ZX_OK) {
      zxlogf(ERROR, "failed to disable MSI-X: %s", zx_status_get_string(status));
      return status;
    }
  }

  irqs_.mode = fuchsia_hardware_pci::InterruptMode::kDisabled;
  return ZX_OK;
}

zx_status_t Device::InitLocked() {
  // Cache basic device info
  vendor_id_ = cfg_->Read(Config::kVendorId);
  device_id_ = cfg_->Read(Config::kDeviceId);
  class_id_ = cfg_->Read(Config::kBaseClass);
  subclass_ = cfg_->Read(Config::kSubClass);
  prog_if_ = cfg_->Read(Config::kProgramInterface);
  rev_id_ = cfg_->Read(Config::kRevisionId);
  segment_group_ = bdi()->GetSegmentGroup();

  // Disable the device in event of a failure initializing. TA is disabled
  // because it cannot track the scope of AutoCalls and their associated
  // locking semantics. The lock is grabbed by |Init| and held at this point.
  auto disable = fit::defer([this]() __TA_NO_THREAD_SAFETY_ANALYSIS { DisableLocked(); });

  // Parse and sanity check the capabilities and extended capabilities lists
  // if they exist
  zx_status_t st = ProbeCapabilities();
  if (st != ZX_OK) {
    zxlogf(ERROR, "device %s encountered an error parsing capabilities: %d", cfg_->addr(), st);
    return st;
  }

  ProbeBars();

  // Now that we know what our capabilities are, initialize our internal IRQ
  // bookkeeping and disable all interrupts until a driver requests them.
  st = InitInterrupts();
  if (st != ZX_OK) {
    return st;
  }

  // Power the device on by transitioning to the highest power level if possible
  // and necessary.
  if (caps_.power) {
    if (auto state = caps_.power->GetPowerState(*cfg_);
        state != PowerManagementCapability::PowerState::D0) {
      zxlogf(DEBUG, "[%s] transitioning power state from D%d to D0", cfg_->addr(), state);
      caps_.power->SetPowerState(*cfg_, PowerManagementCapability::PowerState::D0);
    }
  }

  auto endpoints = fidl::CreateEndpoints<fuchsia_io::Directory>();
  if (endpoints.is_error()) {
    return endpoints.status_value();
  }

  auto pci_bind_topo = static_cast<uint32_t>(BIND_PCI_TOPO_PACK(bus_id(), dev_id(), func_id()));

  zx_device_str_prop_t pci_device_props[] = {
      ddk::MakeStrProperty(bind_fuchsia::PCI_VID, static_cast<uint32_t>(vendor_id())),
      ddk::MakeStrProperty(bind_fuchsia::PCI_DID, static_cast<uint32_t>(device_id())),
      ddk::MakeStrProperty(bind_fuchsia::PCI_CLASS, static_cast<uint32_t>(class_id())),
      ddk::MakeStrProperty(bind_fuchsia::PCI_SUBCLASS, static_cast<uint32_t>(subclass())),
      ddk::MakeStrProperty(bind_fuchsia::PCI_INTERFACE, static_cast<uint32_t>(prog_if())),
      ddk::MakeStrProperty(bind_fuchsia::PCI_REVISION, static_cast<uint32_t>(rev_id())),
      ddk::MakeStrProperty(bind_fuchsia::PCI_TOPO, pci_bind_topo),
      ddk::MakeStrProperty(bind_fuchsia_pci::SEGMENT, static_cast<uint32_t>(segment_group())),
  };
  std::array offers = {
      fpci::Service::Name,
  };

  auto bus_info = std::make_unique<fdf::BusInfo>(fdf::BusInfo{{
      .bus = fdf::BusType::kPci,
      .address = fdf::DeviceAddress::WithArrayIntValue({bus_id(), dev_id(), func_id()}),
      // TODO(surajmalhotra): Determine if device is soldered on or removable. For now we only
      // really run on devices with where everything on the pci bus is pretty much permanent.
      .address_stability = fdf::DeviceAddressStability::kStable,
  }});

  outgoing_dir_.emplace(fdf::Dispatcher::GetCurrent()->async_dispatcher());
  zx::result result = outgoing_dir_->AddService<fuchsia_hardware_pci::Service>(
      fuchsia_hardware_pci::Service::InstanceHandler({
          .device = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                            fidl::kIgnoreBindingClosure),
      }));
  if (result.is_error()) {
    zxlogf(ERROR, "Failed to add Service to the outgoing directory: %s", result.status_string());
    return result.status_value();
  }

  result = outgoing_dir_->Serve(std::move(endpoints->server));
  if (result.is_error()) {
    zxlogf(ERROR, "Failed to service the outgoing directory: %s", result.status_string());
    return result.status_value();
  }

  const auto name = std::string(config()->addr());
  zx_status_t status = DdkAdd(ddk::DeviceAddArgs(name.c_str())
                                  .set_str_props(pci_device_props)
                                  .set_bus_info(std::move(bus_info))
                                  .set_flags(DEVICE_ADD_MUST_ISOLATE)
                                  .set_outgoing_dir(endpoints->client.TakeChannel())
                                  .set_fidl_service_offers(offers));
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to create pci device %s: %s", config()->addr(),
           zx_status_get_string(status));
    return status;
  }

  // In DFv1, the DDK manages device lifecycle without RefPtr. Increment our
  // reference count to account for DDK ownership until DdkRelease is called.
  this->AddRef();

  // Devices described by the devicetree get their composite from the devicetree
  // (pci-child-visitor), which already aggregates this fragment with the
  // device's sideband resources. Publishing a composite here too would race the
  // devicetree's composite for the same device, so we stop at the fragment.
  if (has_devicetree()) {
    disable.cancel();
    return ZX_OK;
  }

  auto pci_info = CompositeInfo{
      .vendor_id = vendor_id(),
      .device_id = device_id(),
      .class_id = class_id(),
      .subclass = subclass(),
      .program_interface = prog_if(),
      .revision_id = rev_id(),
      .bus_id = bus_id(),
      .dev_id = dev_id(),
      .func_id = func_id(),
      .has_acpi = has_acpi(),
  };

  char spec_name[8];
  snprintf(spec_name, sizeof(spec_name), "%02x_%02x_%01x", bus_id(), dev_id(), func_id());
  status = DdkAddCompositeNodeSpec(spec_name, CreateCompositeNodeSpec(pci_info));
  if (status != ZX_OK) {
    zxlogf(ERROR, "[%s] Failed to add pci composite spec: %s", config()->addr(),
           zx_status_get_string(status));
    return status;
  }

  disable.cancel();
  return ZX_OK;
}

zx_status_t Device::ModifyCmd(uint16_t clr_bits, uint16_t set_bits) {
  const fbl::AutoLock dev_lock(&dev_lock_);
  // In order to keep internal bookkeeping coherent, and interactions between
  // MSI/MSI-X and Legacy IRQ mode safe, API users may not directly manipulate
  // the legacy IRQ enable/disable bit.  Just ignore them if they try to
  // manipulate the bit via the modify cmd API.
  // TODO(cja) This only applies to PCI(e)
  clr_bits = static_cast<uint16_t>(clr_bits & ~kCommandIntDisable);
  set_bits = static_cast<uint16_t>(set_bits & ~kCommandIntDisable);

  if (plugged_in_) {
    ModifyCmdLocked(clr_bits, set_bits);
    return ZX_OK;
  }

  return ZX_ERR_UNAVAILABLE;
}

void Device::ModifyCmdLocked(uint16_t clr_bits, uint16_t set_bits) {
  fbl::AutoLock cmd_reg_lock(&cmd_reg_lock_);
  cfg_->Write(Config::kCommand,
              static_cast<uint16_t>((cfg_->Read(Config::kCommand) & ~clr_bits) | set_bits));
}

void Device::Disable() {
  const fbl::AutoLock dev_lock(&dev_lock_);
  DisableLocked();
}

void Device::DisableLocked() {
  // Disable a device because we cannot allocate space for all of its BARs (or
  // forwarding windows, in the case of a bridge).  Flag the device as
  // disabled from here on out.
  zxlogf(TRACE, "[%s] %s %s", cfg_->addr(), (is_bridge()) ? " (b)" : "", __func__);

  // Flag the device as disabled.  Close the device's MMIO/PIO windows, shut
  // off device initiated accesses to the bus, disable legacy interrupts.
  // Basically, prevent the device from doing anything from here on out.
  disabled_ = true;
  AssignCmdLocked(kCommandIntDisable);

  // Release all BAR allocations back into the pool they came from.
  for (auto& bar : bars_) {
    bar.reset();
  }
}

zx_status_t Device::SetBusMastering(bool enabled) {
  // Only allow bus mastering to be turned off if the device is disabled.
  if (enabled && disabled_) {
    return ZX_ERR_BAD_STATE;
  }

  ModifyCmdLocked(enabled ? /*clr_bits=*/0 : /*set_bits=*/kCommandBusMasterEn,
                  enabled ? /*clr_bits=*/kCommandBusMasterEn : /*set_bits=*/0);
  return upstream_->SetBusMasteringUpstream(enabled);
}

// Configures the BAR represented by |bar| by writing to its register and configuring
// IO and Memory access accordingly.
zx_status_t Device::WriteBarInformation(const Bar& bar) {
  // Now write the allocated address space to the BAR.
  uint16_t cmd_backup = cfg_->Read(Config::kCommand);
  // Figure out the IO type of the bar and disable that while we adjust the bar address.
  uint16_t mem_io_en_flag = (bar.is_mmio) ? kCommandMemEn : kCommandIoEn;
  ModifyCmdLocked(mem_io_en_flag, cmd_backup);

  cfg_->Write(Config::kBar(bar.bar_id), static_cast<uint32_t>(bar.address));
  if (bar.is_64bit) {
    const uint32_t addr_hi = static_cast<uint32_t>(bar.address >> 32);
    cfg_->Write(Config::kBar(bar.bar_id + 1), addr_hi);
  }
  // Flip the IO bit back on for this type of bar
  AssignCmdLocked(cmd_backup | mem_io_en_flag);
  return ZX_OK;
}

zx::result<> Device::ProbeBar(uint8_t bar_id) {
  if (bar_id >= bar_count_) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  Bar bar{};
  uint32_t bar_val = cfg_->Read(Config::kBar(bar_id));

  bar.bar_id = bar_id;
  bar.is_mmio = (bar_val & kBarIoTypeMask) == kBarIoTypeMmio;
  bar.is_64bit = bar.is_mmio && ((bar_val & kBarMmioTypeMask) == kBarMmioType64Bit);
  bar.is_prefetchable = bar.is_mmio && (bar_val & kBarMmioPrefetchMask);
  const uint32_t addr_mask = (bar.is_mmio) ? kBarMmioAddrMask : kBarPioAddrMask;

  // Check the read-only configuration of the BAR. If it's invalid then don't add it to our BAR
  // list.
  if (bar.is_64bit && (bar.bar_id == bar_count_ - 1)) {
    zxlogf(ERROR, "[%s] has a 64bit bar in invalid position %u!", cfg_->addr(), bar.bar_id);
    return zx::error(ZX_ERR_BAD_STATE);
  }

  if (bar.is_64bit && !bar.is_mmio) {
    zxlogf(ERROR, "[%s] bar %u is 64bit but not mmio!", cfg_->addr(), bar.bar_id);
    return zx::error(ZX_ERR_BAD_STATE);
  }

  // Disable MMIO & PIO access while we perform the probe. We don't want the
  // addresses written during probing to conflict with anything else on the
  // bus. Note: No drivers should have access to this device's registers
  // during the probe process as the device should not have been published
  // yet. That said, there could be other (special case) parts of the system
  // accessing a devices registers at this point in time, like an early init
  // debug console or serial port. Don't make any attempt to print or log
  // until the probe operation has been completed. Hopefully these special
  // systems are quiescent at this point in time, otherwise they might see
  // some minor glitching while access is disabled.
  uint16_t cmd_backup = ReadCmdLocked();
  bool enabled = !!(cmd_backup & (kCommandMemEn | kCommandIoEn));
  if (enabled) {
    ModifyCmdLocked(/*clr_bits=*/kCommandMemEn | kCommandIoEn,
                    /*set_bits=*/cmd_backup);
    // For enabled devices save the original address in the BAR. If the device
    // is enabled then we should assume the bios configured it and we should
    // attempt to retain the BAR allocation.
    bar.address = bar_val & addr_mask;
  }

  // Write ones to figure out the size of the BAR
  cfg_->Write(Config::kBar(bar_id), UINT32_MAX);
  bar_val = cfg_->Read(Config::kBar(bar_id));
  // BARs that are not wired up return all zeroes on read after probing.
  if (bar_val == 0) {
    return zx::ok();
  }

  uint64_t size_mask = ~(bar_val & addr_mask);
  if (bar.is_mmio && bar.is_64bit) {
    // Retain the high 32bits of the 64bit address address if the device was
    // enabled already.
    if (enabled) {
      bar.address |= static_cast<uint64_t>(cfg_->Read(Config::kBar(bar_id + 1))) << 32;
    }

    // Get the high 32 bits of size for the 64 bit BAR by repeating the
    // steps of writing 1s and then reading the value of the next BAR.
    cfg_->Write(Config::kBar(bar_id + 1), UINT32_MAX);
    size_mask |= static_cast<uint64_t>(~cfg_->Read(Config::kBar(bar_id + 1))) << 32;
  } else if (!bar.is_mmio && !(bar_val & (UINT16_MAX << 16))) {
    // Per spec, if the type is IO and the upper 16 bits were zero in the read
    // then they should be removed from the size mask before incrementing it.
    size_mask &= UINT16_MAX;
  }
  // No matter what configuration we've found, |size_mask| should contain a
  // mask representing all the valid bits that can be set in the address.
  bar.size = size_mask + 1;

  // Write the original address value we had before probing and re-enable its
  // access mode now that probing is complete.
  WriteBarInformation(bar);

  bars_[bar_id] = std::move(bar);
  return zx::ok();
}

void Device::ProbeBars() {
  for (uint32_t bar_id = 0; bar_id < bar_count_; bar_id++) {
    auto result = ProbeBar(bar_id);
    if (result.is_error()) {
      zxlogf(ERROR, "[%s] Skipping bar %u due to probing error: %s", cfg_->addr(), bar_id,
             result.status_string());
      continue;
    }

    // If the bar was probed as 64 bit then mark then we can just skip the next bar.
    if (bars_[bar_id] && bars_[bar_id]->is_64bit) {
      bar_id++;
    }
  }
}

// Allocates appropriate address space for BAR |bar| out of any suitable
// upstream allocators, using |base| as the base address if present.
zx::result<std::unique_ptr<PciAllocation>> Device::AllocateFromUpstream(
    const Bar& bar, std::optional<zx_paddr_t> base) {
  ZX_DEBUG_ASSERT(bar.size > 0);
  zx_paddr_t start = base.value_or(0);

  // On all platforms if a BAR is not marked in its register as MMIO then it
  // goes through the Root Host IO/PIO allocator, regardless of whether the
  // platform's IO is actually MMIO backed.
  if (!bar.is_mmio) {
    return upstream_->pio_regions().Allocate(base, bar.size);
  }

  // If a BAR is prefetchable and we're attached to a bridge then the highly preferred
  // allocation option is to use the PF-MMIO window. Otherwise, when
  // attached to a root we can use either MMIO allocator.
  if (upstream_->type() == pci::UpstreamNode::Type::BRIDGE && bar.is_prefetchable) {
    if (auto result = upstream_->pf_mmio_regions().Allocate(base, bar.size); result.is_ok()) {
      return result;
    }
    // Fall back to allocating from non-prefetchable window; this is valid but unoptimal.
    return upstream_->mmio_regions().Allocate(base, bar.size);
  }

  // If the allocation fits within the low MMIO window then attempt to allocate
  // it there. Ensure it can't cross the low to high boundary between 4GB and
  // beyond. Any prefetchable BARs at this point are downstream of a root so it
  // doesn't matter which allocator we use for prefetchability specifically.
  // It's worth noting that if a BAR did not have an existing allocation then
  // its start address will be 0, so we'll always try to allocate from the low
  // MMIO allocator first in that case.
  zx_paddr_t end_offset = 0;
  if (!add_overflow(start, bar.size - 1, &end_offset) &&
      end_offset <= std::numeric_limits<uint32_t>::max()) {
    if (auto result = upstream_->mmio_regions().Allocate(base, bar.size); result.is_ok()) {
      return result.take_value();
    }
  }

  // Otherwise, try to use the high MMIO allocator.
  return upstream_->pf_mmio_regions().Allocate(base, bar.size);
}

// Higher level method to allocate address space a previously probed BAR id
// |bar_id| and handle configuration space setup.
zx::result<> Device::AllocateBar(uint8_t bar_id) {
  ZX_DEBUG_ASSERT(upstream_);
  ZX_DEBUG_ASSERT(bar_id < bar_count_);
  ZX_DEBUG_ASSERT(bars_[bar_id].has_value());

  Bar& bar = *bars_[bar_id];
  // First try to allocate any address that we found during the probe. If it
  // fails then log it because it most likely failed due to an expected address
  // region being in use already. If the address is zero due to being
  // uninitialized when PCI comes up then we can skip this step because we know
  // we will never be able to allocate from address 0 in any address space type.
  zx::result<std::unique_ptr<PciAllocation>> result;
  if (bar.address) {
    result = AllocateFromUpstream(bar, bar.address);
  }

  // If the previous allocation failed, or result has been unused, then try to
  // reallocate from any allocator at any location.
  if (!result.is_ok()) {
    result = AllocateFromUpstream(bar, std::nullopt);
  }

  if (result.is_error()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  bar.allocation = std::move(result.value());
  bar.address = bar.allocation->base();
  WriteBarInformation(bar);

  return zx::ok();
}

zx::result<> Device::AllocateBars() {
  const fbl::AutoLock dev_lock(&dev_lock_);
  ZX_DEBUG_ASSERT(plugged_in_);
  ZX_DEBUG_ASSERT(bar_count_ <= bars_.max_size());

  std::vector<uint32_t> bar_allocation_order;
  bar_allocation_order.reserve(kMaxBarCount);
  for (uint32_t i = 0; i < bars_.size(); ++i) {
    if (bars_[i]) {
      ZX_DEBUG_ASSERT(bars_[i]->bar_id == i);
      bar_allocation_order.push_back(i);
    }
  }

  // Sort BARs by size descending so the largest BAR will be allocated first. This avoids a smaller
  // BAR fragmenting the window and preventing the larger BAR from being allocated.
  std::ranges::sort(bar_allocation_order, [this](uint32_t a, uint32_t b) {
    []() __TA_ASSERT(dev_lock_) {}();
    return bars_[a]->size > bars_[b]->size;
  });

  // Ensure we allocate BARs that already have an assigned address first, in
  // case it lines up with the allocators that we might use an address in a
  // lower BAR that is already assigned to a later BAR.
  std::ranges::stable_partition(bar_allocation_order, [this](uint32_t i) {
    []() __TA_ASSERT(dev_lock_) {}();
    return bars_[i]->address != 0;
  });

  // Allocate BARs for the device
  for (auto bar_id : bar_allocation_order) {
    if (auto result = AllocateBar(bar_id); result.is_error()) {
      zxlogf(ERROR, "[%s] failed to allocate bar %u: %s", cfg_->addr(), bar_id,
             result.status_string());
      return result.take_error();
    }
  }

  return zx::ok();
}

zx::result<PowerManagementCapability::PowerState> Device::GetPowerState() {
  const fbl::AutoLock dev_lock(&dev_lock_);
  if (!caps_.power) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  return zx::ok(caps_.power->GetPowerState(*cfg_));
}

void Device::Unplug() {
  zxlogf(TRACE, "[%s] %s %s", cfg_->addr(), (is_bridge()) ? " (b)" : "", __func__);
  const fbl::AutoLock dev_lock(&dev_lock_);
  // Disable should have been called before Unplug and would have disabled
  // everything in the command register
  ZX_DEBUG_ASSERT(disabled_);
  upstream_->UnlinkDevice(this);
  // After unplugging from the Bus there should be no further references to this
  // device and the dtor will be called.
  bdi_->UnlinkDevice(this);
  plugged_in_ = false;
  zxlogf(TRACE, "device [%s] unplugged", cfg_->addr());
}

void Device::DdkUnbind(ddk::UnbindTxn txn) { txn.Reply(); }

void Device::DdkRelease() {
  bindings_.CloseAll(ZX_OK);
  outgoing_dir_.reset();
  // Release the reference held for DDK ownership and destroy the device if no
  // other references remain.
  if (Release()) {
    delete this;
  }
}

void Device::Bind(fidl::ServerEnd<fuchsia_hardware_pci::Device> request) {
  fidl::BindServer(fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(request), this);
}

void Device::GetDeviceInfo(GetDeviceInfoCompleter::Sync& completer) {
  completer.Reply({.vendor_id = vendor_id(),
                   .device_id = device_id(),
                   .base_class = class_id(),
                   .sub_class = subclass(),
                   .program_interface = prog_if(),
                   .revision_id = rev_id(),
                   .bus_id = bus_id(),
                   .dev_id = dev_id(),
                   .func_id = func_id()});
  RETURN_DEBUG(ZX_OK, "");
}

void Device::GetBar(GetBarRequestView request, GetBarCompleter::Sync& completer) {
  if (request->bar_id >= pci::kMaxBarCount) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    RETURN_DEBUG(ZX_ERR_INVALID_ARGS, "%u", request->bar_id);
  }

  fbl::AutoLock dev_lock(&dev_lock_);
  auto& bar = bars_[request->bar_id];
  if (!bar) {
    completer.ReplyError(ZX_ERR_NOT_FOUND);
    RETURN_DEBUG(ZX_ERR_NOT_FOUND, "%u", request->bar_id);
  }

  size_t bar_size = bar->size;

  ZX_DEBUG_ASSERT(bar->allocation);
  switch (bar->allocation->type()) {
    case PCI_ADDRESS_SPACE_MEMORY: {
      zx::result<zx::vmo> result = bar->allocation->CreateVmo();
      if (result.is_ok()) {
        completer.ReplySuccess(
            {.bar_id = request->bar_id,
             .size = bar_size,
             .result = fpci::wire::BarResult::WithVmo(std::move(result.value()))});
        RETURN_DEBUG(ZX_OK, "%u", request->bar_id);
      }
    } break;
    case PCI_ADDRESS_SPACE_IO: {
      zx::result<zx::resource> result = bar->allocation->CreateResource();
      if (result.is_ok()) {
        fidl::Arena arena;
        completer.ReplySuccess(
            {.bar_id = request->bar_id,
             .size = bar_size,
             .result = fpci::wire::BarResult::WithIo(
                 arena, fuchsia_hardware_pci::wire::IoBar{.address = bar->address,
                                                          .resource = std::move(result.value())})});
        RETURN_DEBUG(ZX_OK, "%u", request->bar_id);
      }
    } break;
  }

  completer.ReplyError(ZX_ERR_BAD_STATE);
  RETURN_DEBUG(ZX_ERR_BAD_STATE, "%u", request->bar_id);
}

void Device::SetBusMastering(SetBusMasteringRequestView request,
                             SetBusMasteringCompleter::Sync& completer) {
  fbl::AutoLock dev_lock(&dev_lock_);
  zx_status_t status = SetBusMastering(request->enabled);
  if (status != ZX_OK) {
    completer.ReplyError(status);
    RETURN_DEBUG(status, "");
  }

  completer.ReplySuccess();
  RETURN_DEBUG(status, "");
}

void Device::ResetDevice(ResetDeviceCompleter::Sync& completer) {
  completer.Reply(zx::error(ZX_ERR_NOT_SUPPORTED));
  RETURN_DEBUG(ZX_ERR_NOT_SUPPORTED, "");
}

void Device::AckInterrupt(AckInterruptCompleter::Sync& completer) {
  fbl::AutoLock dev_lock(&dev_lock_);
  zx_status_t status = AckLegacyIrq();
  if (status != ZX_OK) {
    completer.ReplyError(status);
    return;
  }
  completer.ReplySuccess();
}

void Device::MapInterrupt(MapInterruptRequestView request, MapInterruptCompleter::Sync& completer) {
  zx::result<zx::interrupt> result = MapInterrupt(request->which_irq);
  if (result.is_error()) {
    completer.ReplyError(result.status_value());
    RETURN_DEBUG(result.status_value(), "%#x", request->which_irq);
  }

  completer.ReplySuccess(std::move(result.value()));
  RETURN_DEBUG(result.status_value(), "%#x", request->which_irq);
}

void Device::SetInterruptMode(SetInterruptModeRequestView request,
                              SetInterruptModeCompleter::Sync& completer) {
  zx_status_t status = SetIrqMode(request->mode, request->requested_irq_count);
  if (status != ZX_OK) {
    completer.ReplyError(status);
    RETURN_DEBUG(status, "%u, %#x", static_cast<uint8_t>(request->mode),
                 request->requested_irq_count);
  }

  completer.ReplySuccess();
  RETURN_DEBUG(status, "%u, %#x", static_cast<uint8_t>(request->mode),
               request->requested_irq_count);
}

void Device::GetInterruptModes(GetInterruptModesCompleter::Sync& completer) {
  pci_interrupt_modes_t modes = GetInterruptModes();
  completer.Reply({.has_legacy = modes.has_legacy,
                   .msi_count = modes.msi_count,
                   .msix_count = modes.msix_count});
  RETURN_DEBUG(ZX_OK, "");
}

void Device::ReadConfig8(ReadConfig8RequestView request, ReadConfig8Completer::Sync& completer) {
  auto result = ReadConfig<uint8_t, PciReg8>(request->offset);
  if (result.is_error()) {
    completer.ReplyError(result.status_value());
    RETURN_DEBUG(result.status_value(), "%#x", request->offset);
  }

  completer.ReplySuccess(result.value());
  RETURN_TRACE(result.status_value(), "%#x", request->offset);
}

void Device::ReadConfig16(ReadConfig16RequestView request, ReadConfig16Completer::Sync& completer) {
  auto result = ReadConfig<uint16_t, PciReg16>(request->offset);
  if (result.is_error()) {
    completer.ReplyError(result.status_value());
    RETURN_DEBUG(result.status_value(), "%#x", request->offset);
  }

  completer.ReplySuccess(result.value());
  RETURN_TRACE(result.status_value(), "%#x", request->offset);
}

void Device::ReadConfig32(ReadConfig32RequestView request, ReadConfig32Completer::Sync& completer) {
  auto result = ReadConfig<uint32_t, PciReg32>(request->offset);
  if (result.is_error()) {
    completer.ReplyError(result.status_value());
    RETURN_DEBUG(result.status_value(), "%#x", request->offset);
  }

  completer.ReplySuccess(result.value());
  RETURN_TRACE(result.status_value(), "%#x", request->offset);
}

void Device::WriteConfig8(WriteConfig8RequestView request, WriteConfig8Completer::Sync& completer) {
  zx_status_t status = WriteConfig<uint8_t, PciReg8>(request->offset, request->value);
  if (status != ZX_OK) {
    completer.ReplyError(status);
    RETURN_DEBUG(status, "%#x, %#x", request->offset, request->value);
  }

  completer.ReplySuccess();
  RETURN_TRACE(status, "%#x, %#x", request->offset, request->value);
}

void Device::WriteConfig16(WriteConfig16RequestView request,
                           WriteConfig16Completer::Sync& completer) {
  zx_status_t status = WriteConfig<uint16_t, PciReg16>(request->offset, request->value);
  if (status != ZX_OK) {
    completer.ReplyError(status);
    RETURN_DEBUG(status, "%#x, %#x", request->offset, request->value);
  }

  completer.ReplySuccess();
  RETURN_TRACE(status, "%#x, %#x", request->offset, request->value);
}

void Device::WriteConfig32(WriteConfig32RequestView request,
                           WriteConfig32Completer::Sync& completer) {
  zx_status_t status = WriteConfig<uint32_t, PciReg32>(request->offset, request->value);
  if (status != ZX_OK) {
    completer.ReplyError(status);
    RETURN_DEBUG(status, "%#x, %#x", request->offset, request->value);
  }

  completer.ReplySuccess();
  RETURN_TRACE(status, "%#x, %#x", request->offset, request->value);
}

void Device::GetCapabilities(GetCapabilitiesRequestView request,
                             GetCapabilitiesCompleter::Sync& completer) {
  std::vector<uint8_t> capabilities;
  {
    fbl::AutoLock dev_lock(&dev_lock_);
    for (const auto& capability : caps_.list) {
      if (capability->id() == static_cast<uint8_t>(request->id)) {
        capabilities.push_back(capability->base());
      }
    }
  }

  completer.Reply(::fidl::VectorView<uint8_t>::FromExternal(capabilities));
  RETURN_DEBUG(ZX_OK, "%#x", static_cast<uint8_t>(request->id));
}

void Device::GetExtendedCapabilities(GetExtendedCapabilitiesRequestView request,
                                     GetExtendedCapabilitiesCompleter::Sync& completer) {
  std::vector<uint16_t> ext_capabilities;
  {
    fbl::AutoLock dev_lock(&dev_lock_);
    for (const auto& ext_capability : caps_.ext_list) {
      if (ext_capability->id() == static_cast<uint16_t>(request->id)) {
        ext_capabilities.push_back(ext_capability->base());
      }
    }
  }

  completer.Reply(::fidl::VectorView<uint16_t>::FromExternal(ext_capabilities));
  RETURN_DEBUG(ZX_OK, "%#x", static_cast<uint16_t>(request->id));
}

void Device::GetBti(GetBtiRequestView request, GetBtiCompleter::Sync& completer) {
  fbl::AutoLock dev_lock(&dev_lock_);
  zx::bti bti;
  zx_status_t status = bdi_->GetBti(this, request->index, &bti);
  if (status != ZX_OK) {
    completer.ReplyError(status);
    RETURN_DEBUG(status, "%u", request->index);
  }

  completer.ReplySuccess(std::move(bti));
  RETURN_DEBUG(status, "%u", request->index);
}

}  // namespace pci
