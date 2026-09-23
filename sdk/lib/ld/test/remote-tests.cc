// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/testing/diagnostics.h>
#include <lib/ld/remote-abi-heap.h>
#include <lib/ld/remote-abi-stub.h>
#include <lib/ld/remote-dynamic-linker.h>
#include <lib/ld/remote-perfect-symbol-filter.h>
#include <lib/ld/remote-zygote.h>
#include <lib/ld/testing/get-test-vmo.h>
#include <lib/ld/testing/test-elf-object.h>

#include <gtest/gtest.h>

#include "ld-remote-process-tests.h"
#include "remote-perfect-symbol-filter-test.h"

namespace {

using ::testing::AnyOf;
using ::testing::Each;
using ::testing::Field;
using ::testing::IsTrue;
using ::testing::Pointee;
using ::testing::Property;
using ::testing::Truly;
using ::testing::VariantWith;

// These tests reuse the fixture that supports the LdLoadTests (load-tests.cc)
// for the common handling of creating and launching a Zircon process.  The
// Load method is not used here, since that itself uses the RemoteDynamicLinker
// API under the covers, and the tests here are for that API surface itself.
using LdRemoteTests = ld::testing::LdRemoteProcessTests;

// This is the basic examplar of using the API to load a main executable in the
// standard way.
TEST_F(LdRemoteTests, RemoteDynamicLinker) {
  constexpr int64_t kReturnValue = 17;

  // The Init() method in the test fixture handles creating a process and such.
  // This is outside the scope of the ld::RemoteDynamicLinker API.
  ASSERT_NO_FATAL_FAILURE(Init());

  LdsvcPathPrefix("many-deps");

  auto diag = elfldltl::testing::ExpectOkDiagnostics();

  // Acquire the layout details from the stub.  The same ld::RemoteAbiStub
  // object can be reused for creating and populating the passive ABI of any
  // number of separate dynamic linking domains in however many processes.
  //
  // The TakeSubLdVmo() method in the test fixture returns the (read-only,
  // executable) zx::vmo for the stub dynamic linker provided along with the
  // //sdk/lib/ld library and packaged somewhere with the code using this API.
  // The user of the API must acquire such a VMO by their own means.
  Linker linker;
  linker.set_abi_stub(ld::RemoteAbiStub<>::Create(diag, TakeStubLdVmo(), kPageSize));
  ASSERT_TRUE(linker.abi_stub());

  // The main executable is an ELF file in a VMO.  The GetExecutableVmoo()
  // method in the test fixture returns the (read-only, executable) zx::vmo for
  // the main executable.  The user of the API must acquire this VMO by their
  // own means.
  zx::vmo exec_vmo;
  ASSERT_NO_FATAL_FAILURE(exec_vmo = ld::testing::GetExecutableVmo("many-deps"));

  // This makes sure the Needed() call below finds the files in the test
  // packaging correctly.  These are only aspects of the test framework API,
  // not of the remote dynamic linker API.
  ConfigFromInterp(exec_vmo.borrow());

  // Decode the main executable.  This transfers ownership of the zx::vmo for
  // the executable into the new fbl::RefPtr<ld::RemoteDecodedModule> object.
  // If there were decoding problems they will have been reported to the
  // Diagnostics template API object.  If that object said to bail out after an
  // error or warning, Create returns a null RefPtr.  If it said to keep going
  // after an error, then an object was created but may be incomplete: it can
  // be used in ld::RemoteDynamicLinker::Init, but may not be in a fit state to
  // attempt relocation.
  Linker::Module::DecodedPtr decoded_executable =
      Linker::Module::Decoded::Create(diag, std::move(exec_vmo), kPageSize);
  EXPECT_TRUE(decoded_executable);

  // If the program is meant to make Zircon system calls, then it needs a vDSO,
  // in the form of a (read-only, executable) zx::vmo handle to one of the
  // kernel's blessed vDSO VMOs.  The GetVdsoVmo() function in the testing
  // library returns the same one used by the test itself.  The user of the API
  // must acquire the desired vDSO VMO by their own means.
  zx::vmo vdso_vmo;
  zx_status_t status = ld::testing::GetVdsoVmo()->duplicate(ZX_RIGHT_SAME_RIGHTS, &vdso_vmo);
  EXPECT_EQ(status, ZX_OK) << zx_status_get_string(status);

  // Decode the vDSO, just as done for the main executable.  The DecodedPtr
  // references can be cached and reused for any VMO of an ELF file.
  Linker::Module::DecodedPtr decoded_vdso =
      Linker::Module::Decoded::Create(diag, std::move(vdso_vmo), kPageSize);
  EXPECT_TRUE(decoded_vdso);

  // The get_dep callback is any object callable as GetDepResult(Soname).  It
  // returns std::nullopt for missing dependencies, or a DecodedPtr.  The
  // GetDepFunction() in the test fixture returns an object that approximates
  // for the test context something like looking up files in /pkg/lib as is
  // done via fuchsia.ldsvc FIDL protocols by the usual in-process dynamic
  // linker.  The Needed() method in the test fixture indicates the expected
  // sequence of requests and collects those files from the test package's
  // special directory layout.  The user of the API must supply a callback that
  // turns strings into appropriate ld::RemoteDecodedModule::Ptr refs.  The
  // callback returns std::nullopt to bail out after a failure; the
  // RemoteDynamicLinker does not do any logging about this directly, so the
  // callback itself should do so.  The callback may also return a null Ptr
  // instead to indicate work should keep going despite the missing file.  This
  // will likely result in more errors later, such as undefined symbols; but it
  // gives the opportunity to report more missing files before bailing out.
  auto get_dep = GetDepFunction(diag);
  ASSERT_NO_FATAL_FAILURE(Needed({
      "libld-dep-a.so",
      "libld-dep-b.so",
      "libld-dep-f.so",
      "libld-dep-c.so",
      "libld-dep-d.so",
      "libld-dep-e.so",
  }));
  ASSERT_NO_FATAL_FAILURE(LdsvcExpectNeeded());

  // Init() decodes everything and loads all the dependencies.
  auto init_result = linker.Init(
      // Any <lib/elfldltl/diagnostics.h> template API object can be used.
      diag,
      // The InitModuleList argument is a std::vector, so it can be constructed
      // in many ways including an initializer list.  For inividual InitModule
      // elements there is a convenient factory function that suits each use
      // case.  The order of the root modules is important: it becomes the
      // "load order" used for symbol resolution and seen in the passive
      // ABI--but usually that's just the main executable.  Implicit modules
      // can appear in any order with respect to each other or the root
      // modules; the only effect is on the relative order of any unreferenced
      // implicit modules at the end of the ld::RemoteDynamicLinker::modules()
      // "load order" list.
      {Linker::Executable(std::move(decoded_executable)),
       Linker::Implicit(std::move(decoded_vdso))},
      get_dep);
  ASSERT_TRUE(init_result);

  // The return value is a vector parallel to the InitModuleList passed in.
  ASSERT_EQ(init_result->size(), 2u);

  // Allocate() chooses load addresses by creating new child VMARs within some
  // given parent VMAR, such as the root VMAR of a new process.
  EXPECT_TRUE(linker.Allocate(diag, root_vmar().borrow()));

  // The corresponding return vector element is an iterator into the
  // ld::RemoteDynamicLinker::modules() list.  After Allocate, the vaddr
  // details of each module have been decided.  The vDSO base address is
  // usually passed as the main executable entry point's second argument when
  // the process is launched via zx::process::start.  The test fixture's Run()
  // method passes this to zx::process::start, but launching the process is
  // outside the scope of this API.
  const Linker::Module& loaded_vdso = *init_result->back();
  set_vdso_base(loaded_vdso.module().vaddr_start());

  // main_entry() yields the runtime entry point address of the main (first)
  // root module, usually the main executable.  Naturally, it's only valid
  // after a successful Allocate phase.  The test fixture's Run() method passes
  // this to zx::process::start, but launching the process is outside the scope
  // of this API.
  set_entry(linker.main_entry());

  // main_stack_size() yields either std::nullopt or a specific stack size
  // requested by the executable's PT_GNU_STACK program header.  The test
  // fixture's Run() method uses this to allocate a stack and pass the initial
  // SP in zx::process::start; stack setup is outside the scope of this API.
  set_stack_size(linker.main_stack_size());

  // Relocate() applies relocations to segment VMOs.  This is the last place
  // that anything can usually go wrong due to a missing or invalid ELF file,
  // undefined symbol, or such problems with dynamic linking per se.
  EXPECT_TRUE(linker.Relocate(diag));

  // Finally, all the VMO contents are in place to be mapped into the process.
  // If this fails, it will be because of some system problem like resource
  // exhaustion rather than something about dynamic linking.
  ASSERT_TRUE(linker.Load(diag));

  // Any failure before here would destroy all the VMARs when the linker object
  // goes out of scope.  From here the mappings will stick in the process.
  linker.Commit();

  // The test fixture method does the rest of the work of launching the
  // process, all of which is out of the scope of this API:
  //  1. stack setup
  //  2. preparing a channel for the process bootstrap protocol
  //  3. calling zx::process::start with initial PC (e.g. from main_entry()),
  //     SP (from the stack setup), and the two entry point arguments:
  //      * some Zircon handle, usually the channel from which the process
  //        expects to read the message(s) of the process bootstrap protocol;
  //      * some integer, usually the base address where the vDSO was loaded,
  //        e.g. from `.module().vaddr_start` on the Linker::Module object for
  //        the vDSO, an implicit module found via Init()'s return value.
  // The test fixture method yields the process exit status when it finishes.
  EXPECT_EQ(Run(), kReturnValue);

  // The test fixture collected any output from the process and requires that
  // it be checked.
  ExpectLog("");
}

// This demonstrates using ld::RemoteDynamicLinker::Preplaced in the initial
// modules list.
TEST_F(LdRemoteTests, Preplaced) {
  ASSERT_NO_FATAL_FAILURE(Init());

  const zx_info_vmar_t aspace = RootVmarInfo();
  const uint64_t top_gig = aspace.base + aspace.len - 0x40000000;
  const uint64_t load_address = top_gig + 0x1230000;

  auto diag = elfldltl::testing::ExpectOkDiagnostics();

  Linker linker;
  linker.set_abi_stub(ld::RemoteAbiStub<>::Create(diag, TakeStubLdVmo(), kPageSize));
  ASSERT_TRUE(linker.abi_stub());

  zx::vmo exec_vmo;
  ASSERT_NO_FATAL_FAILURE(exec_vmo = ld::testing::GetExecutableVmo("fixed-load-address"));

  Linker::Module::DecodedPtr decoded_executable =
      Linker::Module::Decoded::Create(diag, std::move(exec_vmo), kPageSize);
  EXPECT_TRUE(decoded_executable);

  zx::vmo vdso_vmo;
  zx_status_t status = ld::testing::GetVdsoVmo()->duplicate(ZX_RIGHT_SAME_RIGHTS, &vdso_vmo);
  EXPECT_EQ(status, ZX_OK) << zx_status_get_string(status);

  Linker::Module::DecodedPtr decoded_vdso =
      Linker::Module::Decoded::Create(diag, std::move(vdso_vmo), kPageSize);
  EXPECT_TRUE(decoded_vdso);

  auto init_result = linker.Init(  //
      diag,
      {Linker::Preplaced(std::move(decoded_executable), load_address,
                         ld::abi::Abi<>::kExecutableName),
       Linker::Implicit(std::move(decoded_vdso))},
      GetDepFunction(diag));
  ASSERT_TRUE(init_result);

  EXPECT_TRUE(linker.Allocate(diag, root_vmar().borrow()));
  set_entry(linker.main_entry());
  set_stack_size(linker.main_stack_size());
  set_vdso_base(init_result->back()->module().vaddr_start());

  EXPECT_EQ(init_result->front()->module().vaddr_start, load_address);

  EXPECT_TRUE(linker.Relocate(diag));
  ASSERT_TRUE(linker.Load(diag));
  linker.Commit();

  EXPECT_EQ(Run(), static_cast<int64_t>(load_address));

  ExpectLog("");
}

// This demonstrates performing two separate dynamic linking sessions to
// establish two distinct dynamic linking namespaces inside one process address
// space, where the second session uses the first session's initial modules
// (but not their dependencies) as preloaded implicit modules that can satisfy
// its symbols.
TEST_F(LdRemoteTests, SecondSession) {
  constexpr int64_t kReturnValue = 17;

  ASSERT_NO_FATAL_FAILURE(Init());

  LdsvcPathPrefix("second-session");

  // The ld::RemoteAbiStub only needs to be set up once for all sessions.
  ld::RemoteAbiStub<>::Ptr abi_stub;
  {
    auto diag = elfldltl::testing::ExpectOkDiagnostics();
    abi_stub = ld::RemoteAbiStub<>::Create(diag, TakeStubLdVmo(), kPageSize);
    ASSERT_TRUE(abi_stub);
  }

  zx::vmo exec_vmo = ld::testing::GetExecutableVmo("second-session");
  ASSERT_TRUE(exec_vmo);
  ConfigFromInterp(exec_vmo.borrow());

  // First do a complete dynamic linking session for the main executable.
  ASSERT_NO_FATAL_FAILURE(Needed({
      "libindirect-deps-a.so",
      "libindirect-deps-b.so",
      "libindirect-deps-c.so",
  }));
  ASSERT_NO_FATAL_FAILURE(LdsvcExpectNeeded());
  constexpr std::string_view kMainSoname = "libsecond-session-test.so.1";
  Linker::InitModuleList initial_modules;
  std::string vdso_soname;
  {
    Linker linker;
    linker.set_abi_stub(abi_stub);

    auto diag = elfldltl::testing::ExpectOkDiagnostics();
    Linker::Module::DecodedPtr decoded_executable =
        Linker::Module::Decoded::Create(diag, std::move(exec_vmo), kPageSize);
    EXPECT_TRUE(decoded_executable);

    zx::vmo vdso_vmo;
    zx_status_t status = ld::testing::GetVdsoVmo()->duplicate(ZX_RIGHT_SAME_RIGHTS, &vdso_vmo);
    EXPECT_EQ(status, ZX_OK) << zx_status_get_string(status);

    Linker::Module::DecodedPtr decoded_vdso =
        Linker::Module::Decoded::Create(diag, std::move(vdso_vmo), kPageSize);
    EXPECT_TRUE(decoded_vdso);
    vdso_soname = decoded_vdso->soname().str();

    auto init_result = linker.Init(  //
        diag,
        {Linker::Executable(std::move(decoded_executable)),
         Linker::Implicit(std::move(decoded_vdso))},
        GetDepFunction(diag));
    ASSERT_TRUE(init_result);

    // Check on expected get_dep callbacks made by Init,
    // and wipe the test fixture mock clean for the later session.
    ASSERT_NO_FATAL_FAILURE(VerifyAndClearNeeded());

    EXPECT_TRUE(linker.Allocate(diag, root_vmar().borrow()));
    set_entry(linker.main_entry());
    set_stack_size(linker.main_stack_size());
    set_vdso_base(init_result->back()->module().vaddr_start());

    EXPECT_TRUE(linker.Relocate(diag));
    ASSERT_TRUE(linker.Load(diag));
    linker.Commit();

    // Extract both initial modules to be preloaded implicit modules.
    initial_modules = linker.PreloadedImplicit(*init_result);
    EXPECT_EQ(initial_modules.size(), 2u);

    // The primary domain has more modules than just those.
    EXPECT_GT(linker.modules().size(), 2u);
  }

  // Start the process running now with just the primary domain in place.
  // It will block on reading from the bootstrap channel.
  zx::channel bootstrap_sender = Start(true);
  ASSERT_TRUE(bootstrap().empty());
  ASSERT_TRUE(bootstrap_sender);

  // The other end was never actually used, so don't keep it alive.
  process_log_fd().reset();

  ASSERT_NO_FATAL_FAILURE(CheckProcess());
  ASSERT_NO_FATAL_FAILURE(CheckVmar());

  // Now do a second session using the InitModule::AlreadyLoaded main
  // executable and vDSO from the first session as implicit modules.
  Linker::size_type test_start_fnptr = 0;
  {
    constexpr Linker::Soname kPathPrefix{"second-session-module"};
    constexpr Linker::Soname kRootModule{"second-session-module.so"};
    constexpr std::string_view kDepModule = "libsecond-session-module-deps-a.so";

    Linker second_linker;
    second_linker.set_abi_stub(abi_stub);

    // Point GetLibVmo() to the different place for this module's deps.
    LdsvcPathPrefix(kPathPrefix.str());

    // Acquire the VMO for the root module.
    zx::vmo module_vmo;
    ASSERT_NO_FATAL_FAILURE(module_vmo = GetLibVmo(kRootModule.str()));

    // Decode the root module.
    Linker::Module::DecodedPtr decoded_module;
    {
      auto diag = elfldltl::testing::ExpectOkDiagnostics();
      decoded_module = Linker::Module::Decoded::Create(diag, std::move(module_vmo), kPageSize);
      EXPECT_TRUE(decoded_module);
    }

    // Add in the root module with the implicit modules from the first session.
    initial_modules.emplace_back(Linker::RootModule(decoded_module, kRootModule));

    // Prime fresh expectations for get_dep callbacks from this session.  There
    // is no PT_INTERP in the root module to key off, so rely on the build-time
    // record to find where dependencies got packaged.
    ASSERT_NO_FATAL_FAILURE(VerifyAndClearNeeded());
    ASSERT_NO_FATAL_FAILURE(NeededViaLoadSet(kPathPrefix, {kDepModule}));
    ASSERT_NO_FATAL_FAILURE(LdsvcExpectNeeded());

    // Now resolve dependencies, including the preloaded implicit modules as
    // well as that Needed list, modules newly opened via the get_dep callback.
    auto init_diag = elfldltl::testing::ExpectOkDiagnostics();
    auto init_result =
        second_linker.Init(init_diag, std::move(initial_modules), GetDepFunction(init_diag));
    ASSERT_TRUE(init_result);
    ASSERT_EQ(init_result->size(), 3u);

    EXPECT_EQ(init_result->front()->name().str(), kMainSoname);
    EXPECT_EQ(init_result->at(1)->name().str(), vdso_soname);
    EXPECT_EQ(init_result->back()->name().str(), kRootModule.str());

    EXPECT_TRUE(init_result->front()->preloaded());
    EXPECT_TRUE(init_result->at(1)->preloaded());
    EXPECT_FALSE(init_result->back()->preloaded());

    ASSERT_EQ(second_linker.modules().size(), 5u);
    EXPECT_EQ(second_linker.modules().at(0).name().str(), kRootModule.str());
    EXPECT_EQ(second_linker.modules().at(1).name().str(), kMainSoname);
    EXPECT_TRUE(second_linker.modules().at(1).preloaded());
    EXPECT_EQ(second_linker.modules().at(2).name().str(), kDepModule);
    EXPECT_EQ(second_linker.modules().at(3).name().str(), vdso_soname);
    EXPECT_TRUE(second_linker.modules().at(3).preloaded());
    EXPECT_EQ(second_linker.modules().at(4).name().str(), ld::abi::Abi<>::kSoname.str());

    ASSERT_NO_FATAL_FAILURE(VerifyAndClearNeeded());

    // The second session uses the default TLS resolver.
    constexpr auto is_preloaded = [](const auto& module) -> bool { return module.preloaded(); };
    constexpr auto has_no_tls = [](const auto& module) -> bool {
      return module.tls_module_id() == 0 && !module.uses_static_tls();
    };
    EXPECT_THAT(second_linker.modules(),  //
                Each(AnyOf(Truly(is_preloaded), Truly(has_no_tls))))
        << "some module in the second linking session has PT_TLS or uses IE";

    // Allocate should place the root module and leave preloaded ones alone.
    ASSERT_NO_FATAL_FAILURE(CheckProcess());
    ASSERT_NO_FATAL_FAILURE(CheckVmar());
    {
      auto diag = elfldltl::testing::ExpectOkDiagnostics();
      EXPECT_TRUE(second_linker.Allocate(diag, root_vmar().borrow()));
      EXPECT_TRUE(init_result->front()->preloaded());
      EXPECT_TRUE(init_result->at(1)->preloaded());
      EXPECT_FALSE(init_result->back()->preloaded());
    }

    // Finish dynamic linking.
    {
      auto diag = elfldltl::testing::ExpectOkDiagnostics();
      EXPECT_TRUE(second_linker.Relocate(diag));
    }
    {
      auto diag = elfldltl::testing::ExpectOkDiagnostics();
      ASSERT_TRUE(second_linker.Load(diag));
    }
    second_linker.Commit();

    // Look up the module's entry-point symbol.
    constexpr elfldltl::SymbolName kTestStart{"TestStart"};
    auto* symbol = kTestStart.Lookup(second_linker.main_module().module().symbols);
    ASSERT_TRUE(symbol);
    test_start_fnptr = symbol->value + second_linker.main_module().load_bias();
  }
  EXPECT_NE(test_start_fnptr, 0u);

  // The process is already running and it will block until it reads the
  // function pointer from the bootstrap channel.
  zx_status_t status =
      bootstrap_sender.write(0, &test_start_fnptr, sizeof(test_start_fnptr), nullptr, 0);
  ASSERT_EQ(status, ZX_OK) << "zx_channel_write: " << zx_status_get_string(status);

  // Close our end of the channel before waiting for the process, just in case
  // that kicks it out of a block and into crashing rather than wedging.
  bootstrap_sender.reset();

  // The process should now call TestStart() and exit with its return value.
  EXPECT_EQ(Wait(), kReturnValue);

  ExpectLog("");
}

TEST_F(LdRemoteTests, Zygote) {
  constexpr int64_t kReturnValue = 17;
  constexpr int64_t kSecondaryReturnValue = 23;
  constexpr int kZygoteCount = 10;

  LdsvcPathPrefix("zygote");

  auto diag = elfldltl::testing::ExpectOkDiagnostics();

  const ld::RemoteAbiStub<>::Ptr abi_stub =
      ld::RemoteAbiStub<>::Create(diag, TakeStubLdVmo(), kPageSize);
  ASSERT_TRUE(abi_stub);

  // Linker::Module::Decoded and ZygoteLinker::Module::Decoded are the same but
  // Linker and ZygoteLinker are not quite the same.
  using ZygoteLinker = ld::RemoteZygote<>::Linker;
  ZygoteLinker linker{abi_stub};

  zx::vmo exec_vmo = ld::testing::GetExecutableVmo("zygote");
  ASSERT_TRUE(exec_vmo);
  ConfigFromInterp(exec_vmo.borrow());

  zx::vmo vdso_vmo;
  zx_status_t status = ld::testing::GetVdsoVmo()->duplicate(ZX_RIGHT_SAME_RIGHTS, &vdso_vmo);
  EXPECT_EQ(status, ZX_OK) << zx_status_get_string(status);

  Linker::Module::DecodedPtr executable =
      Linker::Module::Decoded::Create(diag, std::move(exec_vmo), kPageSize);
  Linker::Module::DecodedPtr vdso =
      Linker::Module::Decoded::Create(diag, std::move(vdso_vmo), kPageSize);
  ASSERT_TRUE(executable);
  ASSERT_TRUE(vdso);

  ZygoteLinker::InitModuleList init_modules{{
      ZygoteLinker::Executable(std::move(executable)),
      ZygoteLinker::Implicit(std::move(vdso)),
  }};
  auto get_dep = GetDepFunction(diag);  // Needed primes this.
  ASSERT_NO_FATAL_FAILURE(Needed({"libzygote-dep.so"}));
  ASSERT_NO_FATAL_FAILURE(LdsvcExpectNeeded());
  auto init_result = linker.Init(diag, std::move(init_modules), get_dep);
  ASSERT_TRUE(init_result);
  VerifyAndClearNeeded();

  // Create a process that will be the first to run.  Its ASLR will choose the
  // load addresses used again for all later zygote processes.
  ASSERT_NO_FATAL_FAILURE(Init());

  EXPECT_TRUE(linker.Allocate(diag, root_vmar().borrow()));
  ASSERT_TRUE(linker.Relocate(diag));
  ASSERT_TRUE(linker.Load(diag));

  // Collect what's needed to start the process.
  const ZygoteLinker::Module& loaded_vdso = *init_result->back();
  set_vdso_base(loaded_vdso.module().vaddr_start());
  set_entry(linker.main_entry());
  set_stack_size(linker.main_stack_size());

  // The prototype process is ready to start.  The linker object is now
  // consumed in making the zygote.
  linker.Commit();

  // Capture the settled load details of the executable and vDSO for later.
  ZygoteLinker::InitModuleList secondary_init_modules = linker.PreloadedImplicit(*init_result);

  // Make a zygote that holds onto the DecodedPtr references.
  ld::RemoteZygote<ld::RemoteZygoteVmo::kDecodedPtr> original_zygote;
  auto result = original_zygote.Insert(std::move(linker));
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  EXPECT_EQ(result->main_entry(), entry());
  EXPECT_EQ(result->main_stack_size(), stack_size());

  // The += operator allows for splicing that cannot fail, since the DecodedPtr
  // references just transfer from the other zygote.
  original_zygote += ld::RemoteZygote<ld::RemoteZygoteVmo::kDecodedPtr>{};

  {
    // Run the prototype process to completion.  It will have changed its segment
    // contents, but they should not be shared with later runs.
    auto [exit_code, bootstrap_sender] = RunWithCustomBootstrap();
    EXPECT_EQ(exit_code, kReturnValue);
    EXPECT_TRUE(bootstrap_sender);
    ExpectLog("");

    // Discard the channel endpoint to the defunct process.
    // Each later Run() call will create a new channel for its new process.
    bootstrap_sender.reset();
  }

  // Move into a zygote that owns only zx::vmo and not DecodedPtr.  Splicing
  // into this from the zygote that owns DecodedPtr instead can fail.
  ld::RemoteZygote zygote;
  auto splice = zygote.Splice(std::move(original_zygote));
  EXPECT_TRUE(splice.is_ok()) << splice.status_string();

  // The += operator allows for splicing that cannot fail, since the other
  // object already owns zx::vmo handles directly and they just transfer.
  zygote += ld::RemoteZygote{};

  for (int i = 1; i <= kZygoteCount; ++i) {
    // Make a new process.
    ASSERT_NO_FATAL_FAILURE(Init()) << "zygote child " << i << " of " << kZygoteCount;

    // Load it up from the zygote.
    EXPECT_TRUE(zygote.Load(diag, root_vmar().borrow()))
        << "zygote child " << i << " of " << kZygoteCount;

    // Run it to completion.  It would go wrong or return the wrong value if
    // its segments had been written by an earlier run.
    auto [exit_code, bootstrap_sender] = RunWithCustomBootstrap();
    EXPECT_EQ(exit_code, kReturnValue) << "zygote child " << i << " of " << kZygoteCount;
    EXPECT_TRUE(bootstrap_sender);
    ExpectLog("");

    // Discard the channel endpoint to the defunct process.
    // The next iteration will create a new channel for the next process.
    bootstrap_sender.reset();
  }

  // Start a new process for the secondary session test.
  ASSERT_NO_FATAL_FAILURE(Init()) << "secondary";

  // First the new process gets loaded up from the zygote like the others.
  EXPECT_TRUE(zygote.Load(diag, root_vmar().borrow())) << "secondary";

  // Fetch the secondary domain's root module.  It's built and packaged as an
  // executable since it has an entry point that acts like one.
  constexpr ZygoteLinker::Soname kSecondaryName{"zygote-secondary"};
  LdsvcPathPrefix(kSecondaryName.str());
  zx::vmo secondary_vmo;
  ASSERT_NO_FATAL_FAILURE(secondary_vmo = ld::testing::GetExecutableVmo(kSecondaryName.str()));
  EXPECT_TRUE(secondary_vmo);
  ConfigFromInterp(secondary_vmo.borrow());
  Linker::Module::DecodedPtr secondary =
      Linker::Module::Decoded::Create(diag, std::move(secondary_vmo), kPageSize);

  // Now start the secondary session.  The secondary_init_modules list
  // collected above still corresponds to where the zygote loaded things.
  ZygoteLinker secondary_linker{abi_stub};
  secondary_init_modules.emplace_back(
      ZygoteLinker::RootModule(std::move(secondary), kSecondaryName));
  ASSERT_NO_FATAL_FAILURE(Needed({"libzygote-dep.so"}));
  ASSERT_NO_FATAL_FAILURE(LdsvcExpectNeeded());
  auto secondary_init_result =
      secondary_linker.Init(diag, std::move(secondary_init_modules), get_dep);
  ASSERT_TRUE(secondary_init_result);

  EXPECT_TRUE(secondary_linker.Allocate(diag, root_vmar().borrow()));
  ASSERT_TRUE(secondary_linker.Relocate(diag));
  ASSERT_TRUE(secondary_linker.Load(diag));

  // This process will start at the secondary module's entry point rather than
  // the original executable's.  The set_vdso_base() call above is still in
  // force, since that has not changed since the original session.
  set_entry(secondary_linker.main_entry());
  set_stack_size(secondary_linker.main_stack_size());

  // The prototype secondary process is ready to start.
  secondary_linker.Commit();

  // Consume the secondary_linker object in the existing zygote, so now it will
  // load both the original and secondary modules into each new process.
  auto secondary_result = zygote.Insert(std::move(secondary_linker));
  ASSERT_TRUE(secondary_result.is_ok()) << secondary_result.status_string();
  EXPECT_EQ(secondary_result->main_entry(), entry());
  EXPECT_EQ(secondary_result->main_stack_size(), stack_size());

  {
    // Run the prototype secondary process to completion.
    auto [exit_code, bootstrap_sender] = RunWithCustomBootstrap();
    EXPECT_EQ(exit_code, kSecondaryReturnValue);
    EXPECT_TRUE(bootstrap_sender);
    ExpectLog("");

    bootstrap_sender.reset();
  }

  // Test the combined zygote behaves like the secondary prototype over again.
  for (int i = 1; i <= kZygoteCount; ++i) {
    ASSERT_NO_FATAL_FAILURE(Init()) << "secondary zygote child " << i << " of " << kZygoteCount;

    EXPECT_TRUE(zygote.Load(diag, root_vmar().borrow()))
        << "secondary zygote child " << i << " of " << kZygoteCount;

    auto [exit_code, bootstrap_sender] = RunWithCustomBootstrap();
    EXPECT_EQ(exit_code, kSecondaryReturnValue)
        << "secondary zygote child " << i << " of " << kZygoteCount;
    EXPECT_TRUE(bootstrap_sender);
    ExpectLog("");

    bootstrap_sender.reset();
  }
}

// Clang doesn't generate test binaries with `DataWithZeroFillSegment`s, so our
// other tests don't cover operations involving them. Artificially create this
// type of segment.
TEST_F(LdRemoteTests, ReplaceSegmentDataWithZeroFillSegment) {
  using DataWithZeroFillSegment = typename Linker::Module::LoadInfo::DataWithZeroFillSegment;
  using RemoteAbiHeap = typename ld::RemoteAbiHeap<Linker::Module, elfldltl::RemoteAbiTraits>;
  using Phdr = typename elfldltl::Elf<>::Phdr;

  auto diag = elfldltl::testing::ExpectOkDiagnostics();

  const size_t kFilesz = kPageSize;
  const size_t kMemsz = kPageSize * 5;  // must be larger than filesz

  zx::vmo exec_vmo;
  {
    zx_status_t status = zx::vmo::create(kFilesz, 0, &exec_vmo);
    ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
  }

  Linker::Module::LoadInfo load_info;

  Phdr phdr{.flags = Phdr::kRead | Phdr::kWrite, .filesz = kFilesz, .memsz = kMemsz};
  ASSERT_TRUE(load_info.AddSegment(diag, kPageSize, phdr));

  auto last_segment_var = load_info.RemoveLastSegment();
  ASSERT_THAT(last_segment_var, VariantWith<DataWithZeroFillSegment>(::testing::_));
  DataWithZeroFillSegment& last_segment = std::get<DataWithZeroFillSegment>(last_segment_var);

  zx::result<RemoteAbiHeap::StubConstantSegment> r =
      RemoteAbiHeap::ReplaceSegment(diag, std::move(last_segment), exec_vmo, kMemsz);
  EXPECT_TRUE(r.is_ok()) << r.status_string();
}

TEST_F(LdRemoteTests, RemoteAbiStub) {
  auto diag = elfldltl::testing::ExpectOkDiagnostics();

  // Acquire the layout details from the stub.  The same values collected here
  // can be reused along with the decoded RemoteLoadModule for the stub for
  // creating and populating the RemoteLoadModule for the passive ABI of any
  // number of separate dynamic linking domains in however many processes.
  ld::RemoteAbiStub<>::Ptr abi_stub = ld::RemoteAbiStub<>::Create(diag, TakeStubLdVmo(), kPageSize);
  ASSERT_TRUE(abi_stub);
  EXPECT_GE(abi_stub->data_size(), sizeof(ld::abi::Abi<>) + sizeof(elfldltl::Elf<>::RDebug<>));
  EXPECT_LT(abi_stub->data_size(), kPageSize);
  EXPECT_LE(abi_stub->abi_offset(), abi_stub->data_size() - sizeof(ld::abi::Abi<>));
  EXPECT_LE(abi_stub->rdebug_offset(), abi_stub->data_size() - sizeof(elfldltl::Elf<>::RDebug<>));
  EXPECT_NE(abi_stub->rdebug_offset(), abi_stub->abi_offset())
      << "with data_size() " << abi_stub->data_size();

  // Verify that the TLSDESC entry points were found in the stub and that
  // their addresses pass some basic smell tests.
  std::set<elfldltl::Elf<>::size_type> tlsdesc_entrypoints;
  const auto segment_is_executable = [](const auto& segment) -> bool {
    return segment.executable();
  };
  const Linker::Module::Decoded& stub_module = *abi_stub->decoded_module();
  for (const elfldltl::Elf<>::size_type entry : abi_stub->tlsdesc_runtime()) {
    // Must be nonzero.
    EXPECT_NE(entry, 0u);

    // Must lie within the module bounds.
    EXPECT_GT(entry, stub_module.load_info().vaddr_start());
    EXPECT_LT(entry - stub_module.load_info().vaddr_start(), stub_module.load_info().vaddr_size());

    // Must be inside an executable segment.
    auto segment = stub_module.load_info().FindSegment(entry);
    ASSERT_NE(segment, stub_module.load_info().segments().end());
    EXPECT_TRUE(std::visit(segment_is_executable, *segment));

    // Must be unique.
    auto [it, inserted] = tlsdesc_entrypoints.insert(entry);
    EXPECT_TRUE(inserted) << "duplicate entry point " << entry;
  }
  EXPECT_EQ(tlsdesc_entrypoints.size(), ld::kTlsdescRuntimeCount);
}

TEST_F(LdRemoteTests, LoadedBy) {
  auto diag = elfldltl::testing::ExpectOkDiagnostics();

  // Acquire the layout details from the stub.  The same values collected here
  // can be reused along with the decoded RemoteLoadModule for the stub for
  // creating and populating the RemoteLoadModule for the passive ABI of any
  // number of separate dynamic linking domains in however many processes.
  Linker linker;
  linker.set_abi_stub(ld::RemoteAbiStub<>::Create(diag, TakeStubLdVmo(), kPageSize));
  ASSERT_TRUE(linker.abi_stub());

  LdsvcPathPrefix("many-deps");

  // Decode the main executable.
  zx::vmo vmo = ld::testing::GetExecutableVmo("many-deps");
  ASSERT_TRUE(vmo);
  ConfigFromInterp(vmo.borrow());

  // Prime expectations for its dependencies.
  ASSERT_NO_FATAL_FAILURE(Needed({
      "libld-dep-a.so",
      "libld-dep-b.so",
      "libld-dep-f.so",
      "libld-dep-c.so",
      "libld-dep-d.so",
      "libld-dep-e.so",
  }));
  ASSERT_NO_FATAL_FAILURE(LdsvcExpectNeeded());

  Linker::InitModuleList initial_modules{{
      Linker::Executable(Linker::Module::Decoded::Create(diag, std::move(vmo), kPageSize)),
  }};
  ASSERT_TRUE(initial_modules.front().decoded_module);
  ASSERT_TRUE(initial_modules.front().decoded_module->HasModule());

  // Pre-decode the vDSO.
  zx::vmo vdso_vmo;
  zx_status_t status = ld::testing::GetVdsoVmo()->duplicate(ZX_RIGHT_SAME_RIGHTS, &vdso_vmo);
  ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);

