// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/download_manager.h"

#include <random>

#include <gtest/gtest.h>

#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/zxdb/client/download_observer.h"
#include "src/developer/debug/zxdb/client/mock_symbol_server.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/process_impl.h"
#include "src/developer/debug/zxdb/client/remote_api_test.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/common/host_util.h"
#include "src/developer/debug/zxdb/common/scoped_test_env.h"
#include "src/developer/debug/zxdb/symbols/loaded_module_symbols.h"
#include "src/developer/debug/zxdb/symbols/mock_module_symbols.h"
#include "src/developer/debug/zxdb/symbols/module_symbols.h"
#include "src/developer/debug/zxdb/symbols/process_symbols.h"
#include "src/lib/fxl/memory/ref_ptr.h"

namespace zxdb {

constexpr char kPublicBuildId1[] = "abc123";
constexpr char kPublicBuildId2[] = "deadbeef";
constexpr char kPrivateBuildId1[] = "fed098";
constexpr char kPrivateBuildId2[] = "fee567";

class DownloadManagerTest : public DownloadObserver, public RemoteAPITest {
 public:
  DownloadManagerTest() {
    static auto fake_home =
        std::filesystem::path(GetSelfPath()).parent_path() / "test_data" / "zxdb" / "fake_home";

    test_env_.Set("HOME", fake_home.string());
    test_env_.Unset("XDG_CACHE_HOME");
    test_env_.Unset("GCE_METADATA_HOST");
  }
  ~DownloadManagerTest() = default;

  size_t succeeded() const { return downloads_succeeded_; }

  size_t failed() const { return downloads_failed_; }

  DownloadManager* DownloadManager() { return session().system().GetDownloadManager(); }

  // Notably, this is different from |RemoteAPITest::InjectModule| by explicitly
  // *not* injecting the modules into the system symbols cache. Instead this
  // class's |OnFetch| method will insert the newly "downloaded" modules into
  // SystemSymbols.
  void InjectModulesToProcess(
      const std::map<std::string, std::pair<uint64_t, fxl::RefPtr<ModuleSymbols>>>& mods) {
    std::vector<debug_ipc::Module> modules;

    for (const auto& it : mods) {
      debug_ipc::Module module;
      module.build_id = it.first;
      module.base = it.second.first;
      module.name = it.first + "_mock_module.so";
      module_map_[it.first] = it.second.second;
      modules.push_back(module);
    }

    // Need to convert to an actual ProcessImpl.
    ProcessImpl* process_impl = session().system().ProcessImplFromKoid(process_->GetKoid());
    FX_CHECK(process_impl);
    process_impl->OnModules(modules);
  }

  void MakeServersUnreachable() {
    mock_public_server_->ForceUnreachable();
    mock_private_server_->ForceUnreachable();
  }

 private:
  void SetUp() override {
    RemoteAPITest::SetUp();

    session().AddDownloadObserver(this);

    process_ = InjectProcess(0x1234);

    // Create the non-authenticated server. This will report as ready immediately to
    // DownloadManager.
    auto server =
        std::make_unique<MockSymbolServer>(&session(), "https://debuginfod.fake.server.net");
    mock_public_server_ = server.get();

    mock_public_server_->on_fetch = [this](const std::string& build_id,
                                           DebugSymbolFileType file_type,
                                           SymbolServer::FetchCallback cb) mutable {
      std::set<std::string> build_ids = {kPublicBuildId1, kPublicBuildId2};
      OnFetch(build_ids, build_id, file_type, std::move(cb));
    };

    mock_public_server_->InitForTest();
    session().system().InjectSymbolServerForTesting(std::move(server));

    // Create an authenticated server, which will become ready shortly.
    server =
        std::make_unique<MockSymbolServer>(&session(), "https://debuginfod.private.server.com");
    mock_private_server_ = server.get();

    mock_private_server_->on_fetch = [this](const std::string& build_id,
                                            DebugSymbolFileType file_type,
                                            SymbolServer::FetchCallback cb) mutable {
      std::set<std::string> build_ids = {kPrivateBuildId1, kPrivateBuildId2};
      OnFetch(build_ids, build_id, file_type, std::move(cb));
    };

    // Set the authentication callback to always successfully authenticate by posting the callback
    // back to the message loop. This is a good enough approximation for simulating an actual
    // authentication exchange which typically happens pretty quickly, but is not immediate.
    mock_private_server_->on_do_authenticate = [this](const std::map<std::string, std::string>&,
                                                      fit::callback<void(const Err&)>) mutable {
      loop().PostTask(FROM_HERE, [this]() mutable { mock_private_server_->ForceReady(); });
    };

    mock_private_server_->InitForTest();
    session().system().InjectSymbolServerForTesting(std::move(server));
  }

