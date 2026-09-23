// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/blobfs/test/integration/fdio_test.h"

#include <fidl/fuchsia.fs/cpp/wire.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/executor.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/diagnostics/reader/cpp/archive_reader.h>
#include <lib/diagnostics/reader/cpp/inspect.h>
#include <lib/fdio/cpp/caller.h>
#include <lib/fdio/fd.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/channel.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <condition_variable>
#include <cstdint>
#include <memory>
#include <mutex>
#include <string>
#include <utility>
#include <vector>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"
#include "src/storage/blobfs/common.h"
#include "src/storage/blobfs/component_runner.h"
#include "src/storage/blobfs/mkfs.h"
#include "src/storage/blobfs/mount.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/blobfs/test/unit/local_decompressor_creator.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "src/storage/lib/fs_management/cpp/admin.h"

namespace blobfs {

constexpr uint64_t kBlockSize = 512;
constexpr uint64_t kNumBlocks = 8192;

void FdioTest::SetUp() {
  loop_ = std::make_unique<async::Loop>(&kAsyncLoopConfigNoAttachToCurrentThread);

  auto device = std::make_unique<block_client::FakeBlockDevice>(kNumBlocks, kBlockSize);
  block_device_ = device.get();
  ASSERT_OK(FormatFilesystem(block_device_, FilesystemOptions{
                                                .oldest_minor_version = GetOldestMinorVersion(),
                                            }));

  auto [outgoing_dir_client, outgoing_dir_server] =
      fidl::Endpoints<fuchsia_io::Directory>::Create();

  // FdioTest already creates a local threaded version of blobfs instead of the component, so don't
  // require an external decompressor component either.
  decompressor_creator_ = LocalDecompressorCreator::Create().value();
  mount_options_.decompression_connector = &decompressor_creator_->GetDecompressorConnector();

  runner_ = std::make_unique<ComponentRunner>(
      *loop_, ComponentOptions{.pager_threads = mount_options_.paging_threads});
  ASSERT_OK(runner_->ServeRoot(std::move(outgoing_dir_server), {}));

  ASSERT_OK(runner_->Configure(std::move(device), mount_options_));

  ASSERT_OK(loop_->StartThread("blobfs test dispatcher"));

  auto root_client_or = fs_management::FsRootHandle(outgoing_dir_client);
  ASSERT_OK(root_client_or);

  // FDIO serving the root directory.
  ASSERT_OK(
      fdio_fd_create(root_client_or->TakeChannel().release(), root_fd_.reset_and_get_address()));
  ASSERT_TRUE(root_fd_.is_valid());
  ASSERT_OK(fdio_fd_create(outgoing_dir_client.TakeChannel().release(),
                           outgoing_dir_fd_.reset_and_get_address()));
  ASSERT_TRUE(outgoing_dir_fd_.is_valid());

  fdio_cpp::UnownedFdioCaller outgoing_dir(outgoing_dir_fd_);
  auto svc_dir = component::OpenDirectoryAt(outgoing_dir.directory(), "svc");
  ASSERT_OK(svc_dir);
  blob_reader_ = std::make_unique<BlobReaderWrapper>(BlobReaderWrapper::Connect(*svc_dir));
  blob_creator_ = std::make_unique<BlobCreatorWrapper>(BlobCreatorWrapper::Connect(*svc_dir));
}

void FdioTest::TearDown() {
  fdio_cpp::UnownedFdioCaller outgoing_dir(outgoing_dir_fd_);
  auto svc_client = component::OpenDirectoryAt(outgoing_dir.directory(), "svc");
  ASSERT_OK(svc_client);
  auto admin_client = component::ConnectAt<fuchsia_fs::Admin>(*svc_client);
  ASSERT_OK(admin_client);
  ASSERT_OK(fidl::WireCall(*admin_client)->Shutdown().status());
}

zx_handle_t FdioTest::outgoing_dir() {
  zx::channel outgoing_dir;
  fdio_fd_clone(outgoing_dir_fd_.get(), outgoing_dir.reset_and_get_address());
  return outgoing_dir.release();
}

void FdioTest::TakeSnapshot(inspect::Hierarchy* output) {
  async::Loop loop = async::Loop(&kAsyncLoopConfigNoAttachToCurrentThread);
  loop.StartThread("metric-collection-thread");
  async::Executor executor(loop.dispatcher());

  async_dispatcher_t* dispatcher = executor.dispatcher();

  std::condition_variable cv;
  std::mutex m;
  bool done = false;
  {
    constexpr char kTestComponentMoniker[2] = ".";
    diagnostics::reader::ArchiveReader reader(dispatcher, {});
    executor.schedule_task(reader.SnapshotInspectUntilPresent({kTestComponentMoniker})
                               .and_then([&](std::vector<diagnostics::reader::InspectData>& data) {
                                 {
                                   std::unique_lock<std::mutex> lock(m);
                                   diagnostics::reader::InspectData* inspect_data = nullptr;
                                   for (auto& i : data) {
                                     if (i.moniker() == kTestComponentMoniker) {
                                       // Check for duplicates.
                                       ASSERT_EQ(inspect_data, nullptr)
                                           << "Duplicate test entry found.";
                                       inspect_data = &i;
                                     }
                                   }
                                   ASSERT_NE(inspect_data, nullptr)
                                       << "Failed to find test moniker";
                                   if (!inspect_data->payload().has_value()) {
                                     FX_LOGS(INFO) << "inspect_data had nullopt payload";
                                   }
                                   if (inspect_data->payload().has_value() &&
                                       inspect_data->payload().value() == nullptr) {
                                     FX_LOGS(INFO) << "inspect_data had nullptr for payload";
                                   }
                                   if (inspect_data->metadata().errors.has_value()) {
                                     for (const auto& e : inspect_data->metadata().errors.value()) {
                                       FX_LOGS(INFO) << e.message;
                                     }
                                   }

                                   ASSERT_TRUE(inspect_data->payload().has_value());
                                   *output = inspect_data->TakePayload();
                                   done = true;
                                 }
                                 cv.notify_all();
                               }));

    std::unique_lock<std::mutex> lock(m);
    cv.wait(lock, [&] { return done; });
  }
  loop.Quit();
  loop.JoinThreads();
}

void FdioTest::GetUintMetricFromHierarchy(const inspect::Hierarchy& hierarchy,
                                          const std::vector<std::string>& path,
                                          const std::string& property, uint64_t* value) {
  ASSERT_NE(value, nullptr);
  const inspect::Hierarchy* direct_parent = hierarchy.GetByPath(path);
  ASSERT_NE(direct_parent, nullptr);

  const inspect::IntPropertyValue* property_node =
      direct_parent->node().get_property<inspect::IntPropertyValue>(property);
  ASSERT_NE(property_node, nullptr);

  *value = static_cast<uint64_t>(property_node->value());
}

void FdioTest::GetUintMetric(const std::vector<std::string>& path, const std::string& property,
                             uint64_t* value) {
  inspect::Hierarchy hierarchy;
  TakeSnapshot(&hierarchy);
  GetUintMetricFromHierarchy(hierarchy, path, property, value);
}

}  // namespace blobfs