  initial_modules.push_back(
      Linker::Implicit(Linker::Module::Decoded::Create(diag, std::move(vdso_vmo), kPageSize)));
  ASSERT_TRUE(initial_modules.back().decoded_module);
  ASSERT_TRUE(initial_modules.back().decoded_module->HasModule());

  auto init_result = linker.Init(diag, initial_modules, GetDepFunction(diag));
  ASSERT_TRUE(init_result);
  ASSERT_EQ(init_result->size(), initial_modules.size());

  // The root module went on the list first.
  const auto& modules = linker.modules();
  EXPECT_EQ(init_result->front(), modules.begin());

  // The vDSO module went somewhere on the list.
  EXPECT_NE(init_result->back(), modules.end());

  // Check the loaded-by pointers.
  EXPECT_FALSE(modules.front().loaded_by_modid())
      << "executable loaded by " << modules[*modules.front().loaded_by_modid()].name();
  {
    auto next_module = std::next(modules.begin());
    auto loaded_by_name = [next_module, &modules]() -> std::string_view {
      if (next_module->loaded_by_modid()) {
        return modules[*next_module->loaded_by_modid()].name().str();
      }
      return "<none>";
    };
    if (next_module != modules.end() && next_module->HasModule() &&
        next_module->module().symbols_visible) {
      // The second module must be a direct dependency of the executable.
      EXPECT_THAT(next_module->loaded_by_modid(), ::testing::Optional(0u))
          << " second module " << next_module->name().str() << " loaded by " << loaded_by_name();
    }
    for (; next_module != modules.end(); ++next_module) {
      if (!next_module->HasModule()) {
        continue;
      }
      if (next_module->module().symbols_visible) {
        // This module wouldn't be here if it wasn't loaded by someone.
        EXPECT_NE(next_module->loaded_by_modid(), std::nullopt)
            << "visible module " << next_module->name().str() << " loaded by " << loaded_by_name();
      } else {
        // A predecoded module was not referenced, so it's loaded by no-one.
        EXPECT_EQ(next_module->loaded_by_modid(), std::nullopt)
            << "invisible module " << next_module->name().str() << " loaded by "
            << loaded_by_name();
      }
    }
  }
}

