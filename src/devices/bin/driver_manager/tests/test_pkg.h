// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_TEST_PKG_H_
#define SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_TEST_PKG_H_

#include <fidl/fuchsia.io/cpp/test_base.h>
#include <fuchsia/io/cpp/fidl_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fdio/directory.h>
#include <lib/fidl/cpp/binding.h>
#include <zircon/errors.h>

#include <gtest/gtest.h>

namespace test_utils {

class TestFile : public fuchsia::io::testing::File_TestBase {
 public:
  explicit TestFile(std::string_view path) : path_(std::move(path)) {}

 private:
  void GetBackingMemory(fuchsia::io::VmoFlags flags, GetBackingMemoryCallback callback) override {
    EXPECT_EQ(fuchsia::io::VmoFlags::READ | fuchsia::io::VmoFlags::EXECUTE |
                  fuchsia::io::VmoFlags::PRIVATE_CLONE,
              flags);
    auto [client, server] = fidl::Endpoints<fuchsia_io::File>::Create();
    EXPECT_EQ(ZX_OK, fdio_open3(path_.data(),
                                uint64_t{fuchsia::io::PERM_READABLE | fuchsia::io::PERM_EXECUTABLE},
                                server.TakeChannel().release()));

    fidl::WireSyncClient<fuchsia_io::File> file(std::move(client));
    fidl::WireResult result = file->GetBackingMemory(fuchsia_io::wire::VmoFlags(uint32_t(flags)));
    EXPECT_TRUE(result.ok()) << result.FormatDescription();
    auto* res = result.Unwrap();
    if (res->is_error()) {
      callback(fuchsia::io::File_GetBackingMemory_Result::WithErr(std::move(res->error_value())));
      return;
    }
    callback(fuchsia::io::File_GetBackingMemory_Result::WithResponse(
        fuchsia::io::File_GetBackingMemory_Response(std::move(res->value()->vmo))));
  }

  void NotImplemented_(const std::string& name) override {
    printf("Not implemented: File::%s\n", name.data());
  }

  std::string path_;
};

class TestDirectory : public fuchsia::io::testing::Directory_TestBase {
 public:
  using OpenHandler =
      fit::function<void(fuchsia::io::Flags flags, const std::string& path, zx::channel object)>;

  void SetOpenHandler(OpenHandler open_handler) { open_handler_ = std::move(open_handler); }

  void Open(std::string path, fuchsia::io::Flags flags, fuchsia::io::Options mode,
            zx::channel object) override {
    open_handler_(flags, path, std::move(object));
  }

  void NotImplemented_(const std::string& name) override {
    printf("Not implemented: Directory::%s\n", name.data());
  }

  OpenHandler open_handler_;
};

// Implementation of a /pkg directory that can be passed as a component namespace entry
// for the started driver host or driver component.
class TestPkg {
 public:
  struct ModuleConfig {
    // Where the module is located in the test's package. e.g.
    // /pkg/bin/driver_host2.
    std::string_view test_pkg_path;
    // The path that will be requested to the /pkg open
    // handler for the module. e.g. bin/driver_host2.
    std::string_view open_path;
  };

  struct Config {
    ModuleConfig main_module;
    // The names of the libraries that are needed by the main module.
    // This list will be used to construct the test files that the driver host runner
    // or driver runner expects to be present in the "/pkg/libs" dir that will be passed
    // to the dynamic linker. No additional validation is done on the strings in |expected_libs|.
    std::vector<std::string_view> expected_libs;
    // Modules that need to be load with the main module.
    // For example, the DFv1 driver to be loaded with the compatibility shim.
    // Additional libs are not supported.
    std::vector<ModuleConfig> additional_modules;
  };

  // |server| is the channel that will be served by |TestPkg|.
  //
  // |module_test_pkg_path| is where the module is located in the test's package. e.g.
  // /pkg/bin/driver_host2.
  //
  // |module_open_path| is the path that will be requested to the /pkg open
  // handler for the module. e.g. bin/driver_host2.
  //
  // |expected_libs| holds that names of the libraries that are needed by the module.
  // This list will be used to construct the test files that the driver host runner
  // or driver runner expects to be present in the "/pkg/libs" dir that will be passed
  // to the dynamic linker. No additional validation is done on the strings in |expected_libs|.
  TestPkg(fidl::ServerEnd<fuchsia_io::Directory> server, std::string_view module_test_pkg_path,
          std::string_view module_open_path, const std::vector<std::string_view> expected_libs,
          const std::vector<ModuleConfig> additional_modules_configs = std::vector<ModuleConfig>());

  TestPkg(fidl::ServerEnd<fuchsia_io::Directory> server, Config config)
      : TestPkg(std::move(server), config.main_module.test_pkg_path, config.main_module.open_path,
                config.expected_libs, config.additional_modules) {}

  ~TestPkg() {
    loop_.Quit();
    loop_.JoinThreads();
  }

 private:
  static constexpr std::string_view kLibPathPrefix = "/pkg/lib/";

  struct Module {
    TestFile file;
    std::unique_ptr<fidl::Binding<fuchsia::io::File>> binding;
    // The path that will be requested to the /pkg open
    // handler for the module. e.g. bin/driver_host2.
    std::string open_path;
  };

  async::Loop loop_{&kAsyncLoopConfigNoAttachToCurrentThread};

  TestDirectory pkg_dir_;
  fidl::Binding<fuchsia::io::Directory> pkg_binding_{&pkg_dir_};

  TestDirectory lib_dir_;
  fidl::Binding<fuchsia::io::Directory> lib_dir_binding_{&lib_dir_};
  std::map<std::string, TestFile> libname_to_file_;
  std::vector<std::unique_ptr<fidl::Binding<fuchsia::io::File>>> lib_file_bindings_;

  TestFile module_;
  fidl::Binding<fuchsia::io::File> module_binding_{&module_};

  // The fake test files and bindings for the additional modules.
  std::vector<Module> additional_modules_;
};

}  // namespace test_utils

#endif  // SRC_DEVICES_BIN_DRIVER_MANAGER_TESTS_TEST_PKG_H_
