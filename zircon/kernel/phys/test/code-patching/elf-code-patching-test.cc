// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <inttypes.h>
#include <lib/zbitl/error-stdio.h>
#include <stdio.h>

#include <fbl/no_destructor.h>
#include <ktl/array.h>
#include <ktl/byte.h>
#include <ktl/span.h>
#include <ktl/string_view.h>
#include <phys/elf-image.h>
#include <phys/kernel-package.h>
#include <phys/symbolize.h>

#include "../physload-test-main.h"
#include "test.h"

#include <ktl/enforce.h>

namespace {

constexpr ktl::string_view kAddOne = "add-one";
constexpr ktl::string_view kMultiply = "multiply_by_factor";

// We need space for 6 modules: physload, this module, and the patched and
// unpatched versions of kAddOne and kMultiply. Keep it as a global so
// references to this storage can persist outside this PhysLoadTestMain.
ktl::array<const ElfImage*, 6> modules;

}  // namespace

int PhysLoadTestMain(KernelStorage& kernelfs) {
  constexpr const char* kTestName = "elf-code-patching-test";
  gSymbolize->set_name(kTestName);

  gSymbolize->Context();

  gSymbolize->ReplaceModulesStorage(Symbolize::ModuleList(ktl::span(modules)));

  constexpr uint64_t kValue = 42;

  // Test that unpatched add-one loads and behaves as expected.
  // Note this keeps the Allocation alive after the test so its
  // pages won't be reused for the second test, since that could
  // require cache flushing operations.
  Allocation unpatched;
  printf("%s: Testing unpatched add-one...\n", gSymbolize->name());
  {
    static fbl::NoDestructor<ElfImage> add_one;
    if (auto result = add_one->Init(kernelfs.root(), kAddOne, true); result.is_error()) {
      zbitl::PrintBootfsError(result.error_value());
      return 1;
    }

    add_one->AssertInterpMatchesBuildId(kAddOne, gSymbolize->build_id());
    unpatched = add_one->Load(memalloc::Type::kPhysElf, {}, false);
    add_one->Relocate();

    printf("%s: Calling %p...", gSymbolize->name(), add_one->ImageEntry<TestFn>());
    uint64_t value = add_one->Call<TestFn>(kValue);
    ZX_ASSERT_MSG(value == kValue + 1, "unpatched add-one: got %" PRIu64 " != expected %" PRIu64,
                  value, kValue + 1);
  }
  printf("OK\n");

  // Now test it with nop patching: AddOne becomes the identity function.
  Allocation patched;
  printf("%s: Testing patched add-one...\n", gSymbolize->name());
  {
    static fbl::NoDestructor<ElfImage> add_one;
    if (auto result = add_one->Init(kernelfs.root(), kAddOne, true); result.is_error()) {
      zbitl::PrintBootfsError(result.error_value());
      return 1;
    }

    add_one->AssertInterpMatchesBuildId(kAddOne, gSymbolize->build_id());
    patched = add_one->Load(memalloc::Type::kPhysElf, {}, false);
    add_one->Relocate();

    enum class ExpectedCase : uint32_t { kAddOne = kAddOneCaseId };

    auto patch = [](code_patching::Patcher& patcher, ExpectedCase case_id,
                    ktl::span<ktl::byte> code, auto&& print) -> fit::result<ElfImage::Error> {
      ZX_ASSERT_MSG(case_id == ExpectedCase::kAddOne,
                    "code-patching case ID %" PRIu32 " != expected %" PRIu32,
                    static_cast<uint32_t>(case_id), static_cast<uint32_t>(ExpectedCase::kAddOne));
      ZX_ASSERT_MSG(code.size_bytes() == PATCH_SIZE_ADD_ONE, "code patch %zu bytes != expected %d",
                    code.size_bytes(), PATCH_SIZE_ADD_ONE);
      print({"patched nop-fill"});
      patcher.NopFill(code);
      return fit::ok();
    };
    auto result = add_one->ForEachPatch<ExpectedCase>(patch);
    ZX_ASSERT(result.is_ok());

    printf("%s: Calling %p...", gSymbolize->name(), add_one->ImageEntry<TestFn>());
    uint64_t value = add_one->Call<TestFn>(kValue);
    ZX_ASSERT_MSG(value == kValue, "nop-patched add-one: got %" PRIu64 " != expected %" PRIu64,
                  value, kValue);
  }
  printf("OK\n");

  // Now test the hermetic blob stub case.
  Allocation patched_stub2;
  printf("%s: Testing hermetic blob (alternative 1)...\n", gSymbolize->name());
  {
    static fbl::NoDestructor<ElfImage> multiply;
    if (auto result = multiply->Init(kernelfs.root(), kMultiply, true); result.is_error()) {
      zbitl::PrintBootfsError(result.error_value());
      return 1;
    }

    multiply->AssertInterpMatchesBuildId(kMultiply, gSymbolize->build_id());
    patched_stub2 = multiply->Load(memalloc::Type::kPhysElf, {}, false);
    multiply->Relocate();

    enum class ExpectedCase : uint32_t { kMultiply = kMultiplyByFactorCaseId };

    auto patch = [](code_patching::Patcher& patcher, ExpectedCase case_id,
                    ktl::span<ktl::byte> code, auto&& print) -> fit::result<ElfImage::Error> {
      ZX_ASSERT_MSG(case_id == ExpectedCase::kMultiply,
                    "code-patching case ID %" PRIu32 " != expected %" PRIu32,
                    static_cast<uint32_t>(case_id), static_cast<uint32_t>(ExpectedCase::kMultiply));
      ZX_ASSERT_MSG(code.size_bytes() == PATCH_SIZE_MULTIPLY_BY_FACTOR,
                    "code patch %zu bytes != expected %d", code.size_bytes(),
                    PATCH_SIZE_MULTIPLY_BY_FACTOR);
      print({"patch in multiply_by_two"});
      return patcher.PatchWithAlternative(code, "multiply_by_two");
    };
    auto result = multiply->ForEachPatch<ExpectedCase>(patch);
    ZX_ASSERT_MSG(result.is_ok(), "%.*s", static_cast<int>(result.error_value().reason.size()),
                  result.error_value().reason.data());

    printf("%s: Calling %p...", gSymbolize->name(), multiply->ImageEntry<TestFn>());
    uint64_t value = multiply->Call<TestFn>(kValue);
    ZX_ASSERT_MSG(value == kValue * 2, "multiply_by_two got %" PRIu64 " != expected %" PRIu64,
                  value, kValue * 2);
  }
  printf("OK\n");

  // Now test the hermetic blob stub case.
  Allocation patched_stub10;
  printf("%s: Testing hermetic blob (alternative 2)...\n", gSymbolize->name());
  {
    static fbl::NoDestructor<ElfImage> multiply;
    if (auto result = multiply->Init(kernelfs.root(), kMultiply, true); result.is_error()) {
      zbitl::PrintBootfsError(result.error_value());
      return 1;
    }

    multiply->AssertInterpMatchesBuildId(kMultiply, gSymbolize->build_id());
    patched_stub10 = multiply->Load(memalloc::Type::kPhysElf, {}, false);
    multiply->Relocate();

    enum class ExpectedCase : uint32_t { kMultiply = kMultiplyByFactorCaseId };

    auto patch = [](code_patching::Patcher& patcher, ExpectedCase case_id,
                    ktl::span<ktl::byte> code, auto&& print) -> fit::result<ElfImage::Error> {
      ZX_ASSERT_MSG(case_id == ExpectedCase::kMultiply,
                    "code-patching case ID %" PRIu32 " != expected %" PRIu32,
                    static_cast<uint32_t>(case_id), static_cast<uint32_t>(ExpectedCase::kMultiply));
      ZX_ASSERT_MSG(code.size_bytes() == PATCH_SIZE_MULTIPLY_BY_FACTOR,
                    "code patch %zu bytes != expected %d", code.size_bytes(),
                    PATCH_SIZE_MULTIPLY_BY_FACTOR);
      print({"patch in multiply_by_ten"});
      return patcher.PatchWithAlternative(code, "multiply_by_ten");
    };
    auto result = multiply->ForEachPatch<ExpectedCase>(patch);
    ZX_ASSERT_MSG(result.is_ok(), "%.*s", static_cast<int>(result.error_value().reason.size()),
                  result.error_value().reason.data());

    printf("%s: Calling %p...", gSymbolize->name(), multiply->ImageEntry<TestFn>());
    uint64_t value = multiply->Call<TestFn>(kValue);
    ZX_ASSERT_MSG(value == kValue * 10, "multiply_by_ten got %" PRIu64 " != expected %" PRIu64,
                  value, kValue * 10);
  }
  printf("OK\n");

  return 0;
}
