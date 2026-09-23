// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.fshost/cpp/fidl.h>
#include <fidl/fuchsia.fshost/cpp/test_base.h>
#include <fidl/fuchsia.hardware.power.statecontrol/cpp/wire.h>
#include <fidl/fuchsia.hardware.power.statecontrol/cpp/wire_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fastboot/fastboot.h>
#include <lib/fastboot/test/test-transport.h>
#include <lib/zx/result.h>
#include <zircon/status.h>

#include <algorithm>
#include <future>
#include <vector>

#include <fbl/ref_ptr.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/firmware/lib/fastboot/sparse_format.h"
#include "src/firmware/lib/fastboot/test/fastboot-test.h"
#include "src/storage/lib/vfs/cpp/managed_vfs.h"
#include "src/storage/lib/vfs/cpp/pseudo_dir.h"
#include "src/storage/lib/vfs/cpp/service.h"
#include "src/storage/lib/vfs/cpp/vmo_file.h"
#include "src/storage/testing/fake-paver.h"

namespace fastboot {

namespace {

namespace fio = fuchsia_io;

constexpr size_t kBlockSize = 4096;
/// Maximum size of a given sparse payload.
constexpr size_t kMaxDownloadSize = 256ull * 1024ull;
/// The maximum size of an unsparsed payload, *including* don't-care chunks, that the tests allow.
constexpr size_t kMaxBlobImageSize = 40ull * 1024ull * 1024ull;
/// The maximum number of chunks that we can flash in the tests below.
constexpr size_t kMaxNumChunks = kMaxBlobImageSize / kBlockSize;

/// Extended version of the C++ VFS VmoFile type that allows for Resize/Truncate and Sync operations
/// to succeed. We also use this to test out-of-space conditions when we might flash an image too
/// large for the device.
class VmoFile final : public fs::VmoFile {
 public:
  explicit VmoFile(zx::vmo vmo)
      : fs::VmoFile(std::move(vmo), kMaxBlobImageSize, /*writable*/ true) {}

  /// Returns a promise that the next Sync operation will resolve with the given value.
  std::promise<zx_status_t> GetSyncPromise() {
    std::promise<zx_status_t> promise;
    sync_future_ = promise.get_future();
    return promise;
  }

 private:
  zx_status_t Truncate(size_t len) final {
    if (len > kMaxBlobImageSize) {
      return ZX_ERR_NO_SPACE;
    }
    return ZX_OK;
  }

  void Sync(SyncCallback closure) final {
    closure(sync_future_ ? sync_future_->get() : ZX_OK);
    sync_future_.reset();
  }

  std::optional<std::future<zx_status_t>> sync_future_;

 protected:
  friend fbl::internal::MakeRefCountedHelper<VmoFile>;
  friend fbl::RefPtr<VmoFile>;
};

class FastbootFlashBlobTest : public FastbootDownloadTest {
  class MockFshostRecovery : public fidl::testing::TestBase<fuchsia_fshost::Recovery> {
    friend class FastbootFlashBlobTest;

   public:
    explicit MockFshostRecovery(zx::vmo vmo, fs::ManagedVfs* vfs)
        : vfs_(vfs), image_file_{fbl::MakeRefCounted<VmoFile>(std::move(vmo))} {}

    void GetBlobImageHandle(GetBlobImageHandleCompleter::Sync& completer) final {
      const zx_status_t error_override = error_;
      if (error_override != ZX_OK) {
        completer.Reply(zx::error(error_override));
        return;
      }
      if (is_unformatted_) {
        completer.Reply(
            zx::ok(fuchsia_fshost::RecoveryGetBlobImageHandleResponse::WithUnformatted({})));
        return;
      }
      zx::eventpair ep0, ep1;
      ZX_ASSERT(zx::eventpair::create(0, &ep0, &ep1) == ZX_OK);
      auto [client, server] = fidl::Endpoints<fio::File>::Create();
      ZX_ASSERT(vfs_->Serve(image_file_, server.TakeChannel(),
                            fio::kPermReadable | fio::kPermWritable) == ZX_OK);
      fuchsia_fshost::MountedSystemContainer container{std::move(client), std::move(ep1)};
      completer.Reply(
          zx::ok(fuchsia_fshost::RecoveryGetBlobImageHandleResponse::WithMountedSystemContainer(
              std::move(container))));
    }

    void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) final {
      FAIL() << "Unexpected call to fuchsia.fshost/Recovery: " << name;
      completer.Close(ZX_ERR_NOT_SUPPORTED);
    }

