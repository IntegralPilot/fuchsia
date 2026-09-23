// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.io/cpp/markers.h>
#include <fidl/fuchsia.io/cpp/wire_types.h>
#include <fidl/fuchsia.process.lifecycle/cpp/markers.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/directory.h>
#include <lib/fdio/fd.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <unistd.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <cstddef>
#include <cstdint>
#include <memory>
#include <optional>
#include <utility>

#include <fbl/unique_fd.h>
#include <fuzzer/FuzzedDataProvider.h>

#include "src/storage/blobfs/common.h"
#include "src/storage/blobfs/component_runner.h"
#include "src/storage/blobfs/mkfs.h"
#include "src/storage/blobfs/mount.h"
#include "src/storage/blobfs/test/blob_utils.h"
#include "src/storage/blobfs/test/unit/local_decompressor_creator.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"

namespace blobfs {
namespace {

constexpr uint32_t kBlockDeviceSize = 128 * 1024 * 1024;
constexpr uint32_t kMaxBlobSize = 96 * 1024 * 1024;
constexpr uint32_t kBlockSize = 512;
constexpr uint32_t kNumBlocks = kBlockDeviceSize / kBlockSize;

fidl::ClientEnd<fuchsia_io::Directory> ServeOutgoingDirectory(ComponentRunner& runner) {
  auto root_endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
  auto status = runner.ServeRoot(std::move(root_endpoints.server),
                                 fidl::ServerEnd<fuchsia_process_lifecycle::Lifecycle>());
  ZX_ASSERT(status.is_ok());
  return std::move(root_endpoints.client);
}

fidl::ClientEnd<fuchsia_io::Directory> GetRootDirectory(
    fidl::ClientEnd<fuchsia_io::Directory>& outgoing) {
  auto [client, server] = fidl::Endpoints<fuchsia_io::Directory>::Create();
  auto status = fidl::WireCall(outgoing)->Open("root",
                                               fuchsia_io::wire::kPermReadable |
                                                   fuchsia_io::wire::kPermWritable |
                                                   fuchsia_io::wire::Flags::kProtocolDirectory,
                                               {}, server.TakeChannel());
  ZX_ASSERT(status.ok());
  return std::move(client);
}

class BlobfsInstance {
 public:
  explicit BlobfsInstance()
      : loop_(&kAsyncLoopConfigNoAttachToCurrentThread),
        runner_(loop_, ComponentOptions{.pager_threads = 1}),
        local_decompressor_creator_(LocalDecompressorCreator::Create().value()) {
    loop_.StartThread();
    auto device = std::make_unique<block_client::FakeBlockDevice>(kNumBlocks, kBlockSize);
    ZX_ASSERT(FormatFilesystem(device.get(), FilesystemOptions{}) == ZX_OK);
    auto outgoing = ServeOutgoingDirectory(runner_);
    ZX_ASSERT(runner_
                  .Configure(std::move(device),
                             MountOptions{
                                 .decompression_connector =
                                     &local_decompressor_creator_->GetDecompressorConnector(),
                                 .paging_threads = 1,
                             })
                  .is_ok());
    auto root = GetRootDirectory(outgoing);
    ZX_ASSERT(fdio_fd_create(root.TakeChannel().release(), root_fd_.reset_and_get_address()) ==
              ZX_OK);
    ZX_ASSERT(root_fd_.is_valid());

    auto svc_dir = component::OpenDirectoryAt(outgoing.borrow(), "svc");
    ZX_ASSERT(svc_dir.is_ok());
    blob_creator_ = std::make_unique<BlobCreatorWrapper>(BlobCreatorWrapper::Connect(*svc_dir));
    blob_reader_ = std::make_unique<BlobReaderWrapper>(BlobReaderWrapper::Connect(*svc_dir));
  }

  const fbl::unique_fd& root_fd() const { return root_fd_; }
  const BlobCreatorWrapper& blob_creator() const { return *blob_creator_; }
  const BlobReaderWrapper& blob_reader() const { return *blob_reader_; }

 private:
  async::Loop loop_;
  ComponentRunner runner_;
  std::unique_ptr<LocalDecompressorCreator> local_decompressor_creator_;
  fbl::unique_fd root_fd_;
  std::unique_ptr<BlobCreatorWrapper> blob_creator_;
  std::unique_ptr<BlobReaderWrapper> blob_reader_;
};

std::optional<bool> GetDeliveryBlobCompression(FuzzedDataProvider& provider) {
  enum class DeliveryBlobCompression : uint8_t {
    kAlwaysCompress,
    kNeverCompress,
    kMaybeCompress,
    kMaxValue = kMaybeCompress,
  };
  switch (provider.ConsumeEnum<DeliveryBlobCompression>()) {
    case DeliveryBlobCompression::kAlwaysCompress:
      return true;
    case DeliveryBlobCompression::kNeverCompress:
      return false;
    case DeliveryBlobCompression::kMaybeCompress:
      return std::nullopt;
  }
}

extern "C" int LLVMFuzzerTestOneInput(const uint8_t* data, size_t size) {
  static const BlobfsInstance* blobfs = new BlobfsInstance();
  FuzzedDataProvider provider(data, size);
  size_t blob_size = provider.ConsumeIntegralInRange<size_t>(1, kMaxBlobSize);
  TestBlobData blob = TestBlobData::CreateRealistic(blob_size);
  TestDeliveryBlob delivery_blob(blob, GetDeliveryBlobCompression(provider));

  ZX_ASSERT(blobfs->blob_creator().CreateAndWriteBlob(delivery_blob).is_ok());
  // The contents of a newly created blob is not cached and will be paged back in.
  ZX_ASSERT(blobfs->blob_reader().VerifyBlob(blob).is_ok());
  ZX_ASSERT(unlinkat(blobfs->root_fd().get(), blob.digest().ToString().c_str(), 0) == 0);
  return 0;
}

}  // namespace
}  // namespace blobfs
