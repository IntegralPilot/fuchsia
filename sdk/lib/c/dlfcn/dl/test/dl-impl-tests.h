// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_DLFCN_DL_TEST_DL_IMPL_TESTS_H_
#define LIB_C_DLFCN_DL_TEST_DL_IMPL_TESTS_H_

#include <lib/fit/defer.h>
#include <lib/ld/testing/startup-ld-abi.h>

#include "../runtime-dynamic-linker.h"
#include "../tlsdesc-runtime-dynamic.h"
#include "dl-load-tests-base.h"

#ifdef __Fuchsia__
#include "dl-load-zircon-tests-base.h"
#endif

namespace dl::testing {

// This handles TLS runtime test support that need not be templated like the
// rest of DlImplTests.  The only instance of this (empty) class is its own
// private thread_local that ensures per-thread cleanup.
class DlImplTestsTls {
 public:
  // Ensure this thread is ready for a TLSDESC access.  This stands in for the
  // integration of thread startup with RuntimeDynamicLinker, and for the
  // synchronization regime for existing threads when dlopen expands the
  // _dl_tlsdesc_runtime_dynamic_blocks arrays.
  static void Prepare(const RuntimeDynamicLinker& linker);

  // This happens at the end of each test, which is only on the main thread.
  // Always leave a clean slate for the next test.
  static void Cleanup();

 private:
  constexpr DlImplTestsTls() = default;

  ~DlImplTestsTls() { Cleanup(); }

  // This just exists to get the destructor run in each thread as it exits.  On
  // the main thread, this doesn't happen until process exit; it's almost
  // always a no-op because the last ~DlImplTestsTls() run after the end of a
  // test left things clear anyway.  Other threads are from std::jthread used
  // inside a test, so those are all joined and this have already run this
  // destructor before the governing test ended (and hit its Cleanup() call).
  static thread_local DlImplTestsTls cleanup_at_thread_exit_;

  // This tracks the last-allocated blocks in case of expansion.
  // Its ownership is "shared" with _dl_tlsdesc_runtime_dynamic_blocks.
  SizedDynamicTlsArray blocks_;
};

// The Base class provides testing facilities and logic specific to the
// platform the test is running on. DlImplTests invokes Base methods when
// functions need to operate differently depending on the OS.
template <class Base>
class DlImplTests : public Base {
 public:
  // Error messages in tests can be matched exactly with this test fixture,
  // since the error message returned from the libdl implementation will be the
  // same regardless of the OS.
  static constexpr bool kCanMatchExactError = true;
  // TODO(https://fxbug.dev/382529434): Have dlclose() run finalizers
  static constexpr bool kDlCloseCanRunFinalizers = false;
  // TODO(https://fxbug.dev/342028933): Have dlclose() unload modules
  static constexpr bool kDlCloseUnloadsModules = false;
  // TODO(https://fxbug.dev/368112176): enable DT_SONAME lookup among pending
  // modules waiting to be loaded in a LinkingSession. See
  // DlTests.SonameFilenameLoadedDep for current vs expected behavior.
  static constexpr bool kSonameLookupInPendingDeps = false;
  // The resolver policy that determines symbol resolution order, and that tests
  // use to verify resolved values.
  static constexpr elfldltl::ResolverPolicy kResolverPolicy = ld::kResolverPolicy;

#ifndef __Fuchsia__
  // TODO(https://fxbug.dev/419872720): Disable a DT_SONAME lookup test on host
  // because the DT_NEEDED order in the module is generated in the wrong order.
  static constexpr bool kSonameLookupInLoadedDeps = false;
#endif

  class DynamicTlsHelper {
   public:
    // Load the same module in parallel with the system dlopen.  It and its
    // deps should get assigned the same module IDs that the just-completed
    // DlImplTests::DlOpen call assigned, so the system __tls_get_addr lookups
    // will find the corresponding module's dynamic TLS segment with the right
    // initial data.
    void Init(const char* file) {
      ASSERT_EQ(system_handle_, nullptr);
      system_handle_ = dlopen(file, RTLD_NOW | RTLD_LOCAL);
      ASSERT_TRUE(system_handle_) << "system dlopen(\"" << file << "\"): " << dlerror();
    }