   private:
    fs::ManagedVfs* vfs_;
    fbl::RefPtr<VmoFile> image_file_;
    std::atomic<zx_status_t> error_ = ZX_OK;
    std::atomic<bool> is_unformatted_ = false;
  };

  class MockFshostAdmin : public fidl::testing::TestBase<fuchsia_fshost::Admin> {
    friend class FastbootFlashBlobTest;

   public:
    void ShredDataVolume(ShredDataVolumeCompleter::Sync& completer) final {
      data_volume_shredded_ = true;
      completer.Reply(zx::ok());
    }

    void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) final {
      FAIL() << "Unexpected call to fuchsia.fshost/Admin: " << name;
      completer.Close(ZX_ERR_NOT_SUPPORTED);
    }

   private:
    std::atomic<bool> data_volume_shredded_ = false;
  };

  class MockPowerStateControl
      : public fidl::testing::WireTestBase<fuchsia_hardware_power_statecontrol::Admin> {
   public:
    void Shutdown(ShutdownRequestView request, ShutdownCompleter::Sync& completer) override {
      shutdown_called_ = true;
      completer.ReplySuccess();
    }
    void NotImplemented_(const std::string& name, fidl::CompleterBase& completer) override {
      FAIL() << "Unexpected call to fuchsia.hardware.power.statecontrol/Admin: " << name;
      completer.Close(ZX_ERR_NOT_SUPPORTED);
    }
    bool shutdown_called() const { return shutdown_called_; }

   private:
    std::atomic<bool> shutdown_called_ = false;
  };

 protected:
  FastbootFlashBlobTest() = default;

  void SetUp() override {
    zx::vmo vmo;
    ASSERT_EQ(zx::vmo::create(kMaxBlobImageSize, 0, &vmo), ZX_OK);
    recovery_server_ = std::make_unique<MockFshostRecovery>(std::move(vmo), &vfs_);
    admin_server_ = std::make_unique<MockFshostAdmin>();

    auto svc_dir = fbl::MakeRefCounted<fs::PseudoDir>();
    svc_dir->AddEntry(
        fidl::DiscoverableProtocolName<fuchsia_fshost::Recovery>,
        fbl::MakeRefCounted<fs::Service>([this](fidl::ServerEnd<fuchsia_fshost::Recovery> request) {
          fidl::BindServer(loop_.dispatcher(), std::move(request), recovery_server_.get());
          return ZX_OK;
        }));

    svc_dir->AddEntry(
        fidl::DiscoverableProtocolName<fuchsia_fshost::Admin>,
        fbl::MakeRefCounted<fs::Service>([this](fidl::ServerEnd<fuchsia_fshost::Admin> request) {
          fidl::BindServer(loop_.dispatcher(), std::move(request), admin_server_.get());
          return ZX_OK;
        }));

    svc_dir->AddEntry(
        fidl::DiscoverableProtocolName<fuchsia_paver::Paver>,
        fbl::MakeRefCounted<fs::Service>([this](fidl::ServerEnd<fuchsia_paver::Paver> request) {
          paver_.Connect(loop_.dispatcher(), std::move(request));
          return ZX_OK;
        }));

    power_mock_ = std::make_unique<MockPowerStateControl>();
    svc_dir->AddEntry(
        fidl::DiscoverableProtocolName<fuchsia_hardware_power_statecontrol::Admin>,
        fbl::MakeRefCounted<fs::Service>(
            [this](fidl::ServerEnd<fuchsia_hardware_power_statecontrol::Admin> request) {
              fidl::BindServer(loop_.dispatcher(), std::move(request), power_mock_.get());
              return ZX_OK;
            }));

    auto [client, server] = fidl::Endpoints<fio::Directory>::Create();
    vfs_.ServeDirectory(svc_dir, std::move(server));
    svc_client_ = std::move(client);
    loop_.StartThread("fastboot-flash-blob-test-loop");
  }

  void TearDown() override {
    std::promise<zx_status_t> promise;
    vfs_.Shutdown([&promise](zx_status_t status) { promise.set_value(status); });
    ASSERT_EQ(promise.get_future().get(), ZX_OK);
    loop_.Shutdown();
  }

  fidl::ClientEnd<fio::Directory> TakeSvcClient() { return std::move(svc_client_); }

  std::promise<zx_status_t> GetSyncPromise() {
    return recovery_server_->image_file_->GetSyncPromise();
  }