  void TearDown() override {
    session().RemoveDownloadObserver(this);

    RemoteAPITest::TearDown();
  }

  void OnFetch(const std::set<std::string>& valid_build_ids, const std::string& build_id,
               DebugSymbolFileType file_type, SymbolServer::FetchCallback cb) {
    if (valid_build_ids.find(build_id) != valid_build_ids.end()) {
      // Insert the mock module directly into SystemSymbol's cache to avoid trying to actually load
      // a real binary.
      session().system().GetSymbols()->InjectModuleForTesting(build_id,
                                                              module_map_[build_id].get());

      loop().PostTask(FROM_HERE, [cb = std::move(cb)]() mutable { cb(Err(), "some/path"); });
    } else {
      loop().PostTask(FROM_HERE,
                      [cb = std::move(cb)]() mutable { cb(Err(ErrType::kNotFound), ""); });
    }
  }

  // DownloadObserver implementation.
  void OnDownloadsStopped(size_t num_succeeded, size_t num_failed) override {
    downloads_succeeded_ = num_succeeded;
    downloads_failed_ = num_failed;
    loop().QuitNow();
  }

  size_t downloads_succeeded_ = 0;
  size_t downloads_failed_ = 0;

  ScopedTestEnv test_env_;

  // This server will become ready immediately.
  MockSymbolServer* mock_public_server_;

  // This server will become ready after a small delay.
  MockSymbolServer* mock_private_server_;

  // Process to inject symbols to.
  Process* process_;

