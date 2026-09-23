// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TLS_H_
#define LIB_LD_TLS_H_

#include <bit>
#include <cassert>
#include <cstddef>
#include <ranges>

#include "abi.h"

namespace ld {
namespace abi {

// This describes the details gleaned from the PT_TLS header for a module.
// These are stored in an array indexed by TLS module ID number - 1, as the
// module ID number zero is never used.
//
// Note that while module ID number 1 is most often the main executable, that
// need not always be so: if the main executable has no PT_TLS of its own, then
// the earliest module loaded that does have a PT_TLS gets module ID 1.
//
// What is importantly special about the main executable is that offsets in the
// static TLS block are chosen with the main executable first--it may have been
// linked with Local Exec TLS access code where the linker chose its expected
// offsets at static link time.  When the dynamic linker follows the usual
// procedure of assigning module IDs in load order and then doing static TLS
// layout in the same order, it always comes out the same.  But the only real
// constraint on the runtime layout chosen is that if the main executable has a
// PT_TLS segment, it must be first and its offset from the thread pointer must
// be the fixed value prescribed by the psABI.  The adjacent private portions
// of the runtime thread descriptor must be located such that both their own
// alignment requirements and the p_align of module 1's PT_TLS are respected.

template <class Elf, class AbiTraits>
struct Abi<Elf, AbiTraits>::TlsModule {
  constexpr Elf::size_type tls_size() const { return tls_initial_data.size() + tls_bss_size; }

  // Initial data image in memory, usually a pointer into the RODATA or RELRO
  // segment of the module's load image.
  Span<const std::byte> tls_initial_data;

  // If the module has a PT_TLS, its total size in memory (for each thread) is
  // determined by the initial data (tls_initial_data.size_bytes(), from .tdata
  // et al) plus this size of zero-initialized bytes (from .tbss et al).
  Addr tls_bss_size = 0;

  // The runtime memory for each thread's copy of the initialized PT_TLS data
  // for this segment must have at least this minimum alignment (p_align).
  // This is validated to be a power of two before the module is loaded.
  Addr tls_alignment = 0;

  // <lib/ld/remote-abi-transcriber.h> introspection API.

  using AbiLocal = Abi<Elf, elfldltl::LocalAbiTraits>::TlsModule;

  template <template <class...> class Template>
  using AbiBases = Template<>;

  template <template <auto...> class Template>
  using AbiMembers =
      Template<&TlsModule::tls_initial_data, &TlsModule::tls_bss_size, &TlsModule::tls_alignment>;
};

// This is the symbol that compilers generate calls to for GD/LD TLS accesses
// in the original ABI (without TLSDESC).  Its linkage name is known to the
// compiler and the linker.  This is not actually implemented by ld.so, but
// must be supplied by something in the dependency graph of a program that uses
// old-style TLS.  The implementation in libc or libdl or suchlike can use the
// `_ld_abi.static_tls_offsets` data to handle TLS module IDs in the
// initial-exec set, e.g. via ld::TlsInitialExecOffset (see below).
extern "C" void* __tls_get_addr(const elfldltl::Elf<>::TlsGetAddrGot<>& got);

// The standard symbol name with hash value cached statically.
inline constexpr elfldltl::SymbolName kTlsGetAddrSymbol{"__tls_get_addr"};

}  // namespace abi

// Fetch the current thread pointer with the given byte offset.
// With a second argument, use that instead of the current thread pointer.
template <typename T = void>
inline T* TpRelative(ptrdiff_t offset = 0, void* tp = __builtin_thread_pointer()) {
  return reinterpret_cast<T*>(static_cast<std::byte*>(tp) + offset);
}

// Return the given pointer's byte offset from the thread pointer.
// `TpRelative(TpRelativeToOffset(ptr)) == ptr` should always be true.
template <typename T>
inline ptrdiff_t TpRelativeToOffset(T* ptr) {
  std::byte* tp = TpRelative<std::byte>();
  return reinterpret_cast<const std::byte*>(ptr) - tp;
}

// Interrogate the passive ABI (e.g. ld::abi::_ld_abi) for the thread-pointer
// offset of each thread's static TLS data area for the given TLS module ID
// among the initial-exec set of TLS modules.
template <class Elf, class AbiTraits>
constexpr ptrdiff_t TlsInitialExecOffset(const typename abi::Abi<Elf, AbiTraits>& abi,
                                         typename Elf::size_type modid) {
  // The offset is stored as unsigned, but is actually signed.
  const size_t offset = abi.static_tls_offsets[modid - 1];
  return std::bit_cast<ptrdiff_t>(offset);
}

// Populate a static TLS segment for the given module in one thread.  The size
// of the segment must match .tls_size().
template <class Module>
constexpr void TlsModuleInit(const Module& module, std::span<std::byte> segment,
                             bool known_zero = false) {
  std::span<const std::byte> initial_data = module.tls_initial_data;
  if (!initial_data.empty()) {
    std::span dest = segment.subspan(0, initial_data.size());
    segment = segment.subspan(dest.size());
    memcpy(dest.data(), initial_data.data(), dest.size());
  }
  assert(segment.size() >= module.tls_bss_size);
  if (module.tls_bss_size != 0 && !known_zero) {
    memset(segment.data(), 0, module.tls_bss_size);
  }
}

// This describes a single module's PT_TLS and its segment in a given thread.
template <class Elf, class AbiTraits>
struct TlsSegment {
  using TlsModule = abi::Abi<Elf, AbiTraits>::TlsModule;