  const zx::vmo& image_vmo() const { return recovery_server_->image_file_->vmo(); }

  bool data_volume_shredded() const { return admin_server_->data_volume_shredded_; }

  paver_test::FakePaver& paver() { return paver_; }

  void set_get_blob_image_error(const zx_status_t status) { recovery_server_->error_ = status; }

  void set_get_blob_image_unformatted() { recovery_server_->is_unformatted_ = true; }

  bool shutdown_called() const { return power_mock_->shutdown_called(); }

 private:
  async::Loop loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  fs::ManagedVfs vfs_{loop_.dispatcher()};
  std::unique_ptr<MockFshostRecovery> recovery_server_;
  std::unique_ptr<MockFshostAdmin> admin_server_;
  std::unique_ptr<MockPowerStateControl> power_mock_;
  paver_test::FakePaver paver_;
  fidl::ClientEnd<fio::Directory> svc_client_;
};

/// Flashing the blob volume requires the input be in the Android sparse format.
TEST_F(FastbootFlashBlobTest, ImageMustBeAndroidSparseFormat) {
  Fastboot fastboot(kMaxDownloadSize, TakeSvcClient());
  // Provide an image that's large enough to be an Android sparse image, but does not have the
  // correct magic.
  std::vector<uint8_t> image(sizeof(sparse_header_t));
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, image));
  TestTransport transport;
  transport.AddInPacket("flash:blob");
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error()) << "flash command should fail with a non-sparse image";
  ASSERT_EQ(ret.status_value(), ZX_ERR_NOT_SUPPORTED) << ret.status_string();
  ASSERT_EQ(transport.GetOutPackets().size(), 1ULL);
  ASSERT_EQ(transport.GetOutPackets()[0].compare(0, 4, "FAIL"), 0);
}

/// We currently attempt to resize the underlying image file to the total unsparsed size of the
/// image. Here we test trying to flash an image larger than kMaxBlobImageSize, which should result
/// in the attempt to resize the image failing with ZX_ERR_NO_SPACE.
TEST_F(FastbootFlashBlobTest, FailsIfImageIsTooLarge) {
  Fastboot fastboot(kMaxDownloadSize, TakeSvcClient());
  // Stage a sparse image that exceeds kMaxBlobImageSize when unsparsed.
  const std::vector<uint8_t> image =
      SparseImageBuilder(kBlockSize).SkipChunk(kMaxNumChunks + 1).Build();
  ASSERT_GT(fastboot::GetUnsparsedSize(image.data(), image.size()).value(), kMaxBlobImageSize)
      << "unsparsed image size is not large enough to exceed maximum image size";
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, image));

  TestTransport transport;
  transport.AddInPacket("flash:blob");
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error()) << "flash command should fail if image is too large";
  ASSERT_EQ(ret.status_value(), ZX_ERR_NO_SPACE) << ret.status_string();
  ASSERT_EQ(transport.GetOutPackets().size(), 1ULL);
  ASSERT_EQ(transport.GetOutPackets()[0].compare(0, 4, "FAIL"), 0);
}

