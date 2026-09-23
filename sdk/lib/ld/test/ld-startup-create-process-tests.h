// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_LD_STARTUP_CREATE_PROCESS_TESTS_H_
#define LIB_LD_TEST_LD_STARTUP_CREATE_PROCESS_TESTS_H_

#include <lib/elfldltl/layout.h>
#include <lib/elfldltl/testing/loader.h>
#include <lib/ld/abi.h>
#include <lib/ld/testing/test-processargs.h>
#include <lib/ld/testing/vdso.h>
#include <lib/ld/vmar.h>
#include <lib/zx/thread.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>

#include <cstdint>
#include <initializer_list>
#include <optional>
#include <string_view>

#include <gtest/gtest.h>

#include "ld-load-zircon-process-tests-base.h"

namespace ld::testing {

// This is the common base class for template instantiations.  It handles the
// process mechanics, while the templated subclass does the loading.
class LdStartupCreateProcessTestsBase : public LdLoadZirconProcessTestsBase {
 public:
  void Init(std::initializer_list<std::string_view> args = {},
            std::initializer_list<std::string_view> env = {});

  ~LdStartupCreateProcessTestsBase();

 protected:
  // Call this after CreateProcess() to mimic the system program loader's
  // behavior of avoiding the low half of the address space.
  void MimicSpawnProcessVmarReservation();

  void FinishLoad(zx::vmo executable_vmo);
};

template <class Elf = elfldltl::Elf<>>
class LdStartupCreateProcessTests
    : public elfldltl::testing::LoadTests<elfldltl::testing::RemoteVmarLoaderTraits, Elf>,
      public LdStartupCreateProcessTestsBase {
 public:
  static constexpr std::string_view kName = "LdStartupCreateProcessTests";

  using LdStartupCreateProcessTestsBase::Run;

  void Load(std::string_view executable_name,
            std::optional<std::string_view> expected_config = std::nullopt) {
    ASSERT_TRUE(root_vmar());  // Init must have been called already.

    // This points GetLibVmo() to the right place.
    LdsvcPathPrefix(executable_name);

    // This will adjust GetLibVmo() to use the libprefix from PT_INTERP so the
    // fetch of abi::kInterp will get the right install location.
    zx::vmo executable_vmo = GetExecutableVmoWithInterpConfig(executable_name, expected_config);
    ASSERT_TRUE(executable_vmo);

    // Load the dynamic linker and record its entry point.
    std::optional<LoadResult> result;
    ASSERT_NO_FATAL_FAILURE(
        this->Load(GetInterp(executable_name, expected_config), result, root_vmar()));
    set_entry(result->entry + result->loader.load_bias());
    set_stack_size(result->stack_size);

    // The bootstrap message gets the VMAR where the dynamic linker resides.
    zx::vmar self_vmar = std::move(result->loader).Commit(kNoRelro).TakeVmar();
    ASSERT_NO_FATAL_FAILURE(bootstrap().AddSelfVmar(std::move(self_vmar)));

    // Load the vDSO and record its base address.
    // No handles to the VMAR where it was loaded survive.
    ASSERT_NO_FATAL_FAILURE(this->Load(*GetVdsoVmo(), result, root_vmar()));
    set_vdso_base(result->info.vaddr_start() + result->loader.load_bias());
    std::ignore = std::move(result->loader).Commit(kNoRelro);

    ASSERT_NO_FATAL_FAILURE(FinishLoad(std::move(executable_vmo)));
  }

  template <class... Reports>
  void LoadAndFail(std::string_view name, Reports&&... reports) {
    ASSERT_NO_FATAL_FAILURE(StartupLoadAndFail(*this, name, std::forward<Reports>(reports)...));
  }

 private:
  using Base = elfldltl::testing::LoadTests<elfldltl::testing::RemoteVmarLoaderTraits, Elf>;
  using Base::kNoRelro;
  using Base::Load;
  using typename Base::LoadResult;
};

// Just the same, but create the process with the ZX_PROCESS_SHARED flag so
// that its root VMAR will be the "top half only" VMAR.
template <class Elf = elfldltl::Elf<>>
class LdStartupCreateSharedProcessTests : public LdStartupCreateProcessTests<Elf> {
 public:
  static constexpr std::string_view kName = "LdStartupCreateSharedProcessTests";

  LdStartupCreateSharedProcessTests() { this->set_create_options(ZX_PROCESS_SHARED); }
};

}  // namespace ld::testing

#endif  // LIB_LD_TEST_LD_STARTUP_CREATE_PROCESS_TESTS_H_
