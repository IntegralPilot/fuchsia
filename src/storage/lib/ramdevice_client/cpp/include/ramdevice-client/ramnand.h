// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_RAMDEVICE_CLIENT_CPP_INCLUDE_RAMDEVICE_CLIENT_RAMNAND_H_
#define SRC_STORAGE_LIB_RAMDEVICE_CLIENT_CPP_INCLUDE_RAMDEVICE_CLIENT_RAMNAND_H_

#include <fidl/fuchsia.device/cpp/wire.h>
#include <fidl/fuchsia.hardware.nand/cpp/wire.h>
#include <inttypes.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <zircon/compiler.h>

#include <memory>
#include <optional>

#include <fbl/string.h>

namespace ramdevice_client {

// A client library for creating, configuring and manipulating ramnands.
// ```
// ASSERT_EQ(ZX_OK, device_watcher::RecursiveWaitForFile("/dev/sys/platform/ram-nand/nand-ctl",
//   zx::sec(60)).status_value());
// fuchsia_hardware_nand::wire::RamNandInfo ram_nand_config = {
//   .nand_info = {
//     .page_size = 4096,
//     .pages_per_block = 32,
//     .num_blocks = 64,
//     .ecc_bits = 8,
//     .oob_size = 16,
//     .nand_class = fuchsia_hardware_nand::wire::Class::FTL,
//   }
// }
// auto ram_nand = ramdevice_client::RamNand::Create(std::move(ram_nand_config));
// ASSERT_TRUE(ram_nand.is_ok());
// ```
class RamNand {
 public:
  // The default path to the system nand-ctl.
  static constexpr char kBasePath[] = "/dev/sys/platform/ram-nand/nand-ctl";

  // Creates a ram_nand under ram_nand_ctl running under the main devmgr.
  static zx::result<RamNand> Create(fuchsia_hardware_nand::wire::RamNandInfo config);

  // Creates a RamNand from an existing Controller channel.
  // This will connect to the RamNand FIDL protocol.
  static zx::result<RamNand> Create(fidl::ClientEnd<fuchsia_device::Controller> controller,
                                    std::optional<fbl::String> path = std::nullopt,
                                    std::optional<fbl::String> filename = std::nullopt);

  // Not copyable.
  RamNand(const RamNand&) = delete;
  RamNand& operator=(const RamNand&) = delete;

  // Movable.
  RamNand(RamNand&&) = default;
  RamNand& operator=(RamNand&&) = default;

  ~RamNand();

  // Don't unbind in destructor.
  void NoUnbind() { unbind = false; }

  const fidl::ClientEnd<fuchsia_device::Controller>& controller() const { return controller_; }

  // Return the path to the created ramnand device, or nullptr if this object did not create the
  // device itself.
  const char* path() const {
    if (path_) {
      return path_->c_str();
    }
    return nullptr;
  }

  // Return the path to the filename of ramnand device, or nullptr if this object did not create the
  // device itself.
  const char* filename() {
    if (filename_) {
      return filename_->c_str();
    }
    return nullptr;
  }

 private:
  RamNand(fidl::ClientEnd<fuchsia_device::Controller> controller,
          fidl::ClientEnd<fuchsia_hardware_nand::RamNand> ram_nand, std::optional<fbl::String> path,
          std::optional<fbl::String> filename);

  fidl::ClientEnd<fuchsia_device::Controller> controller_;
  fidl::ClientEnd<fuchsia_hardware_nand::RamNand> ram_nand_;
  bool unbind = true;

  // Only valid if not spawned in an isolated devmgr.
  std::optional<fbl::String> path_;

  // Only valid if not spawned in an isolated devmgr.
  std::optional<fbl::String> filename_;
};

}  // namespace ramdevice_client

#endif  // SRC_STORAGE_LIB_RAMDEVICE_CLIENT_CPP_INCLUDE_RAMDEVICE_CLIENT_RAMNAND_H_
