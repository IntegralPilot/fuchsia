// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/zbitl/error-stdio.h>

#include <cstdint>

#include <phys/address-space.h>
#include <phys/elf-image.h>
#include <phys/kernel-package.h>
#include <phys/symbolize.h>

#include "../physload-test-main.h"
#include "get-int.h"

#ifdef __aarch64__
#include <lib/arch/arm64/system.h>
#endif

#include <ktl/enforce.h>

namespace {

constexpr const char* kTestName = "virtual-address-loading-loading-test";

// The name of ELF module to be loaded.
constexpr ktl::string_view kGetInt = "get-int.virtual-address-loading-test";

// TODO(https://fxbug.dev/42172722): Pick a load address through a sort of allocator
// (i.e., as we would in production).
constexpr uint64_t kLoadAddress = 0xffff'ffff'0000'0000;

}  // namespace

int PhysLoadTestMain(KernelStorage& kernelfs) {
  gSymbolize->set_name(kTestName);

#ifdef __aarch64__
  // TODO(https://fxbug.dev/42085337): There is no upper address space in EL2+ by
  // default, so drop down to EL1 and proceed with the test.
  if (auto el = arch::ArmCurrentEl::Read().el(); el > 1) {
    printf(
        "TODO(https://fxbug.dev/42085337): %s test is only supported for EL1 (current EL is %lu); skipping it\n",
        kTestName, el);
    return 0;
  }
#endif

  printf("Loading %.*s...\n", static_cast<int>(kGetInt.size()), kGetInt.data());
  ElfImage elf;
  if (auto result = elf.Init(kernelfs.root(), kGetInt, true); result.is_error()) {
    zbitl::PrintBootfsError(result.error_value());
    return 1;
  }

  ZX_ASSERT(!elf.has_patches());

  // The GN target for get-int uses kernel_elf_interp() on this test binary.
  printf("Verifying PT_INTERP matches test build ID...\n");
  elf.AssertInterpMatchesBuildId(kGetInt, gSymbolize->build_id());

  Allocation loaded = elf.Load(memalloc::Type::kPhysElf, kLoadAddress);
  elf.Relocate();

  // TODO(https://fxbug.dev/42172722): Set up C++ ABI support and map and jump into a
  // loaded C++ program.
  if (auto result = elf.MapInto(*gAddressSpace); result.is_error()) {
    printf("Failed to map loaded image\n");
    return 1;
  }

  auto entry = reinterpret_cast<decltype(GetInt)*>(elf.entry() + elf.load_bias());
  printf("Calling virtual entry point %p...\n", entry);

  // We should now be able to access GetInt()!
  constexpr int kExpected = 42;
  if (int actual = entry(); actual != kExpected) {
    printf("FAILED: Expected %d; got %d\n", kExpected, actual);
    return 1;
  }

  return 0;
}