TEST_F(LdRemoteTests, SymbolFilter) {
  ASSERT_NO_FATAL_FAILURE(Init());

  LdsvcPathPrefix("symbol-filter");

  auto diag = elfldltl::testing::ExpectOkDiagnostics();

  Linker linker;
  linker.set_abi_stub(ld::RemoteAbiStub<>::Create(diag, TakeStubLdVmo(), kPageSize));
  ASSERT_TRUE(linker.abi_stub());

  zx::vmo vdso_vmo;
  zx_status_t status = ld::testing::GetVdsoVmo()->duplicate(ZX_RIGHT_SAME_RIGHTS, &vdso_vmo);
  EXPECT_EQ(status, ZX_OK) << zx_status_get_string(status);

  zx::vmo exec_vmo = ld::testing::GetExecutableVmo("symbol-filter");
  ASSERT_TRUE(exec_vmo);
  ConfigFromInterp(exec_vmo.borrow());

  auto decode = [](zx::vmo vmo) {
    auto diag = elfldltl::testing::ExpectOkDiagnostics();
    return Linker::Module::Decoded::Create(diag, std::move(vmo), kPageSize);
  };

  Linker::InitModuleList init_modules = {
      Linker::Executable(decode(std::move(exec_vmo))),
      Linker::Implicit(decode(GetLibVmo("libsymbol-filter-dep17.so"))),
      Linker::Implicit(decode(GetLibVmo("libsymbol-filter-dep23.so"))),
      Linker::Implicit(decode(GetLibVmo("libsymbol-filter-dep42.so"))),
      Linker::Implicit(decode(std::move(vdso_vmo))),
  };
  ASSERT_THAT(init_modules,
              Each(Field("decoded_module", &Linker::InitModule::decoded_module,
                         Pointee(Property("successfully decoded", &Linker::DecodedModule::HasModule,
                                          IsTrue())))));

  auto init_result = linker.Init(diag, std::move(init_modules), GetDepFunction(diag));
  ASSERT_TRUE(init_result);

  auto filter_out = [](auto... ignore_names) {
    return [ignore_names...](const auto& module,
                             auto& name) -> fit::result<bool, const elfldltl::Elf<>::Sym*> {
      auto diag = elfldltl::testing::ExpectOkDiagnostics();
      const bool ignore = ((name == ignore_names) || ...);
      return fit::ok(ignore ? nullptr : name.Lookup(module.symbol_info()));
    };
  };

  // Dependency order should be dep17, dep23, dep42.
  EXPECT_EQ(std::next(init_result->at(1)), init_result->at(2));
  EXPECT_EQ(std::next(init_result->at(2)), init_result->at(3));

  // first can come from dep17, but not second or third.
  init_result->at(1)->set_symbol_filter(filter_out("second", "third"));

  // first and second can come from dep23, but not third.
  init_result->at(2)->set_symbol_filter(filter_out("third"));

  // Hence: first from dep17; second from dep23; third from dep42.
  constexpr int64_t kReturnValue = (17 * 1) + (23 * 2) + (42 * 3);

  EXPECT_TRUE(linker.Allocate(diag, root_vmar().borrow()));
  set_entry(linker.main_entry());
  set_stack_size(linker.main_stack_size());
  set_vdso_base(init_result->back()->module().vaddr_start());

  EXPECT_TRUE(linker.Relocate(diag));
  ASSERT_TRUE(linker.Load(diag));
  linker.Commit();

  EXPECT_EQ(Run(), kReturnValue);

  ExpectLog("");
}

