// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/fs_management/cpp/admin.h"

#include <fidl/fuchsia.io/cpp/wire.h>
#include <lib/fdio/directory.h>
#include <lib/zx/channel.h>

#include <vector>

#include <gtest/gtest.h>
#include <ramdevice-client/ramdisk.h>

#include "src/storage/fs_test/crypt_service.h"
#include "src/storage/lib/fs_management/cpp/format.h"
#include "src/storage/lib/fs_management/cpp/mkfs_with_default.h"
#include "src/storage/lib/fs_management/cpp/mount.h"

namespace fs_management {
namespace {

namespace fio = fuchsia_io;
using fuchsia_io::Directory;

enum State {
  kFormatted,
  kStarted,
};

enum class Mode {
  // A statically routed component.
  kStatic,

  // A dynamically routed component.
  kDynamic,
};

constexpr char kTestFilePath[] = "test_file";

class OutgoingDirectoryFixture : public testing::Test {
 public:
  explicit OutgoingDirectoryFixture(DiskFormat format, Mode mode) : format_(format), mode_(mode) {}

  void TearDown() final { ASSERT_NO_FATAL_FAILURE(StopFilesystem()); }

  fidl::ClientEnd<Directory> DataRoot() {
    ZX_ASSERT(component_);  // Ensure this isn't used after stopping the filesystem.
    auto data = fs_->DataRoot();
    ZX_ASSERT_MSG(data.is_ok(), "Invalid data root: %s", data.status_string());
    return std::move(*data);
  }

  const fidl::ClientEnd<Directory>& ExportRoot() {
    ZX_ASSERT(component_);  // Ensure this isn't used after stopping the filesystem.
    return fs_->ExportRoot();
  }

 protected:
  void StartFilesystem(MountOptions options) {
    ASSERT_FALSE(component_);

    switch (mode_) {
      case Mode::kStatic: {
        std::string name = "static-test-";
        name.append(DiskFormatString(format_));
        component_ = fs_management::FsComponent::StaticChild(name, format_);
        break;
      }
      case Mode::kDynamic:
        component_ = fs_management::FsComponent::FromDiskFormat(format_);
    }

    if (!ramdisk_.is_valid()) {
      zx::result ramdisk = ramdevice_client::Ramdisk::Create(512, 1 << 17);
      ASSERT_EQ(ramdisk.status_value(), ZX_OK);
      ramdisk_ = std::move(*ramdisk);

      zx_status_t status;
      if (format_ == kDiskFormatFxfs) {
        zx::result service = fs_test::InitializeCryptService();
        ASSERT_TRUE(service.is_ok());
        zx::result status =
            MkfsWithDefault(ramdisk_.path().c_str(), *component_, {}, *std::move(service));
        ASSERT_TRUE(status.is_ok()) << status.status_string();
      } else {
        ASSERT_EQ(status = Mkfs(ramdisk_.path().c_str(), *component_, {}), ZX_OK)
            << zx_status_get_string(status);
      }

      ASSERT_EQ(status = Fsck(ramdisk_.path().c_str(), *component_, {}), ZX_OK)
          << zx_status_get_string(status);
    }

    zx::result device = ramdisk_.ConnectBlock();
    ASSERT_EQ(device.status_value(), ZX_OK);

    if (format_ == kDiskFormatFxfs) {
      options.crypt_client = [] {
        zx::result crypt_service = fs_test::InitializeCryptService();
        if (crypt_service.is_error()) {
          ADD_FAILURE() << "Unable to get crypt service: " << crypt_service.status_string();
        }
        return crypt_service;
      };
      auto fs = MountMultiVolumeWithDefault(std::move(device.value()), *component_, options);
      ASSERT_TRUE(fs.is_ok()) << fs.status_string();
      fs_ = std::make_unique<StartedSingleVolumeMultiVolumeFilesystem>(std::move(*fs));
    } else {
      auto fs = Mount(std::move(device.value()), *component_, options);
      ASSERT_TRUE(fs.is_ok()) << fs.status_string();
      fs_ = std::make_unique<StartedSingleVolumeFilesystem>(std::move(*fs));
    }
  }

