// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_ENVIRONMENT_H_
#define SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_ENVIRONMENT_H_

#include <lib/zx/exception.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <string.h>
#include <unistd.h>
#include <zircon/status.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls/debug.h>
#include <zircon/syscalls/exception.h>
#include <zircon/types.h>

#include <map>
#include <memory>
#include <optional>
#include <vector>

#include <bringup/lib/restricted-machine/internal/common.h>
#include <bringup/lib/restricted-machine/machine-type.h>
#include <bringup/lib/restricted-machine/register-state.h>
#include <fbl/ref_ptr.h>
#include <region-alloc/region-alloc.h>

namespace restricted_machine {

// Forward declare LoadableBlob to avoid pulling in all its dependencies.
namespace internal {
class LoadableBlob;

// Help unique_ptr out with the forward declaration.
class LoadableBlobDeleter {
 public:
  void operator()(LoadableBlob* b);
};

}  // namespace internal

// restricted_machine::Environment provides the necessary environment for
// restricted machine computation to occur. It is responsible for:
// - ElfMachine-appropriate mapped and loaded ELF binary blobs.
// - Symbol resolution for loaded blobs.
// - Accessible memory mapping and allocation.
// - Hardware support checking.
//
// Environments may be reused across multiple Machine instances, but any
// writable memory shared between them must be managed by the Machine caller
// or code run by the Machine-itself.
class Environment : public fbl::RefCounted<Environment> {
 public:
  Environment() : machine_(MachineType::kNative) {}
  virtual ~Environment();

  // The default size of the shared memory pool allocated for the environment.
  constexpr static size_t kDefaultMemoryPoolSize{4096 * 12};

  // Confirms that restricted mode is supported for the target machine
  // architecture.
  static bool HardwareSupported(const MachineType& machine);

  // Initializes the environment for a given machine type.
  //
  // This sets up a shared memory VMO that can be used for allocating memory
  // accessible to code running within the restricted machine.
  //
  // |machine|: The target machine architecture. Defaults to the running
  // architecture.
  // |shared_mem_size|: The size of the shared memory pool to allocate.
  // |address_limit|: The upper bound on memory addresses accessible within the
  // environment.
  //
  bool Initialize(MachineType machine = MachineType::kNative,
                  size_t shared_mem_size = kDefaultMemoryPoolSize, uint64_t address_limit = 0);

  // Loads and maps a blob from a VMO, exposing all discoverable symbols.
  //
  // |vmo_name|: The name of the VMO to load.
  // |map_at|: An optional address to map the blob at.
  zx::result<> AddLoadableBlob(const std::string_view& vmo_name,
                               std::optional<zx_vaddr_t> map_at = std::nullopt);

  // Loads and maps a blob from a VMO, exposing only the requested symbols.
  //
  // |vmo_name|: The name of the VMO to load.
  // |symbols|: A list of symbols to expose.
  // |export_symbols|: If true, the symbols are exported for other blobs to use.
  // |map_at|: An optional address to map the blob at.
  zx::result<> AddLoadableBlob(const std::string_view& vmo_name,
                               const std::vector<std::string_view>& symbols,
                               bool export_symbols = false,
                               std::optional<zx_vaddr_t> map_at = std::nullopt);

  // An RAII wrapper for a memory allocation within the shared memory pool.
  using Allocation = RegionAllocator::Region::UPtr;

  // A deleter for arguments allocated in the shared memory pool.
  // This just provides a means to keep the region allocation unique_ptr
  // alive for the lifetime of the allocation unique_ptr.
  template <typename T>
  struct ArgumentDeleter {
    ArgumentDeleter(Allocation&& region) : region_(std::move(region)) {}
    void operator()(T* object) const {}
    Allocation region_{};
  };

  // A unique_ptr for an argument allocated in the shared memory pool.
  template <typename T>
  using Argument = std::unique_ptr<T, ArgumentDeleter<T>>;

  // Allocates a region of memory from the shared memory pool.
  //
  // |size|: The size of the allocation.
  zx::result<Allocation> Allocate(size_t size);

  // Allocates and constructs an object in the shared memory pool.
  //
  // Returns a zx::result containing an Argument<T> on success.
  template <typename T, typename... Args>
  zx::result<Argument<T>> MakeArgumentResult(Args... args) {
    zx::result<Allocation> alloc = Allocate(sizeof(T));
    if (alloc.is_error()) {
      return alloc.take_error();
    }
    auto region = std::move(alloc.value());
    T* t = new (reinterpret_cast<void*>(region->base)) T(args...);
    Argument<T> argument(t, ArgumentDeleter<T>(std::move(region)));
    return zx::ok(std::move(argument));
  }

  // A convenience wrapper for MakeArgumentResult that asserts on failure.
  template <typename T, typename... Args>
  Argument<T> MakeArgument(Args... args) {
    auto result = MakeArgumentResult<T>(args...);
    ZX_ASSERT(result.is_ok());
    return std::move(result.value());
  }

  // Returns the address of a symbol.
  //
  // |name|: The name of the symbol to look up.
  zx::result<uint64_t> SymbolAddress(std::string_view name);

  // Returns the "path" to the loadable .so which is requested from the loader
  // using the given client-specific |prefix|.
  std::string GetLoadableBlobPath(const std::string_view& prefix) const;

  // Returns the machine type of the environment.
  MachineType machine() const { return machine_; }

  // Returns the address limit of the environment.
  uint64_t address_limit() const { return address_limit_; }

  // The name of the "ping" function, used for testing.
  constexpr static std::string_view kPingFunctionName{"ping"};
  // The name of the "caller" thunk function.
  constexpr static std::string_view kThunkFunctionName{"caller"};
  // The prefix of the caller loadable blob.
  constexpr static std::string_view kCallerBlobName{"caller"};

 private:
  MachineType machine_{MachineType::kNone};
  uint64_t address_limit_ = 0;

  zx::vmo shared_vmo_;
  zx_vaddr_t shared_mem_ = 0;
  uint64_t shared_mem_size_ = 0;
  std::unique_ptr<RegionAllocator> alloc_{nullptr};
  std::map<void*, Allocation> arguments_{};

  // Contains all mapped blobs.
  std::vector<std::unique_ptr<internal::LoadableBlob, internal::LoadableBlobDeleter>> blobs_;
  // Contains the unified set of symbol to address mappings.
  std::unordered_map<std::string_view, uint64_t> symbol_map_{};
  // Contains the internal symbols only.
  std::unordered_map<std::string_view, uint64_t> internal_symbol_map_{};

  constexpr static std::string_view kRestrictedCodePath{"restricted-loadable/"};
  constexpr static std::string_view kRestrictedCodeSeparator{"."};
  constexpr static std::string_view kRestrictedCodeSuffix{"so"};

  constexpr static std::array<std::string_view, 2> kRestrictedCodeSymbols{kThunkFunctionName,
                                                                          kPingFunctionName};
};

}  // namespace restricted_machine
#endif  // SRC_BRINGUP_LIB_RESTRICTED_MACHINE_INCLUDE_BRINGUP_LIB_RESTRICTED_MACHINE_ENVIRONMENT_H_
