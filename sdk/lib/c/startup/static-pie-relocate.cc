// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/diagnostics.h>
#include <lib/elfldltl/machine.h>
#include <lib/elfldltl/phdr.h>
#include <lib/elfldltl/relro.h>
#include <lib/ld/abi.h>
#include <lib/ld/bootstrap.h>
#include <lib/ld/module.h>
#include <lib/ld/tls.h>
#include <zircon/assert.h>
#include <zircon/status.h>

#include <atomic>
#include <cassert>

#include "startup-relocate.h"

// This file is only linked into a static PIE that gets __libc_start_main from
// libc.a; in libc.so stub-relocate.cc defines StartupRelocate no-op methods.
//
// When linked in, this file also provides the _ld_abi linkage symbol as the
// startup dynamic linker (or remote stub dynamic linker) normally would.  Code
// in libc (or elsewhere) that uses _ld_abi to find the ELF modules can work as
// it would with dynamic linking, but it will see just the executable and vDSO.

namespace LIBC_NAMESPACE_DECL {
namespace {

// As the startup dynamic linker does, _ld_abi and the data it points to are
// all made read-only before application code runs.  Here that's done by
// placing the variables explicitly in the executable's RELRO segment at link
// time.  The ProtectRelro method will make all those pages read-only not long
// after StartupRelocate is done initializing the data.
#define RELRO(type, name, ...) \
  [[gnu::section(".data.rel.ro." #name)]] constinit type name __VA_ARGS__

using Abi = ld::abi::Abi<>;
using Elf = elfldltl::Elf<>;
using Self = elfldltl::Self<>;
using Tls = elfldltl::TlsTraits<>;

using RelroObserver = elfldltl::PhdrRelroObserver<Elf>;
using StackObserver = elfldltl::PhdrStackObserver<Elf>;
using TlsObserver = elfldltl::PhdrTlsObserver<Elf>;

using PreinitObserver = elfldltl::DynamicPreinitObserver<Elf>;
using InitObserver = elfldltl::DynamicInitObserver<Elf>;
using FiniObserver = elfldltl::DynamicFiniObserver<Elf>;

extern Abi::Module gSelfModule, gVdsoModule;

// First in the module list: the executable.
RELRO(Abi::Module, gSelfModule) = {
    .link_map = {.next{&gVdsoModule.link_map}},
    .symbols{elfldltl::kLinkerZeroInitialized},
    .symbolizer_modid = 0,
    .symbols_visible = true,
};

RELRO(Abi::Module, gVdsoModule) = {
    .link_map = {.prev{&gSelfModule.link_map}},
    .symbols{elfldltl::kLinkerZeroInitialized},
    .symbolizer_modid = 1,
    .symbols_visible = true,
};

using SingleTlsModule = std::array<Abi::TlsModule, 1>;
using SingleTlsOffset = std::array<Elf::Addr, 1>;

RELRO(SingleTlsModule, gTlsModules);
RELRO(SingleTlsOffset, gTlsOffsets);

}  // namespace

// TLS fields are initialized in StartupRelocate if there is a PT_TLS segment.
extern "C" RELRO(Abi, mutable_ld_abi, __asm__("_ld_abi"));
constinit decltype(mutable_ld_abi) mutable_ld_abi = {
    .loaded_modules{&gSelfModule},
    .loaded_modules_count{2},
};

StartupRelocate StartupRelocate::Create(const void* vdso_base) {
  // Do the bootstrap relocation so system calls can be made.
  auto bootstrap_diag = elfldltl::TrapDiagnostics();
  std::optional<Elf::Phdr> relro_phdr, tls_phdr;
  std::optional<Elf::size_type> stack_size;
  std::span<const Elf::Addr> preinit_array;
  ld::Bootstrap bootstrap{
      bootstrap_diag,
      vdso_base,
      []() { return zx_system_get_page_size(); },  // Must be after relocation.
      gSelfModule,
      gVdsoModule,
      // Collect information for _ld_abi along the way.
      std::forward_as_tuple(  // Phdr observers.
          RelroObserver{relro_phdr}, StackObserver{stack_size}, TlsObserver{tls_phdr}),
      std::forward_as_tuple(  // Dynamic observers.
          InitObserver{gSelfModule.init}, FiniObserver{gSelfModule.init},
          PreinitObserver{preinit_array}),
  };

  mutable_ld_abi.preinit_array = preinit_array;

  // After the bootstrap protocol acquires the VMAR handle, the RELRO data can
  // be protected.  First, this function modifies some of that RELRO data.
  StartupRelocate reloc;
  if (relro_phdr) [[likely]] {
    std::tie(reloc.start_, reloc.size_) =  // ProtectRelro will use these.
        elfldltl::RelroBounds(*relro_phdr, bootstrap.page_size());
    reloc.start_ += gSelfModule.link_map.addr;  // Apply the load bias.
  }

  if (!stack_size) [[unlikely]] {
    ZX_PANIC("PT_GNU_STACK must specify a p_memsz; use -Wl,-z,stack-size=...");
  }
  mutable_ld_abi.stack_size = *stack_size;

  // If the executable has a PT_TLS, fill out all the TLS bits in _ld_abi.
  if (tls_phdr) {
    const Elf::Phdr& tls = *tls_phdr;
    auto memory = Self::Memory();
    std::span tdata = *memory.ReadArray<std::byte>(tls.vaddr, tls.filesz);

    gSelfModule.tls_modid = 1;
    auto& tls_module = gTlsModules.front();
    auto& tls_offset = gTlsOffsets.front();

    tls_module = {.tls_initial_data = tdata,
                  .tls_bss_size = tls.memsz - tls.filesz,
                  .tls_alignment = tls.align};

    tls_offset = mutable_ld_abi.static_tls_layout.Assign(tls.memsz, tls.align);
    mutable_ld_abi.static_tls_modules = std::span(gTlsModules);
    mutable_ld_abi.static_tls_offsets = std::span(gTlsOffsets);
  }

  // At this point everything looks pretty much like it would have looked at
  // the program entry point after dynamic linking.  The main difference left
  // is that the startup dynamic linker would have write-protected the pages
  // containing _ld_abi and what it points to.  That can't be done until the
  // bootstrap protocol acquires the VMAR handle and calls ProtectRelro,
  // below.  All the stores to RELRO data must be ordered before that.
  std::atomic_signal_fence(std::memory_order_release);

  return reloc;
}

// The (only) VMAR handle will be closed on return, so the region can never be
// made writable again later.
void StartupRelocate::ProtectRelro(zx::vmar loaded_vmar) const&& {
  assert(loaded_vmar);
  zx_status_t status = loaded_vmar.protect(ZX_VM_PERM_READ, start_, size_);
  ZX_ASSERT_MSG(status == ZX_OK, "cannot protect RELRO: %s", zx_status_get_string(status));
}

}  // namespace LIBC_NAMESPACE_DECL