  void StopFilesystem() {
    if (!component_)
      return;

    ASSERT_EQ(fs_->Unmount().status_value(), ZX_OK);

    fs_.reset();
    if (mode_ == Mode::kDynamic)
      component_.reset();
  }

 private:
  ramdevice_client::Ramdisk ramdisk_;
  DiskFormat format_;
  Mode mode_;
  std::optional<fs_management::FsComponent> component_;
  std::unique_ptr<SingleVolumeFilesystemInterface> fs_;
};

// Generalized Admin Tests

std::string PrintTestSuffix(const testing::TestParamInfo<std::tuple<DiskFormat, Mode>> params) {
  std::stringstream out;
  out << DiskFormatString(std::get<0>(params.param));
  switch (std::get<1>(params.param)) {
    case Mode::kDynamic:
      out << "_dynamic";
      __FALLTHROUGH;
    case Mode::kStatic:
      out << "_component";
      break;
  }
  return out.str();
}

// Generalized outgoing directory tests which should work in both mutable and read-only modes.
class OutgoingDirectoryTest : public OutgoingDirectoryFixture,
                              public testing::WithParamInterface<std::tuple<DiskFormat, Mode>> {
 public:
  OutgoingDirectoryTest()
      : OutgoingDirectoryFixture(std::get<0>(GetParam()), std::get<1>(GetParam())) {}
};

TEST_P(OutgoingDirectoryTest, DataRootIsValid) {
  ASSERT_NO_FATAL_FAILURE(StartFilesystem({}));

  std::string_view format_str = DiskFormatString(std::get<0>(GetParam()));
  auto resp = fidl::WireCall(DataRoot())->QueryFilesystem();
  ASSERT_TRUE(resp.ok()) << resp.status_string();
  ASSERT_EQ(resp.value().s, ZX_OK) << zx_status_get_string(resp.value().s);
  ASSERT_STREQ(format_str.data(), reinterpret_cast<char*>(resp.value().info->name.data()));
}

using Combinations = std::vector<std::tuple<DiskFormat, Mode>>;

Combinations TestCombinations() {
  Combinations c;

  auto add = [&](DiskFormat format, std::initializer_list<Mode> modes) {
    for (Mode mode : modes) {
      c.emplace_back(format, mode);
    }
  };

  add(kDiskFormatBlobfs, {Mode::kDynamic, Mode::kStatic});
  add(kDiskFormatMinfs, {Mode::kDynamic, Mode::kStatic});
  add(kDiskFormatFxfs, {Mode::kDynamic, Mode::kStatic});
  add(kDiskFormatF2fs, {Mode::kDynamic, Mode::kStatic});

  return c;
}

INSTANTIATE_TEST_SUITE_P(OutgoingDirectoryTest, OutgoingDirectoryTest,
                         testing::ValuesIn(TestCombinations()), PrintTestSuffix);

// Minfs-Specific Tests (can be generalized to work with any mutable filesystem by parameterizing
// on the disk format if required).
// Launches the filesystem and creates a file called kTestFilePath in the data root.
class OutgoingDirectoryMinfs : public OutgoingDirectoryFixture {
 public:
  // Some test cases involve remounting, which will not work for Mode::kStatic.
  OutgoingDirectoryMinfs() : OutgoingDirectoryFixture(kDiskFormatMinfs, Mode::kDynamic) {}

