// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/paver/block-devices.h"

#include <fcntl.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/driver-integration-test/fixture.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fdio/namespace.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <unistd.h>

#include <string>

#include <fbl/ref_ptr.h>
#include <zxtest/zxtest.h>

#include "src/storage/lib/block_server/fake_server.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/service.h"
#include "src/storage/lib/vfs/cpp/synchronous_vfs.h"

namespace {

using driver_integration_test::IsolatedDevmgr;

/// Mocks the PartitionService exported by fshost.
class FakePartitionService {
 public:
  FakePartitionService(async_dispatcher_t* dispatcher,
                       std::vector<block_server::FakeServer> servers)
      : vfs_(dispatcher),
        root_dir_(fbl::MakeRefCounted<fs::PseudoDir>()),
        servers_(std::move(servers)) {
    auto service_dir = fbl::MakeRefCounted<fs::PseudoDir>();
    ASSERT_OK(root_dir_->AddEntry("fuchsia.storage.partitions.PartitionService", service_dir));
    for (unsigned i = 0; i < servers_.size(); ++i) {
      auto partition_dir = fbl::MakeRefCounted<fs::PseudoDir>();
      EXPECT_OK(service_dir->AddEntry("part-" + std::to_string(i), partition_dir));
      EXPECT_OK(partition_dir->AddEntry(
          "volume", fbl::MakeRefCounted<fs::Service>([this, i](zx::channel channel) {
            fidl::ServerEnd<fuchsia_storage_block::Block> request(std::move(channel));
            this->servers_[i].Serve(std::move(request));
            return ZX_OK;
          })));
    }

    // Bind to the local namespace at Path()
    auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ASSERT_EQ(vfs_.ServeDirectory(root_dir_, std::move(server)), ZX_OK);
    svc_root_ = std::move(client);
  }

  fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root() { return svc_root_.borrow(); }

 private:
  fs::SynchronousVfs vfs_;
  fbl::RefPtr<fs::PseudoDir> root_dir_;
  std::vector<block_server::FakeServer> servers_;
  fidl::ClientEnd<fuchsia_io::Directory> svc_root_;
};

TEST(BlockDevicesTests, TestPartitionsDir) {
  async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};
  ASSERT_OK(loop.StartThread("block-devices-tests-loop"));

  std::vector<block_server::FakeServer> servers;
  servers.emplace_back(block_server::PartitionInfo{
      .block_count = 512,
      .block_size = 512,
      .type_guid = {1, 2, 3, 4},
      .instance_guid = {5, 6, 7, 8},
      .name = "part1",
  });
  servers.emplace_back(block_server::PartitionInfo{
      .block_count = 512,
      .block_size = 512,
      .type_guid = {9, 10, 11, 12},
      .instance_guid = {13, 14, 15, 16},
      .name = "part2",
  });
  FakePartitionService partition_service(loop.dispatcher(), std::move(servers));

  // Although devfs is provided (so BlockDevices doesn't connect to /dev), the partitions dir is
  // preferentially used.
  IsolatedDevmgr::Args args;
  IsolatedDevmgr devmgr;
  ASSERT_OK(IsolatedDevmgr::Create(&args, &devmgr));

  zx::result devices =
      paver::BlockDevices::CreateFromPartitionService(partition_service.svc_root());
  ASSERT_OK(devices);

  {
    // Present partition
    zx::result connector = devices->OpenPartition([](const zx::channel& channel) {
      auto client = fidl::UnownedClientEnd<fuchsia_storage_block::Block>((channel.borrow()));
      auto result = fidl::WireCall(client)->GetInstanceGuid();
      if (!result.ok()) {
        return false;
      }
      const auto& response = result.value();
      if (response.status != ZX_OK) {
        return false;
      }
      const uint8_t kExpectedGuid[16] = {5, 6, 7, 8};
      return memcmp(response.guid->value.data_, &kExpectedGuid[0], 16) == 0;
    });
    ASSERT_OK(connector);

    zx::result partition = connector->Connect();
    ASSERT_OK(partition);

    // Make sure we got the right partition.
    fidl::WireResult name = fidl::WireCall(*partition)->GetName();
    ASSERT_OK(name);
    ASSERT_OK(name.value().status);
    ASSERT_STREQ(name.value().name.data(), "part1");
  }

  {
    // Absent partition
    zx::result connector = devices->OpenPartition([](const zx::channel& channel) {
      auto client = fidl::UnownedClientEnd<fuchsia_storage_block::Block>((channel.borrow()));
      auto result = fidl::WireCall(client)->GetInstanceGuid();
      if (!result.ok()) {
        return false;
      }
      const auto& response = result.value();
      if (response.status != ZX_OK) {
        return false;
      }
      const uint8_t kExpectedGuid[16] = {0xff, 0xff, 0xff, 0xff};
      return memcmp(response.guid->value.data_, &kExpectedGuid[0], 16) == 0;
    });
    ASSERT_EQ(connector.status_value(), ZX_ERR_NOT_FOUND);
  }
}

}  // namespace
