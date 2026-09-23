// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DRIVER_TEST_REALM_SRC_BOOT_ITEMS_H_
#define LIB_DRIVER_TEST_REALM_SRC_BOOT_ITEMS_H_

#include <fidl/fuchsia.boot/cpp/wire.h>

namespace driver_test_realm {
class BootItems final : public fidl::WireServer<fuchsia_boot::Items> {
 public:
  void SetBoardName(std::string_view board_name);
  void SetDeviceTree(zx::vmo devicetree);
  void SetVid(uint32_t vid);
  void SetPid(uint32_t pid);

  // If tunnel_to_incoming, we just connect the request to the one we have in our incoming
  // namespace. Otherwise this class serves as the server implementation.
  zx::result<> Serve(async_dispatcher_t* dispatcher,
                     fidl::ServerEnd<fuchsia_boot::Items> server_end, bool tunnel_to_incoming);

  void Get(GetRequestView request, GetCompleter::Sync& completer) override;
  void Get2(Get2RequestView request, Get2Completer::Sync& completer) override;
  void GetBootloaderFile(GetBootloaderFileRequestView request,
                         GetBootloaderFileCompleter::Sync& completer) override;

 private:
  std::string board_name_;
  zx::vmo devicetree_;
  std::optional<uint32_t> vid_;
  std::optional<uint32_t> pid_;
  fidl::ServerBindingGroup<fuchsia_boot::Items> bindings_;
};
}  // namespace driver_test_realm

#endif  // LIB_DRIVER_TEST_REALM_SRC_BOOT_ITEMS_H_