// This reuses one of the modules from the SymbolFilter test, but it only uses
// the test fixture to acquire the VMO (and the cached page size).  It doesn't
// do any dynamic linking, it just decodes a module and then unit-tests the
// generated filter function.
template <class Elf, class Test>
void PerfectSymbolFilterTest(Test& test, std::string_view path_prefix) {
  using size_type = Elf::size_type;
  using Sym = Elf::Sym;
  using Module = ld::RemoteLoadModule<Elf>;

  test.LdsvcPathPrefix(path_prefix);
  {
    // The only need to find the nominal executable is to inform GetLibVmo
    // where to look in the test packaging.
    zx::vmo exec_vmo;
    if constexpr (Elf::kClass == elfldltl::ElfClass::k32) {
      // The elf32 file is not packaged quite normally yet.
      const std::string executable_path =
          std::filesystem::path("test") / path_prefix / "lib" / path_prefix;
      exec_vmo = elfldltl::testing::GetTestLibVmo(executable_path);
    } else {
      exec_vmo = ld::testing::GetExecutableVmo(path_prefix);
    }
    test.ConfigFromInterp(exec_vmo.borrow());
  }

  auto diag = elfldltl::testing::ExpectOkDiagnostics();
  auto decoded = Module::Decoded::Create(diag, test.GetLibVmo("libsymbol-filter-dep17.so"),
                                         static_cast<size_type>(Test::kPageSize));
  ASSERT_TRUE(decoded);
  ASSERT_TRUE(decoded->HasModule());

  // The original module has all three symbols.

  constexpr elfldltl::SymbolName kFirst = "first";
  const Sym* first_sym = kFirst.Lookup(decoded->symbol_info());
  EXPECT_NE(first_sym, nullptr);

  constexpr elfldltl::SymbolName kSecond = "second";
  const Sym* second_sym = kSecond.Lookup(decoded->symbol_info());
  EXPECT_NE(second_sym, nullptr);

  constexpr elfldltl::SymbolName kThird = "third";
  const Sym* third_sym = kThird.Lookup(decoded->symbol_info());
  EXPECT_NE(third_sym, nullptr);

  // These are distinct symbols.
  EXPECT_NE(first_sym, second_sym);
  EXPECT_NE(first_sym, third_sym);
  EXPECT_NE(second_sym, third_sym);

  // Populate the filter.
  typename Module::SymbolFilter filter = ld::testing::PerfectSymbolFilterTest<Elf>(diag, decoded);
  ASSERT_TRUE(filter);

  // Mock up a module object.  It won't be referenced by calls to the filter.
  Module mod;

  // First symbol is found by the filter.
  elfldltl::SymbolName name = kFirst;
  auto first_result = filter(mod, name);
  ASSERT_TRUE(first_result.is_ok()) << first_result.error_value();
  EXPECT_EQ(*first_result, first_sym);

  // Second symbol is filtered out: not found.
  name = kSecond;
  auto second_result = filter(mod, name);
  ASSERT_TRUE(second_result.is_ok()) << second_result.error_value();
  EXPECT_EQ(*second_result, nullptr);

  // Third symbol is found by the filter.
  name = kThird;
  auto third_result = filter(mod, name);
  ASSERT_TRUE(third_result.is_ok()) << third_result.error_value();
  EXPECT_EQ(*third_result, third_sym);

  // Now install the filter and get the same results via the module.  Nothing
  // else will be used, so the module stays otherwise default-constructed.
  mod.set_symbol_filter(std::move(filter));

  name = kFirst;
  first_result = mod.Lookup(diag, name);
  ASSERT_TRUE(first_result.is_ok()) << first_result.error_value();
  EXPECT_EQ(*first_result, first_sym);

  name = kSecond;
  second_result = mod.Lookup(diag, name);
  ASSERT_TRUE(second_result.is_ok()) << second_result.error_value();
  EXPECT_EQ(*second_result, nullptr);

  name = kThird;
  third_result = mod.Lookup(diag, name);
  ASSERT_TRUE(third_result.is_ok()) << third_result.error_value();
  EXPECT_EQ(*third_result, third_sym);
}