/// Ensure that all possible chunk types are handled correctly by flashing a sparse image containing
/// the following three chunks:
///     Chunk 1: RAW        - two blocks filled with repeating pattern of 0x00, 0x01, 0x02, 0x03...
///     Chunk 2: FILL       - two blocks filled with 0xAA
///     Chunk 3: DON'T CARE - remaining blocks until we use up all kMaxNumChunks
/// We also ensure that DON'T-CARE chunks preserve existing data in the file.
TEST_F(FastbootFlashBlobTest, HandlesAllChunkTypes) {
  Fastboot fastboot(kMaxDownloadSize, TakeSvcClient());

  // Overwrite the two blocks where the DON'T-CARE chunk starts with 0xFF. This data should remain
  // the same since the don't-care/skip chunks should not modify data already in the file.
  std::vector<uint8_t> buffer(kBlockSize * 2, 0xFF);
  ASSERT_EQ(image_vmo().write(buffer.data(), kBlockSize * 4, buffer.size()), ZX_OK);

  std::vector<uint8_t> raw_chunk_data(kBlockSize * 2);
  std::ranges::generate(raw_chunk_data, [n = 0ull]() mutable { return static_cast<uint8_t>(n++); });
  constexpr uint8_t kFillPattern = 0xAA;

  // Build a sparse image containing a chunk of every known type and stage it.
  const std::vector<uint8_t> image = SparseImageBuilder(kBlockSize)
                                         .RawChunk(raw_chunk_data)
                                         .FillChunk(2, kFillPattern)
                                         .SkipChunk(kMaxNumChunks - 4)
                                         .Build();

  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, image));

  // Issue the flash command, and ensure it succeeds.
  TestTransport transport;
  transport.AddInPacket("flash:blob");
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  const std::vector<std::string> expected_packets = {"OKAY"};
  ASSERT_THAT(transport.TakeOutPackets(), testing::ContainerEq(expected_packets));

  // Ensure that the unsparsed data was written into the VMO.
  size_t offset = 0;

  // The first two blocks should contain our raw chunk data.
  ASSERT_EQ(image_vmo().read(buffer.data(), offset, buffer.size()), ZX_OK);
  offset += buffer.size();
  ASSERT_THAT(buffer, ::testing::ContainerEq(raw_chunk_data));

  // The next two blocks should contain our fill pattern.
  ASSERT_EQ(image_vmo().read(buffer.data(), offset, buffer.size()), ZX_OK);
  offset += buffer.size();
  ASSERT_THAT(buffer, ::testing::Each(kFillPattern));

  // All remaining blocks are don't-care/skip, so all existing data should be preserved there.
  // The next two blocks should contain 0xFF, followed by all zeroes.
  ASSERT_EQ(image_vmo().read(buffer.data(), offset, buffer.size()), ZX_OK);
  offset += buffer.size();
  ASSERT_THAT(buffer, ::testing::Each(uint8_t{0xFF}));
  // The remainder should be all zeroes.
  ASSERT_EQ(image_vmo().read(buffer.data(), offset, buffer.size()), ZX_OK);
  offset += buffer.size();
  ASSERT_THAT(buffer, ::testing::Each(uint8_t{0x00}));
}

/// Test flashing an image across several sparse files. This simulates what happens when the host
/// tool resparses an image to respect the maximum download buffer size.
TEST_F(FastbootFlashBlobTest, ResparsedImage) {
  Fastboot fastboot(kMaxDownloadSize, TakeSvcClient());

  constexpr size_t kNumSparseFiles = 4;
  for (size_t i = 0; i < kNumSparseFiles; ++i) {
    SparseImageBuilder builder(kBlockSize);
    if (i == 0) {
      builder.FillChunk(1, 0x00);
      builder.SkipChunk(kMaxNumChunks - (i + 1));
    } else {
      builder.SkipChunk(i);
      builder.FillChunk(1, static_cast<uint8_t>(i));
      builder.SkipChunk(kMaxNumChunks - (i + 1));
    }
    // Stage the image and ensure it is flashed correctly.
    ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, builder.Build()));
    TestTransport transport;
    transport.AddInPacket("flash:blob");
    zx::result<> ret = fastboot.ProcessPacket(&transport);
    ASSERT_TRUE(ret.is_ok()) << ret.status_string();
    const std::vector<std::string> expected_packets = {"OKAY"};
    ASSERT_THAT(transport.TakeOutPackets(), testing::ContainerEq(expected_packets));
  }

  // Ensure that the unsparsed data from all sparse files is now inside the image file.
  std::vector<uint8_t> buffer(kBlockSize);
  for (size_t i = 0; i < kNumSparseFiles; ++i) {
    const uint64_t offset = i * kBlockSize;
    ASSERT_EQ(image_vmo().read(buffer.data(), offset, kBlockSize), ZX_OK);
    ASSERT_THAT(buffer, ::testing::Each(static_cast<uint8_t>(i)));
  }
}

/// Issuing update-super command without :wipe should not affect flash blob.
TEST_F(FastbootFlashBlobTest, UpdateSuperDoesNotChangeBlobTarget) {
  Fastboot fastboot(kMaxDownloadSize, TakeSvcClient());

  TestTransport transport;
  transport.AddInPacket("update-super:super");
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  const std::vector<std::string> expected_packets = {"OKAY"};
  ASSERT_THAT(transport.TakeOutPackets(), testing::ContainerEq(expected_packets));

  const std::vector<uint8_t> image = SparseImageBuilder(kBlockSize).FillChunk(1, 0xFF).Build();
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, image));

  transport.AddInPacket("flash:blob");
  ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  ASSERT_THAT(transport.TakeOutPackets(), testing::ContainerEq(expected_packets));

  // Ensure we wrote to the blob image file.
  std::vector<uint8_t> buffer(kBlockSize);
  ASSERT_EQ(image_vmo().read(buffer.data(), 0, kBlockSize), ZX_OK);
  ASSERT_THAT(buffer, ::testing::Each(uint8_t{0xFF}));

  // Data volume should not have been shredded.
  ASSERT_FALSE(data_volume_shredded());
}

