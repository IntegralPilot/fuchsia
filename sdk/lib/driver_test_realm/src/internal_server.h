// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_TEST_REALM_SRC_INTERNAL_SERVER_H_
#define LIB_DRIVER_TEST_REALM_SRC_INTERNAL_SERVER_H_

#include <fidl/fuchsia.component.resolution/cpp/fidl.h>
#include <fidl/fuchsia.driver.test/cpp/wire.h>

namespace driver_test_realm {

class InternalServer final : public fidl::WireServer<fuchsia_driver_test::Internal> {
 public:
  InternalServer(fidl::ClientEnd<fuchsia_io::Directory> boot_dir,
                 fidl::ClientEnd<fuchsia_io::Directory> pkg_dir,
                 fidl::ClientEnd<fuchsia_io::Directory> test_pkg_dir,
                 std::optional<fuchsia_component_resolution::Context> context,
                 std::optional<std::vector<std::string>> boot_driver_components);

  void Serve(async_dispatcher_t* dispatcher,
             fidl::ServerEnd<fuchsia_driver_test::Internal> server_end);

  void GetTestPackage(GetTestPackageCompleter::Sync& completer) override;
  void GetTestResolutionContext(GetTestResolutionContextCompleter::Sync& completer) override;
  void GetBootDirectory(GetBootDirectoryCompleter::Sync& completer) override;
  void GetBootDriverOverrides(GetBootDriverOverridesCompleter::Sync& completer) override;

 private:
  fidl::ClientEnd<fuchsia_io::Directory> boot_dir_;
  fidl::ClientEnd<fuchsia_io::Directory> pkg_dir_;
  fidl::ClientEnd<fuchsia_io::Directory> test_pkg_dir_;
  std::optional<fuchsia_component_resolution::Context> context_;
  std::optional<std::vector<std::string>> boot_driver_components_;
  fidl::ServerBindingGroup<fuchsia_driver_test::Internal> bindings_;
};

}  // namespace driver_test_realm

#endif  // LIB_DRIVER_TEST_REALM_SRC_INTERNAL_SERVER_H_