 protected:
  void WriteTestFile() {
    auto [client, server] = fidl::Endpoints<fio::File>::Create();
    ASSERT_EQ(fidl::WireCall(DataRoot())
                  ->Open(kTestFilePath,
                         fio::wire::kPermReadable | fio::wire::kPermWritable |
                             fio::wire::Flags::kFlagMaybeCreate | fio::wire::Flags::kProtocolFile,
                         {}, server.TakeChannel())
                  .status(),
              ZX_OK);

    fidl::WireSyncClient<fio::File> file_client(std::move(client));
    std::vector<uint8_t> content{1, 2, 3, 4};
    const fidl::WireResult res =
        file_client->Write(fidl::VectorView<uint8_t>::FromExternal(content));
    ASSERT_TRUE(res.ok()) << res.status_string();
    const fit::result resp = res.value();
    ASSERT_TRUE(resp.is_ok()) << zx_status_get_string(resp.error_value());
    ASSERT_EQ(resp.value()->actual_count, content.size());

    auto resp2 = file_client->Close();
    ASSERT_TRUE(resp2.ok()) << resp2.status_string();
    ASSERT_TRUE(resp2->is_ok()) << zx_status_get_string(resp2->error_value());
  }
};

TEST_F(OutgoingDirectoryMinfs, CannotWriteToReadOnlyDataRoot) {
  ASSERT_NO_FATAL_FAILURE(StartFilesystem({}));
  ASSERT_NO_FATAL_FAILURE(WriteTestFile());

  // Restart the filesystem in read-only mode
  ASSERT_NO_FATAL_FAILURE(StopFilesystem());
  ASSERT_NO_FATAL_FAILURE(StartFilesystem({.readonly = true}));

  auto data_root = DataRoot();
  {
    auto [client, server] = fidl::Endpoints<fio::File>::Create();
    // open "succeeds" but...
    auto open_resp = fidl::WireCall(data_root)->Open(
        kTestFilePath, fio::wire::kPermReadable | fio::wire::kPermWritable, {},
        server.TakeChannel());
    ASSERT_TRUE(open_resp.ok()) << open_resp.status_string();
    // ...we can't actually use the channel
    fidl::WireSyncClient<fio::File> fail_file_client(std::move(client));
    const fidl::WireResult res1 = fail_file_client->Read(4);
    ASSERT_EQ(res1.status(), ZX_ERR_PEER_CLOSED) << res1.status_string();
  }

  {
    // the channel will be valid if we open the file read-only though
    auto [client, server] = fidl::Endpoints<fio::File>::Create();

    auto open_resp = fidl::WireCall(data_root)->Open(kTestFilePath, fio::wire::kPermReadable, {},
                                                     server.TakeChannel());
    ASSERT_TRUE(open_resp.ok()) << open_resp.status_string();

    fidl::WireSyncClient<fio::File> file_client(std::move(client));
    const fidl::WireResult res2 = file_client->Read(4);
    ASSERT_TRUE(res2.ok()) << res2.status_string();
    const fit::result resp2 = res2.value();
    ASSERT_TRUE(resp2.is_ok()) << zx_status_get_string(resp2.error_value());
    ASSERT_EQ(resp2.value()->data[0], 1);

    auto close_resp = file_client->Close();
    ASSERT_TRUE(close_resp.ok()) << close_resp.status_string();
    ASSERT_TRUE(close_resp->is_ok()) << zx_status_get_string(close_resp->error_value());
  }
}

TEST_F(OutgoingDirectoryMinfs, CannotWriteToOutgoingDirectory) {
  ASSERT_NO_FATAL_FAILURE(StartFilesystem({}));
  ASSERT_NO_FATAL_FAILURE(WriteTestFile());

  auto [client, server] = fidl::Endpoints<fio::File>::Create();
  auto open_resp =
      fidl::WireCall(ExportRoot())
          ->Open(kTestFilePath,
                 fio::wire::kPermReadable | fio::wire::kPermWritable |
                     fio::wire::Flags::kFlagMaybeCreate | fio::wire::Flags::kProtocolFile,
                 {}, server.TakeChannel());
  ASSERT_TRUE(open_resp.ok()) << open_resp.status_string();

  fidl::WireSyncClient<fio::File> file_client(std::move(client));
  std::vector<uint8_t> content{1, 2, 3, 4};
  auto write_resp = file_client->Write(fidl::VectorView<uint8_t>::FromExternal(content));
  ASSERT_EQ(write_resp.status(), ZX_ERR_PEER_CLOSED) << write_resp.status_string();
}

}  // namespace
}  // namespace fs_management