/// Issuing update-super command with :wipe should make flash blob target super instead.
TEST_F(FastbootFlashBlobTest, UpdateSuperWipeChangesFlashBlobTarget) {
  Fastboot fastboot(kMaxDownloadSize, TakeSvcClient());

  TestTransport transport;
  transport.AddInPacket("update-super:super:wipe");
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  const std::vector<std::string> expected_packets = {"OKAY"};
  ASSERT_THAT(transport.TakeOutPackets(), testing::ContainerEq(expected_packets));
  ASSERT_TRUE(data_volume_shredded());

  const std::vector<uint8_t> image = SparseImageBuilder(kBlockSize).FillChunk(1, 0xFF).Build();
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, image));

  paver().set_expected_payload_size(image.size());
  transport.AddInPacket("flash:blob");
  ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  ASSERT_THAT(transport.TakeOutPackets(), testing::ContainerEq(expected_packets));

  // Ensure the VMO wasn't written to.
  std::vector<uint8_t> buffer(kBlockSize);
  ASSERT_EQ(image_vmo().read(buffer.data(), 0, kBlockSize), ZX_OK);
  ASSERT_THAT(buffer, ::testing::Each(uint8_t{0}));

  // Ensure we wrote the payload to the "super" partition instead.
  auto trace = paver().GetCommandTrace();
  ASSERT_EQ(trace.size(), 1u);
  ASSERT_EQ(trace.front(), paver_test::Command::kWriteSparseVolume);
}

/// Issuing erase userdata should make subsequent flash blob commands target super instead.
TEST_F(FastbootFlashBlobTest, EraseUserdataChangesFlashBlobTarget) {
  Fastboot fastboot(kMaxDownloadSize, TakeSvcClient());

  TestTransport transport;
  transport.AddInPacket("erase:userdata");
  zx::result<> ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  const std::vector<std::string> expected_packets = {"OKAY"};
  ASSERT_THAT(transport.TakeOutPackets(), testing::ContainerEq(expected_packets));
  ASSERT_TRUE(data_volume_shredded());

  const std::vector<uint8_t> image = SparseImageBuilder(kBlockSize).FillChunk(1, 0xFF).Build();
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, image));

  paver().set_expected_payload_size(image.size());
  transport.AddInPacket("flash:blob");
  ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  ASSERT_THAT(transport.TakeOutPackets(), testing::ContainerEq(expected_packets));

  // Ensure the VMO wasn't written to.
  std::vector<uint8_t> buffer(kBlockSize);
  ASSERT_EQ(image_vmo().read(buffer.data(), 0, kBlockSize), ZX_OK);
  ASSERT_THAT(buffer, ::testing::Each(uint8_t{0}));

  // Ensure we wrote the payload to the "super" partition instead.
  auto trace = paver().GetCommandTrace();
  ASSERT_EQ(trace.size(), 1u);
  ASSERT_EQ(trace.front(), paver_test::Command::kWriteSparseVolume);
}

/// If the system container is unformatted, flashing blob should instead overwrite super.
TEST_F(FastbootFlashBlobTest, UnformattedDiskFormatChangesFlashBlobTarget) {
  set_get_blob_image_unformatted();
  Fastboot fastboot(kMaxDownloadSize, TakeSvcClient());

  const std::vector<uint8_t> image = SparseImageBuilder(kBlockSize).FillChunk(1, 0xFF).Build();
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, image));

  TestTransport transport;
  paver().set_expected_payload_size(image.size());
  transport.AddInPacket("flash:blob");
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  const std::vector<std::string> expected_packets = {"OKAY"};
  ASSERT_THAT(transport.TakeOutPackets(), testing::ContainerEq(expected_packets));

  // Ensure the VMO wasn't written to.
  std::vector<uint8_t> buffer(kBlockSize);
  ASSERT_EQ(image_vmo().read(buffer.data(), 0, kBlockSize), ZX_OK);
  ASSERT_THAT(buffer, ::testing::Each(uint8_t{0}));

  // Ensure we wrote the payload to the "super" partition instead.
  auto trace = paver().GetCommandTrace();
  ASSERT_EQ(trace.size(), 1u);
  ASSERT_EQ(trace.front(), paver_test::Command::kWriteSparseVolume);
}

