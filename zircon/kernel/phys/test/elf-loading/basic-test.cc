// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <inttypes.h>
#include <lib/elfldltl/static-vector.h>
#include <lib/zbitl/error-stdio.h>
#include <lib/zbitl/items/bootfs.h>
#include <lib/zbitl/view.h>
#include <stdio.h>

#include <ktl/byte.h>
#include <ktl/span.h>
#include <ktl/string_view.h>
#include <phys/address-space.h>
#include <phys/elf-image.h>
#include <phys/kernel-package.h>
#include <phys/symbolize.h>
#include <phys/zbitl-allocation.h>

#include "../test-main.h"
#include "get-int.h"

#include <ktl/enforce.h>

namespace {

// The name of ELF module to be loaded.
constexpr ktl::string_view kGetInt = "get-int.basic-elf-loading-test";

// The BOOTFS namespace under which kGetInt lives.
constexpr ktl::string_view kNamespace = "basic-elf-loading-test-data";

}  // namespace

int TestMain(void* zbi_ptr, ktl::optional<EarlyBootZbi> early_zbi, arch::EarlyTicks) {
  MainSymbolize symbolize("basic-elf-loading-test");

  // Initialize memory for allocation/free.
  AddressSpace aspace;
  InitMemory(zbi_ptr, ktl::move(early_zbi), &aspace);

  zbitl::View zbi(
      zbitl::StorageFromRawHeader<ktl::span<ktl::byte>>(static_cast<zbi_header_t*>(zbi_ptr)));
  KernelStorage kernelfs;
  kernelfs.Init(zbi);

  KernelStorage::Bootfs bootfs;
  if (auto result = kernelfs.root().subdir(kNamespace); result.is_error()) {
    zbitl::PrintBootfsError(result.error_value());
    return 1;
  } else {
    bootfs = ktl::move(result).value();
  }

  symbolize.Context();

  ktl::array<const ElfImage*, 2> modules;
  symbolize.EnableModuleLoading(Symbolize::ModuleList(ktl::span(modules)));

  printf("Loading %.*s...\n", static_cast<int>(kGetInt.size()), kGetInt.data());
  ElfImage elf;
  if (auto result = elf.Init(bootfs, kGetInt, true); result.is_error()) {
    zbitl::PrintBootfsError(result.error_value());
    return 1;
  }

  ZX_ASSERT(!elf.has_patches());

  // The GN target for get-int uses kernel_elf_interp() on this test binary.
  printf("Verifying PT_INTERP matches test build ID...\n");
  elf.AssertInterpMatchesBuildId(kGetInt, symbolize.build_id());

  // If the file can't be loaded in place, this Allocation owns its image.
  Allocation loaded = elf.Load(memalloc::Type::kPhysElf);
  elf.Relocate();

  // Since Context() was called above ContextOnLoad() should have printed
  // inside elf.Load() above.  Now that the new module list is in place,
  // printing again should show the same two modules already printed piecemeal.
  printf("Symbolizer context for both modules expected, repeating:\n");
  symbolize.ContextAlways();

  printf("Calling entry point %p...\n", elf.ImageEntry<decltype(GetInt)>());

  // We should now be able to access GetInt()!
  constexpr int kExpected = 42;
  if (int actual = elf.Call<decltype(GetInt)>(); actual != kExpected) {
    printf("FAILED: Expected %d; got %d\n", kExpected, actual);
    return 1;
  }

  return 0;
}
