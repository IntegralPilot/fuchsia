// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_STORAGE_LIB_RAMDEVICE_CLIENT_CPP_INCLUDE_RAMDEVICE_CLIENT_TEST_RAMNANDCTL_H_
#define SRC_STORAGE_LIB_RAMDEVICE_CLIENT_CPP_INCLUDE_RAMDEVICE_CLIENT_TEST_RAMNANDCTL_H_

#include <fbl/unique_fd.h>

#include "src/storage/lib/ramdevice_client/cpp/include/ramdevice-client/ramnand.h"

namespace ramdevice_client_test {

class RamNandCtl {
 public:
  // Creates a ram_nand_ctl device in the given devfs instance.
  static zx_status_t Create(fbl::unique_fd devfs_root, std::unique_ptr<RamNandCtl>* out);

  zx_status_t CreateRamNand(fuchsia_hardware_nand::wire::RamNandInfo config,
                            std::optional<ramdevice_client::RamNand>* out) const;

  ~RamNandCtl() = default;

  const fidl::ClientEnd<fuchsia_hardware_nand::RamNandCtl>& ctl() const { return ctl_; }

  const fbl::unique_fd& devfs_root() const { return devfs_root_; }

 private:
  RamNandCtl(fbl::unique_fd devfs_root, fidl::ClientEnd<fuchsia_hardware_nand::RamNandCtl> ctl)
      : devfs_root_(std::move(devfs_root)), ctl_(std::move(ctl)) {}

  fbl::unique_fd devfs_root_;
  const fidl::ClientEnd<fuchsia_hardware_nand::RamNandCtl> ctl_;
};

}  // namespace ramdevice_client_test

#endif  // SRC_STORAGE_LIB_RAMDEVICE_CLIENT_CPP_INCLUDE_RAMDEVICE_CLIENT_TEST_RAMNANDCTL_H_