  // Keep an owning handle to the module symbols we inject into processes so they aren't deleted in
  // the middle of the test. Normally this handle would be held by LoadedModuleSymbols with a real
  // ModuleSymbolsImpl backing it with all the symbols.
  std::map<std::string, fxl::RefPtr<ModuleSymbols>> module_map_;
};

// Download a buildid from a non-authenticated server.
TEST_F(DownloadManagerTest, PublicDownload) {
  auto sym1 = fxl::MakeRefCounted<MockModuleSymbols>("abc123_mock.so");

  std::map<std::string, std::pair<uint64_t, fxl::RefPtr<ModuleSymbols>>> module_map = {
      {kPublicBuildId1, {0x1000, std::move(sym1)}},
  };

  // Inject the module to the process, but not the system.
  InjectModulesToProcess(module_map);
  EXPECT_TRUE(DownloadManager()->HasDownload(kPublicBuildId1));

  loop().Run();

  EXPECT_EQ(succeeded(), 1ull);
  EXPECT_EQ(failed(), 0ull);
  EXPECT_FALSE(DownloadManager()->HasDownload(kPublicBuildId1));

  // Now the symbols should be available synchronously.
  fxl::RefPtr<ModuleSymbols> mod;
  session().system().GetSymbols()->GetModule("", kPublicBuildId1, false, &mod);

  // This should be the module we asked for.
  ASSERT_NE(mod.get(), nullptr);
  EXPECT_STREQ(mod->GetStatus().name.c_str(), "abc123_mock.so");
  EXPECT_TRUE(mod->GetStatus().symbols_loaded);
}

// Download a buildid from an authenticated server. This should not report any failures even though
// the non-authenticated server is tried first where it will not be found.
TEST_F(DownloadManagerTest, AuthenticatedDownload) {
  auto sym1 = fxl::MakeRefCounted<MockModuleSymbols>("fed098_mock.so");

  std::map<std::string, std::pair<uint64_t, fxl::RefPtr<ModuleSymbols>>> module_map = {
      {kPrivateBuildId1, {0x4000, std::move(sym1)}},
  };

  InjectModulesToProcess(module_map);
  EXPECT_TRUE(DownloadManager()->HasDownload(kPrivateBuildId1));

  loop().Run();

  EXPECT_EQ(succeeded(), 1ull);
  EXPECT_EQ(failed(), 0ull);
  EXPECT_FALSE(DownloadManager()->HasDownload(kPrivateBuildId1));

  fxl::RefPtr<ModuleSymbols> mod;
  session().system().GetSymbols()->GetModule("", kPrivateBuildId1, false, &mod);

  ASSERT_NE(mod.get(), nullptr);
  EXPECT_STREQ(mod->GetStatus().name.c_str(), "fed098_mock.so");
  EXPECT_TRUE(mod->GetStatus().symbols_loaded);
}

// Attempt to download a buildid that is not available on any registered servers.
TEST_F(DownloadManagerTest, BuildIDNotFound) {
  auto sym1 = fxl::MakeRefCounted<MockModuleSymbols>("notreal");

  std::map<std::string, std::pair<uint64_t, fxl::RefPtr<ModuleSymbols>>> module_map = {
      {"badbuildid", {0x0, std::move(sym1)}},
  };

  InjectModulesToProcess(module_map);
  EXPECT_TRUE(DownloadManager()->HasDownload("badbuildid"));

  loop().Run();

  EXPECT_EQ(succeeded(), 0ull);
  EXPECT_EQ(failed(), 1ull);
  EXPECT_FALSE(DownloadManager()->HasDownload("badbuildid"));

  fxl::RefPtr<ModuleSymbols> mod;
  session().system().GetSymbols()->GetModule("", "badbuildid", false, &mod);
  EXPECT_EQ(mod.get(), nullptr);
}

TEST_F(DownloadManagerTest, MultipleDownloads) {
  auto sym1 = fxl::MakeRefCounted<MockModuleSymbols>("abc123_mock.so");
  auto sym2 = fxl::MakeRefCounted<MockModuleSymbols>("deadbeef_mock.so");
  auto sym3 = fxl::MakeRefCounted<MockModuleSymbols>("fed098_mock.so");
  auto sym4 = fxl::MakeRefCounted<MockModuleSymbols>("fee567_mock.so");

  std::map<std::string, std::pair<uint64_t, fxl::RefPtr<ModuleSymbols>>> module_map = {
      {kPublicBuildId1, {0x1000, sym1}},
      {kPublicBuildId2, {0x2000, sym2}},
      {kPrivateBuildId1, {0x3000, sym3}},
      {kPrivateBuildId2, {0x4000, sym4}},
  };

  // These should kick off downloads when we try to save the module without
  // inserting it into the SystemSymbols.
  InjectModulesToProcess(module_map);

  // The downloads should all be present from injecting them into the process.
  for (const auto& buildid :
       {kPrivateBuildId1, kPrivateBuildId2, kPublicBuildId2, kPublicBuildId1}) {
    EXPECT_TRUE(DownloadManager()->HasDownload(buildid));
  }

  loop().Run();

  EXPECT_EQ(succeeded(), 4ull);
  EXPECT_EQ(failed(), 0ull);

  for (const auto& buildid :
       {kPrivateBuildId1, kPrivateBuildId2, kPublicBuildId2, kPublicBuildId1}) {
    EXPECT_FALSE(DownloadManager()->HasDownload(buildid));
  }

  fxl::RefPtr<ModuleSymbols> private_mod1;
  fxl::RefPtr<ModuleSymbols> private_mod2;
  fxl::RefPtr<ModuleSymbols> public_mod2;
  fxl::RefPtr<ModuleSymbols> public_mod1;

  // Now all the symbols should be available syncrhonously.
  session().system().GetSymbols()->GetModule("", kPrivateBuildId1, false, &private_mod1);
  session().system().GetSymbols()->GetModule("", kPrivateBuildId2, false, &private_mod2);
  session().system().GetSymbols()->GetModule("", kPublicBuildId1, false, &public_mod1);
  session().system().GetSymbols()->GetModule("", kPublicBuildId2, false, &public_mod2);

  for (const auto& mod : {private_mod1, private_mod2, public_mod2, public_mod1}) {
    ASSERT_NE(mod.get(), nullptr);
  }

  EXPECT_STREQ(public_mod1->GetStatus().name.c_str(), "abc123_mock.so");
  EXPECT_STREQ(public_mod2->GetStatus().name.c_str(), "deadbeef_mock.so");
  EXPECT_STREQ(private_mod1->GetStatus().name.c_str(), "fed098_mock.so");
  EXPECT_STREQ(private_mod2->GetStatus().name.c_str(), "fee567_mock.so");
  EXPECT_TRUE(public_mod1->GetStatus().symbols_loaded);
  EXPECT_TRUE(public_mod2->GetStatus().symbols_loaded);
  EXPECT_TRUE(private_mod1->GetStatus().symbols_loaded);
  EXPECT_TRUE(private_mod2->GetStatus().symbols_loaded);
}

TEST_F(DownloadManagerTest, SomeSucceedSomeFail) {
  auto sym1 = fxl::MakeRefCounted<MockModuleSymbols>("deadbeef_mock.so");
  auto sym2 = fxl::MakeRefCounted<MockModuleSymbols>("notfound.so");

  std::map<std::string, std::pair<uint64_t, fxl::RefPtr<ModuleSymbols>>> module_map = {
      {kPublicBuildId2, {0x1000, sym1}},
      {"notfound", {0x2000, sym2}},
  };

  InjectModulesToProcess(module_map);
  EXPECT_TRUE(DownloadManager()->HasDownload(kPublicBuildId2));
  EXPECT_TRUE(DownloadManager()->HasDownload("notfound"));

  loop().Run();

  EXPECT_EQ(succeeded(), 1ull);
  EXPECT_EQ(failed(), 1ull);
  EXPECT_FALSE(DownloadManager()->HasDownload(kPublicBuildId2));
  EXPECT_FALSE(DownloadManager()->HasDownload("notfound"));

  // Should have loaded the one that succeeded.
  fxl::RefPtr<ModuleSymbols> mod;
  session().system().GetSymbols()->GetModule("", kPublicBuildId2, false, &mod);
  ASSERT_NE(mod.get(), nullptr);
  EXPECT_STREQ(mod->GetStatus().name.c_str(), "deadbeef_mock.so");
  EXPECT_TRUE(mod->GetStatus().symbols_loaded);
}

// Download a buildid from a non-authenticated server.
TEST_F(DownloadManagerTest, DownloadUnreachable) {
  auto sym1 = fxl::MakeRefCounted<MockModuleSymbols>("abc123_mock.so");

  // Mark all the symbol servers as unreachable, preventing them from being used
  // for downloads.
  MakeServersUnreachable();

  std::map<std::string, std::pair<uint64_t, fxl::RefPtr<ModuleSymbols>>> module_map = {
      {kPublicBuildId1, {0x1000, std::move(sym1)}},
  };

  // Inject the module to the process, but not the system.
  InjectModulesToProcess(module_map);

  // No downloads will be started, because all symbol servers are unreachable
  EXPECT_FALSE(DownloadManager()->HasDownload(kPublicBuildId1));

  loop().RunUntilNoTasks();  // loop().Run() will hang because OnDownloadsStopped() is never called

  EXPECT_FALSE(DownloadManager()->HasDownload(kPublicBuildId1));

  fxl::RefPtr<ModuleSymbols> mod;
  session().system().GetSymbols()->GetModule("", "abc123_mock.so", false, &mod);
  EXPECT_EQ(mod.get(), nullptr);
}

}  // namespace zxdb