TEST_F(LdRemoteTests, PerfectSymbolFilter) {
  ASSERT_NO_FATAL_FAILURE(PerfectSymbolFilterTest<elfldltl::Elf<>>(*this, "symbol-filter"));
}

TEST_F(LdRemoteTests, PerfectSymbolFilterElf32) {
  ASSERT_NO_FATAL_FAILURE(PerfectSymbolFilterTest<elfldltl::Elf32<>>(*this, "symbol-filter-elf32"));
}

TEST_F(LdRemoteTests, ForeignMachine) {
  using ForeignElf = elfldltl::Elf32<elfldltl::ElfData::k2Lsb>;
  constexpr elfldltl::ElfMachine kForeignMachine = elfldltl::ElfMachine::kArm;
  constexpr uint32_t kForeignPageSize = 0x1000;

  using ForeignLinker =
      ld::RemoteDynamicLinker<ForeignElf, ld::RemoteLoadZygote::kNo, kForeignMachine>;

  using ForeignStub = ForeignLinker::AbiStub;

  // Init() creates the process where the test modules will be loaded, and
  // provides its root VMAR.  The modules understand only a 32-bit address
  // space, so they must go into the low 4GiB of the test process.
  ASSERT_NO_FATAL_FAILURE(Init());

  // Reset anything that will never be used since the process never starts.
  // This drops references held to be transferred in a bootstrap message,
  // which can keep things alive longer than they should be.
  ASSERT_NO_FATAL_FAILURE(NeverStart());

  // The kernel reserves the lowest part of the address space, so the root VMAR
  // doesn't start at zero.  The VMAR for the 32-bit address space will not be
  // quite 4GiB in size, so adjust to make sure it ends at exactly 4GiB.  In
  // fact, no 32-bit userland ever expects to have a segment in the very last
  // page, where the page-rounded vaddr+memsz wraps around to 0.  So make the
  // VMAR one page smaller to ensure nothing gets placed all the way up there.
  const size_t kAddressLimit = (size_t{1} << 32) - zx_system_get_page_size();
  zx_info_vmar_t root_vmar_info;
  zx_status_t status =
      root_vmar().get_info(ZX_INFO_VMAR, &root_vmar_info, sizeof(root_vmar_info), nullptr, nullptr);
  ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
  ASSERT_LT(root_vmar_info.base, kAddressLimit);

  constexpr zx_vm_option_t kVmarOptions =
      // Require the specific offset of 0 and allow exact placement within.
      ZX_VM_SPECIFIC | ZX_VM_CAN_MAP_SPECIFIC |
      // Allow all kinds of mappings.
      ZX_VM_CAN_MAP_READ | ZX_VM_CAN_MAP_WRITE | ZX_VM_CAN_MAP_EXECUTE;
  const size_t vmar_size = kAddressLimit - root_vmar_info.base;
  zx::vmar vmar;
  uintptr_t vmar_addr;
  status = root_vmar().allocate(kVmarOptions, 0, vmar_size, &vmar, &vmar_addr);
  ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);
  EXPECT_EQ(vmar_addr, root_vmar_info.base);

  LdsvcPathPrefix("symbol-filter-elf32");

  zx::vmo stub_vmo = elfldltl::testing::GetTestLibVmo(ForeignStub::kFilename);
  ASSERT_TRUE(stub_vmo);

  auto diag = elfldltl::testing::ExpectOkDiagnostics();
  ForeignLinker linker;
  linker.set_abi_stub(ForeignStub::Create(diag, std::move(stub_vmo), kForeignPageSize));
  ASSERT_TRUE(linker.abi_stub());

  // The non-Fuchsia executable gets packaged under lib/ in the test data.
  zx::vmo exec_vmo = GetLibVmo("symbol-filter-elf32");
  ASSERT_TRUE(exec_vmo);
  ConfigFromInterp(exec_vmo.borrow());

  ForeignLinker::Module::DecodedPtr executable;
  ASSERT_TRUE(executable = ForeignLinker::Module::Decoded::Create(  //
                  diag, std::move(exec_vmo), kForeignPageSize));

  auto get_dep = GetDepFunction<ForeignLinker>(diag, kForeignPageSize);
  ASSERT_NO_FATAL_FAILURE(Needed({
      "libsymbol-filter-dep17.so",
      "libsymbol-filter-dep23.so",
      "libsymbol-filter-dep42.so",
  }));
  ASSERT_NO_FATAL_FAILURE(LdsvcExpectNeeded());

  auto init_result = linker.Init(diag, {ForeignLinker::Executable(std::move(executable))}, get_dep,
                                 kForeignMachine);
  ASSERT_TRUE(init_result);

  EXPECT_TRUE(linker.Allocate(diag, vmar.borrow()));

  // These won't really be used, but they can be extracted.
  set_entry(linker.main_entry());
  set_stack_size(linker.main_stack_size());

  // Now it can be relocated for the foreign machine.
  EXPECT_TRUE(linker.Relocate(diag));

  // It can even be loaded.  But it can't be run.
  ASSERT_TRUE(linker.Load(diag));

  ExpectLog("");
}