  const TlsModule& module;       // Points to initial data for new threads.
  std::span<std::byte> segment;  // Points to one thread's own data.
};

// This returns an input range of TlsSegment objects for each module that has a
// PT_TLS segment.  The span passed should cover the whole area allocated for
// static TLS data (and the like) for a given thread.  The offset should be the
// location within that span where the thread pointer will point (which may be
// at the end of the span for x86 negative TLS offsets).
template <class Elf, class AbiTraits>
inline std::ranges::input_range auto TlsInitialExecSegments(
    const typename abi::Abi<Elf, AbiTraits>& abi, std::span<std::byte> block, size_t tp_offset) {
  using size_type = Elf::size_type;
  return std::views::iota(size_type{1}, abi.static_tls_modules.size() + 1) |
         std::views::transform([&abi, block, tp_offset](size_type modid) {
           const auto& module = abi.static_tls_modules[modid - 1];
           const ptrdiff_t offset = TlsInitialExecOffset(abi, modid);
           const size_t start = tp_offset + offset;
           assert(start <= block.size_bytes());
           assert(block.size_bytes() - start >= module.tls_size());
           std::span segment = block.subspan(start, module.tls_size());
           return TlsSegment<Elf, AbiTraits>{module, segment};
         });
}

// Populate the static TLS block with initial data and zero'd tbss regions for
// each module that has a PT_TLS segment.
template <class Elf, class AbiTraits>
inline void TlsInitialExecDataInit(const typename abi::Abi<Elf, AbiTraits>& abi,
                                   std::span<std::byte> block, size_t tp_offset,
                                   bool known_zero = false) {
  auto tls_segments = TlsInitialExecSegments(abi, block, tp_offset);
  for (const auto& [module, segment] : tls_segments) {
    TlsModuleInit(module, segment, known_zero);
  }
}

// Interrogate the passive ABI (e.g. ld::abi::_ld_abi) to locate the current
// thread's TLS data area for the given TLS module ID among the initial-exec
// set of TLS modules.
template <class Elf, class AbiTraits>
inline void* TlsInitialExecData(const typename abi::Abi<Elf, AbiTraits>& abi,
                                typename Elf::size_type modid) {
  if (modid == 0) {
    return nullptr;
  }

  return TpRelative(TlsInitialExecOffset(abi, modid));
}

}  // namespace ld

#endif  // LIB_LD_TLS_H_