    ~DynamicTlsHelper() {
      if (system_handle_) {
        EXPECT_EQ(dlclose(system_handle_), 0) << dlerror();
      }
    }

   private:
    void* system_handle_ = nullptr;
  };

  void SetUp() override {
    Base::SetUp();

    fbl::AllocChecker ac;
    dynamic_linker_ = RuntimeDynamicLinker::Create(ld::testing::gStartupLdAbi, ac);
    ASSERT_TRUE(ac.check());
  }

  void TearDown() override { DlImplTestsTls::Cleanup(); }

  fit::result<Error, void*> DlOpen(const char* file, int mode) {
    // Check that all Needed/Expect* expectations for loaded objects were
    // satisfied and then clear the expectation set.
    auto verify_expectations = fit::defer([&]() { Base::VerifyAndClearNeeded(); });
    auto result = dynamic_linker_->Open<typename Base::Loader>(
        file, mode, std::bind_front(&Base::RetrieveFile, this));
    if (result.is_ok()) {
      // If RTLD_NOLOAD was passed and we have a NULL return value, there is no
      // module to track.
      if ((mode & RTLD_NOLOAD) && !result.value()) {
        return result;
      }
      // TODO(https://fxbug.dev/382527519): RuntimeDynamicLinker should have a
      // `RunInitializers` method that will run this with proper synchronization.
      RuntimeModule::FromPtr(result.value()).InitializeModuleTree();
      Base::TrackModule(result.value(), file);
    }
    return result;
  }

  // TODO(https://fxbug.dev/342028933): Implement dlclose.
  fit::result<Error> DlClose(void* module) {
    auto untrack_file = fit::defer([&]() { Base::UntrackModule(module); });
    // At minimum check that a valid handle was passed and present in the
    // dynamic linker's list of modules.
    for (auto& m : dynamic_linker_->modules()) {
      if (&m == module) {
        return fit::ok();
      }
    }
    return fit::error<Error>{"Invalid library handle %p", module};
  }

  fit::result<Error, void*> DlSym(void* module, const char* ref) {
    const RuntimeModule& root = RuntimeModule::FromPtr(module);
    return dynamic_linker_->LookupSymbol(root, ref);
  }

  int DlIteratePhdr(DlIteratePhdrCallback* callback, void* data) {
    return dynamic_linker_->IteratePhdrInfo(callback, data);
  }

  // The `dynamic_linker_-> dtor will also destroy and unmap modules remaining
  // in its modules list, so there is no need to do any extra clean up
  // operation.
  void CleanUpOpenedFile(void* ptr) override {}

  // A test will call this function before the running thread accesses a TLS
  // variable. This function will allocate and initialize TLS data on the
  // thread so the thread can access that data.
  void PrepareForTlsAccess() { DlImplTestsTls::Prepare(*dynamic_linker_); }

  // Return the `link_map` data structure associated with a module. `handle`
  // should be a valid pointer returned by a successful dlopen() call.
  const link_map* ModuleLinkMap(const void* handle) {
    return RuntimeModule::FromPtr(handle).user_link_map();
  }

  fit::result<Error, int> DlInfo(void* module, int request, void* info) {
    return dynamic_linker_->DlInfo(module, request, info);
  }

 private:
  std::unique_ptr<RuntimeDynamicLinker> dynamic_linker_;
};

class DlLoadPosixTestsBase : public DlLoadTestsBase {
 public:
  static constexpr std::string_view kName = "DlImplLoadPosixTests";
};

using DlImplLoadPosixTests = DlImplTests<DlLoadPosixTestsBase>;
#ifdef __Fuchsia__
using DlImplLoadZirconTests = DlImplTests<DlLoadZirconTestsBase>;
#endif

}  // namespace dl::testing

#endif  // LIB_C_DLFCN_DL_TEST_DL_IMPL_TESTS_H_