/// If the system container is corrupt, flashing blob should fail with an error.
TEST_F(FastbootFlashBlobTest, FlashBlobFailsWithCorruptFilesystem) {
  set_get_blob_image_error(ZX_ERR_IO_DATA_INTEGRITY);
  Fastboot fastboot(kMaxDownloadSize, TakeSvcClient());

  const std::vector<uint8_t> image = SparseImageBuilder(kBlockSize).FillChunk(1, 0xFF).Build();
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, image));

  TestTransport transport;
  paver().set_expected_payload_size(image.size());
  transport.AddInPacket("flash:blob");
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error()) << "flash:blob should fail if filesystem is corrupt";
  ASSERT_EQ(ret.status_value(), ZX_ERR_IO_DATA_INTEGRITY) << ret.status_string();
  ASSERT_EQ(transport.GetOutPackets().size(), 1ULL);
  ASSERT_EQ(transport.GetOutPackets()[0].compare(0, 4, "FAIL"), 0);
}

TEST_F(FastbootFlashBlobTest, ReportsSyncErrorOnNextCommand) {
  Fastboot fastboot(kMaxDownloadSize, TakeSvcClient());

  std::promise<zx_status_t> sync_promise = GetSyncPromise();

  const std::vector<uint8_t> image = SparseImageBuilder(kBlockSize).FillChunk(1, 0xFF).Build();
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, image));

  TestTransport transport;
  transport.AddInPacket("flash:blob");
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();

  // Fail the sync.
  sync_promise.set_value(ZX_ERR_IO);

  // The next fastboot command should return the sync error. This includes flashing subsequent
  // chunks, but since sync happens in parallel, we can't guarantee that the future was resolved
  // before the next command is processed. However, we can use `oem install-blob-image` to force the
  // sync thread to be joined, which should also report the sync error back. We should also make
  // the flash:blob command block until any outstanding sync has been completed, but this will
  // negatively impact performance.
  transport.AddInPacket("oem install-blob-image");
  ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error());
  ASSERT_EQ(ret.status_value(), ZX_ERR_IO);
}

TEST_F(FastbootFlashBlobTest, ShutdownWaitsForPendingSync) {
  Fastboot fastboot(kMaxDownloadSize, TakeSvcClient());

  std::promise<zx_status_t> sync_promise = GetSyncPromise();

  const std::vector<uint8_t> image = SparseImageBuilder(kBlockSize).FillChunk(1, 0xFF).Build();
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, image));

  TestTransport transport;
  transport.AddInPacket("flash:blob");
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();

  // Issue a reboot command in a background thread.
  std::promise<zx::result<>> reboot_promise;
  std::thread reboot_thread([&fastboot, &reboot_promise]() {
    TestTransport transport;
    std::string reboot_cmd = "reboot";
    transport.AddInPacket(reboot_cmd);
    reboot_promise.set_value(fastboot.ProcessPacket(&transport));
  });

  // Complete the sync so the reboot command finishes.
  sync_promise.set_value(ZX_OK);

  auto reboot_ret = reboot_promise.get_future().get();
  ASSERT_TRUE(reboot_ret.is_ok()) << reboot_ret.status_string();
  reboot_thread.join();  // Should block until the sync promise is resolved.
  ASSERT_TRUE(shutdown_called());
}

TEST_F(FastbootFlashBlobTest, SyncErrorDoesNotPreventRebootMoreThanOnce) {
  Fastboot fastboot(kMaxDownloadSize, TakeSvcClient());

  std::promise<zx_status_t> sync_promise = GetSyncPromise();

  const std::vector<uint8_t> image = SparseImageBuilder(kBlockSize).FillChunk(1, 0xFF).Build();
  ASSERT_NO_FATAL_FAILURE(DownloadData(fastboot, image));

  TestTransport transport;
  transport.AddInPacket("flash:blob");
  zx::result ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();

  // Fail the sync.
  sync_promise.set_value(ZX_ERR_IO);

  // The first reboot should fail and report the sync error.
  transport.AddInPacket("reboot");
  ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_error());
  ASSERT_EQ(ret.status_value(), ZX_ERR_IO);
  ASSERT_FALSE(shutdown_called());

  // The next reboot should succeed because the blob_writer_ was reset after the first reboot caught
  // the error.
  transport.AddInPacket("reboot");
  ret = fastboot.ProcessPacket(&transport);
  ASSERT_TRUE(ret.is_ok()) << ret.status_string();
  ASSERT_TRUE(shutdown_called());
}

}  // namespace

}  // namespace fastboot
