// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/vmo.h>
#include <zircon/status.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls.h>

#include <bringup/lib/restricted-machine/environment.h>
#include <bringup/lib/restricted-machine/internal/common.h>
#include <bringup/lib/restricted-machine/internal/loadable-blob.h>

#ifdef __ARM_ACLE
#include <arm_acle.h>
#endif

namespace restricted_machine {

namespace internal {
void LoadableBlobDeleter::operator()(LoadableBlob* b) {
  if (b) {
    delete b;
  }
}
}  // namespace internal

Environment::~Environment() {
  if (zx_restricted_unbind_state) {
    std::ignore = zx_restricted_unbind_state(0);
  }
}

bool Environment::Initialize(MachineType machine, size_t shared_mem_size, uint64_t address_limit) {
  machine_ = machine;
  if (!HardwareSupported(machine)) {
    return false;
  }
  address_limit_ = address_limit;
  shared_mem_size_ = shared_mem_size;

  // Compute the appropriate address limit.
  uint64_t register_size = RegisterStateFactory::Create(machine_)->register_bytes();
  if (register_size < sizeof(uint64_t)) {
    uint64_t hardware_limit = (1ULL << (register_size * CHAR_BIT - 1));
    hardware_limit += hardware_limit - 1;
    if (address_limit_ == 0) {
      address_limit_ = hardware_limit;
    } else if (hardware_limit < address_limit_) {
      address_limit_ = hardware_limit;
    }
  }

  // Allocate and map space for memory shared between normal and restricted
  // mode, while ensuring it is under any address limit.
  if (zx::vmo::create(shared_mem_size_, 0, &shared_vmo_) != ZX_OK) {
    RM_LOG(ERROR) << "failed to create shared page";
    return false;
  }
  auto options = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE;
  auto offset = address_limit_;
  auto page_size = zx_system_get_page_size();
  if (address_limit_ != 0) {
    options |= ZX_VM_OFFSET_IS_UPPER_LIMIT;
    zx_info_vmar_t vmar_info = {};
    ZX_ASSERT(zx::vmar::root_self()->get_info(ZX_INFO_VMAR, &vmar_info, sizeof(vmar_info), nullptr,
                                              nullptr) == ZX_OK);
    ZX_ASSERT(offset > vmar_info.base + shared_mem_size_);
    // Subtract the base from the absolute offset to get the relative offset
    // needed for zx_vmar_map().
    offset -= vmar_info.base;
    // Align it to the nearest page.
    offset -= offset % page_size;
  }
  if (zx::vmar::root_self()->map(options, offset, shared_vmo_, 0, shared_mem_size_, &shared_mem_) !=
      ZX_OK) {
    RM_LOG(ERROR) << "failed to map shared memory";
    return false;
  }
  // The RegionAllocator provides dynamic memory allocation out of the mapped pool.
  alloc_ = std::make_unique<RegionAllocator>(RegionAllocator::RegionPool::Create(shared_mem_size_));
  // Add the shared memory allocation as the only available region.
  // If needed, separable regions can be made for TLS, atomic, stack, and args.
  alloc_->AddRegion({.base = shared_mem_, .size = shared_mem_size_});

  if (auto result = AddLoadableBlob(kCallerBlobName,
                                    std::vector<std::string_view>(kRestrictedCodeSymbols.begin(),
                                                                  kRestrictedCodeSymbols.end()),
                                    false);
      result.is_error()) {
    RM_LOG(ERROR) << "failed to load blob prefixed '" << kCallerBlobName << "' with error "
                  << zx_status_get_string(result.error_value());
    return false;
  }
  // Don't allow subsequent blobs to clobber internal symbols.
  // Similarly, do not pre-resolve them in user loadables.
  internal_symbol_map_ = symbol_map_;
  symbol_map_.clear();
  RM_LOG(DEBUG) << "initialized";

  return true;
}

zx::result<> Environment::AddLoadableBlob(const std::string_view& vmo_prefix,
                                          std::optional<zx_vaddr_t> map_at) {
  return AddLoadableBlob(vmo_prefix, {}, true, map_at);
}

std::string Environment::GetLoadableBlobPath(const std::string_view& prefix) const {
  return std::string(kRestrictedCodePath) + std::string(prefix) +
         std::string(kRestrictedCodeSeparator) + std::string(machine_.AsString()) +
         std::string(kRestrictedCodeSeparator) + std::string(kRestrictedCodeSuffix);
}

zx::result<> Environment::AddLoadableBlob(const std::string_view& vmo_prefix,
                                          const std::vector<std::string_view>& symbols,
                                          bool export_symbols, std::optional<zx_vaddr_t> map_at) {
  std::unique_ptr<internal::LoadableBlob, internal::LoadableBlobDeleter> blob{
      new internal::LoadableBlob(), internal::LoadableBlobDeleter()};
  // Build a full blob name from the provided prefix by appending ${target_cpu}.so.
  std::string vmo_name = GetLoadableBlobPath(vmo_prefix);

  // Prior symbols are added automatically.
  auto result = blob->Initialize(vmo_name, machine_.AsElfMachine(), address_limit_, symbols,
                                 symbol_map_, export_symbols, map_at);
  if (result.is_error()) {
    return result.take_error();
  }
  auto new_symbols = blob->symbols().symbol_map();
  // Add (or override) the existing symbols with those found in the new map.
  for (const auto& sym : new_symbols) {
    RM_LOG(DEBUG) << "Updating symbol map with " << sym.first;
    symbol_map_[sym.first] = sym.second;
  }

  blobs_.push_back(std::move(blob));
  return zx::ok();
}

bool Environment::HardwareSupported(const MachineType& machine) {
  // If the machine type matches the running software, we don't need to
  // check hardware capabilities.
  if (machine == MachineType::kNative) {
    return true;
  }
  // For alternative execution modes, we will need to use the
  // architecture-specific checks to be sure we can run a restricted
  // mode environment properly.
  for (const MachineType& mtype : kSupportedMachines) {
    if (machine == mtype) {
      return RegisterStateFactory::Create(machine)->ArchSupported();
    }
  }
  return false;
}

zx::result<Environment::Allocation> Environment::Allocate(size_t size) {
  Allocation region;
  zx_status_t result = alloc_->GetRegion(size, region);
  if (result != ZX_OK) {
    return zx::error(result);
  }
  uint64_t base_start = reinterpret_cast<uint64_t>(shared_mem_);
  ZX_ASSERT(region->size == size);
  ZX_ASSERT(region->base >= base_start);
  ZX_ASSERT((region->base + region->size) <= (shared_mem_ + shared_mem_size_));
  memset(reinterpret_cast<uint8_t*>(region->base), 0, region->size);
  return zx::ok(std::move(region));
}

zx::result<uint64_t> Environment::SymbolAddress(std::string_view name) {
  auto internal_addr = internal_symbol_map_.find(name);
  if (internal_addr != internal_symbol_map_.end()) {
    return zx::ok(internal_addr->second);
  }

  auto addr = symbol_map_.find(name);
  if (addr == symbol_map_.end()) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }
  return zx::ok(addr->second);
}

}  // namespace restricted_machine
