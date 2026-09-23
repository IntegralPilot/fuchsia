// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ftl_test_observer.h"

#include <dirent.h>
#include <fidl/fuchsia.hardware.block.volume/cpp/wire.h>
#include <fidl/fuchsia.hardware.nand/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/device-watcher/cpp/device-watcher.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/namespace.h>
#include <sys/types.h>

#include <fbl/unique_fd.h>
#include <zxtest/zxtest.h>

FtlTestObserver::FtlTestObserver() = default;

void FtlTestObserver::OnProgramStart() {
  CreateDevice();
  if (zx_status_t status = WaitForBlockDevice(); status != ZX_OK) {
    printf("Unable to wait for block device. Error: %s\n", zx_status_get_string(status));
    return;
  }
  ok_ = true;
}

void FtlTestObserver::CreateDevice() {
  driver_integration_test::IsolatedDevmgr::Args args;
  if (zx_status_t status = driver_integration_test::IsolatedDevmgr::Create(&args, &devmgr_);
      status != ZX_OK) {
    printf("Unable to create devmgr: %s\n", zx_status_get_string(status));
    return;
  }
  std::unique_ptr<ramdevice_client_test::RamNandCtl> ctl;
  zx_status_t status =
      ramdevice_client_test::RamNandCtl::Create(devmgr_.devfs_root().duplicate(), &ctl);
  if (status != ZX_OK) {
    printf("Unable to create ram-nand-ctl\n");
    return;
  }
  ram_nand_ctl_ = std::move(ctl);

  if (zx_status_t status = ram_nand_ctl_->CreateRamNand(
          {
              .nand_info =
                  {

                      .page_size = 4096,
                      .pages_per_block = 64,
                      .num_blocks = 96,
                      .ecc_bits = 8,
                      .oob_size = 8,
                      .nand_class = fuchsia_hardware_nand::wire::Class::kFtl,
                  },
          },
          &ram_nand_);
      status != ZX_OK) {
    printf("Unable to create ram-nand: %s\n", zx_status_get_string(status));
  }
}

zx_status_t FtlTestObserver::WaitForBlockDevice() {
  if (!ram_nand_) {
    return ZX_ERR_BAD_STATE;
  }

  fidl::ClientEnd<fuchsia_io::Directory> exposed_dir = devmgr_.RealmExposedDir();
  fidl::UnownedClientEnd<fuchsia_io::Directory> unowned_exposed_dir(exposed_dir);

  zx::result dir =
      component::OpenDirectoryAt(unowned_exposed_dir, fuchsia_hardware_block_volume::Service::Name);
  if (dir.is_error()) {
    return dir.status_value();
  }

  component::SyncDirectoryWatcher watcher(unowned_exposed_dir,
                                          fuchsia_hardware_block_volume::Service::Name);
  auto watch_result = watcher.GetNextEntry(false, zx::deadline_after(zx::sec(30)));
  if (watch_result.is_error()) {
    return watch_result.status_value();
  }

  std::string path =
      std::string(fuchsia_hardware_block_volume::Service::Name) + "/" + watch_result.value();

  zx::result block_dir = component::OpenDirectoryAt(unowned_exposed_dir, path);
  if (block_dir.is_error()) {
    return block_dir.status_value();
  }

  fdio_ns_t* ns;
  if (zx_status_t status = fdio_ns_get_installed(&ns); status != ZX_OK) {
    return status;
  }

  if (zx_status_t status = fdio_ns_bind(ns, "/block_svc", block_dir->TakeChannel().release());
      status != ZX_OK) {
    return status;
  }

  return ZX_OK;
}