template <class... Elf>
struct OnEachLayout {
  void operator()(auto&& f) const { (f.template operator()<Elf>(), ...); }
};

constexpr auto OnAllLayouts = elfldltl::AllFormats<OnEachLayout>{};

constexpr std::string_view kTestPrefix = "test/";
constexpr std::string_view kTestSuffix;

template <class Elf>
constexpr std::string_view kRemoteDecodedFileTestFile =
    Elf::template kFilename<kTestPrefix, elfldltl::ElfMachine::kNone, kTestSuffix>;

template <class Elf>
struct RemoteDecodedFileTest {
  using size_type = Elf::size_type;
  using Decoded = ld::RemoteDecodedModule<Elf>;
  using DecodedPtr = Decoded::Ptr;

  static constexpr size_type kPageSize = 0x1000;

  static zx::vmo TestData() {
    return elfldltl::testing::GetTestLibVmo(kRemoteDecodedFileTestFile<Elf>);
  }

  static void Test() {
    zx::vmo vmo;
    ASSERT_NO_FATAL_FAILURE(vmo = TestData());

    elfldltl::testing::ExpectOkDiagnostics diag;
    DecodedPtr decoded = Decoded::Create(diag, std::move(vmo), kPageSize);
    ASSERT_TRUE(decoded);

    // Any sort of ld::RemoteDecodedModule<...>::Ptr can be upcast to a generic
    // ld::RemoteDecodedFile::Ptr, but not implicitly.
    ld::RemoteDecodedFile::Ptr file = decoded->AsFile();

    // ld::RemoteDecodedFile::GetIf can downcast for the right format.
    OnAllLayouts([&file, &decoded]<class Layout>() {
      using LayoutPtr = ld::RemoteDecodedModule<Layout>::Ptr;
      LayoutPtr as_layout = file->GetIf<Layout>();
      if constexpr (std::is_same_v<Layout, Elf>) {
        EXPECT_TRUE(as_layout);
        EXPECT_EQ(as_layout, decoded);
      } else {
        EXPECT_FALSE(as_layout);

        // The GetIf overload taking a Diagnostics object will report why.
        if constexpr (Elf::kClass != Layout::kClass && Elf::kData != Layout::kData) {
          elfldltl::testing::ExpectedErrorList diag{
              elfldltl::testing::ExpectReport{"wrong ELF class (bit-width)"},
              elfldltl::testing::ExpectReport{"wrong byte order"},
          };
          as_layout = file->GetIf<Layout>(diag);
          EXPECT_FALSE(as_layout);
        } else if constexpr (Elf::kClass != Layout::kClass) {
          elfldltl::testing::ExpectedErrorList diag{
              elfldltl::testing::ExpectReport{"wrong ELF class (bit-width)"},
          };
          as_layout = file->GetIf<Layout>(diag);
          EXPECT_FALSE(as_layout);
        } else {
          elfldltl::testing::ExpectedErrorList diag{
              elfldltl::testing::ExpectReport{"wrong byte order"},
          };
          as_layout = file->GetIf<Layout>(diag);
          EXPECT_FALSE(as_layout);
        }
      }
    });

    // ld::RemoteDecodedFile::VisitAnyLayout should invoke the lambda with the
    // right type even though it was upcast before.
    file->VisitAnyLayout([decoded]<class SomePtr>(const SomePtr& ptr) {
      if constexpr (std::is_same_v<SomePtr, DecodedPtr>) {
        EXPECT_EQ(ptr, decoded);
      } else {
        ADD_FAILURE();
      }
    });

    // ld::RemoteDecodedFile::VisitAnyClass should invoke the lambda with the
    // right type even though it was upcast before.
    file->VisitAnyClass<Elf::kData>([decoded]<class SomePtr>(const SomePtr& ptr) {
      if constexpr (std::is_same_v<SomePtr, DecodedPtr>) {
        EXPECT_EQ(ptr, decoded);
      } else {
        ADD_FAILURE();
      }
    });
  }
};

TEST_F(LdRemoteTests, RemoteDecodedFile) {
  constexpr auto test = []<class Elf>() {
    ASSERT_NO_FATAL_FAILURE(RemoteDecodedFileTest<Elf>::Test());
  };
  OnAllLayouts(test);
}

}  // namespace
