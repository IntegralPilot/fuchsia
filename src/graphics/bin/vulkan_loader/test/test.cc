// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <fidl/fuchsia.gpu.magma/cpp/test_base.h>
#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.sys2/cpp/wire.h>
#include <fidl/fuchsia.vulkan.loader/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/fdio/directory.h>
#include <lib/fdio/namespace.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/fit/defer.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/zx/vmo.h>
#include <zircon/status.h>
#include <zircon/types.h>

#include <filesystem>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

// This is the first and only ICD loaded, so it should have a "0-" prepended.
const char* kIcdFilename = "0-libvulkan_fake.so";

// This file contains hermetic tests of the Vulkan loader service. A hermetic copy of the service
// and its dependencies are created inside RealmBuilder, using test_realm.cm as a template. The
// /dev/class/gpu and /dev/class/goldfish-pipe server implementations are housed in
// pkg-server-main.cc, along with a fake Magma MSD implementation.
//
// The Magma MSD implementation provides an ICD component path of
// fuchsia-pkg://fuchsia.com/vulkan_loader_tests#meta/test_vulkan_driver.cm for the vulkan loader to
// read from. The "ICD" contained there has a normal manifest.json and libvulkan_fake.json file, but
// there's no real ICD - instead bin/pkg-server (the same pkg-server executable as above) is used as
// the ICD shared library, since the contents don't matter for these tests as long as the file is
// marked executable.

class VulkanLoader : public testing::TestWithParam<bool> {
 public:
  bool is_trusted() const { return GetParam(); }

 protected:
  void SetUp() override {
    loop_.StartThread();

    auto builder = component_testing::RealmBuilder::CreateFromRelativeUrl("#meta/test_realm.cm");
    realm_ = builder.Build(loop_.dispatcher());

    fidl::ClientEnd<fuchsia_vulkan_loader::Loader> client_end;
    if (is_trusted()) {
      fidl::UnownedClientEnd<fuchsia_io::Directory> svc_root(
          realm().component().exposed().unowned_channel());

      component::SyncServiceMemberWatcher<fuchsia_vulkan_loader::TrustedService::Loader> watcher(
          std::move(svc_root));

      // Wait indefinitely until a service instance appears in the service directory
      auto client_end_result = watcher.GetNextInstance(false);
      ASSERT_TRUE(client_end_result.is_ok());

      client_end = std::move(client_end_result.value());
    } else {
      auto client_end_result = realm().component().Connect<fuchsia_vulkan_loader::Loader>();
      ASSERT_TRUE(client_end_result.is_ok());

      client_end = std::move(client_end_result.value());
    }

    loader_ = fidl::WireSyncClient(std::move(client_end));
  }

  void TearDown() override { loop_.Shutdown(); }

  component_testing::RealmRoot& realm() { return *realm_; }

  const auto& loader() const { return *loader_; }

  zx::result<zx::vmo> GetIcd(std::string_view icd_filename) {
    auto response = loader()->Get(fidl::StringView::FromExternal(icd_filename));
    if (!response.ok()) {
      return zx::error(response.status());
    }
    return zx::ok(std::move(response->lib));
  }

 private:
  async::Loop loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  std::optional<component_testing::RealmRoot> realm_;
  std::optional<fidl::WireSyncClient<fuchsia_vulkan_loader::Loader>> loader_;
};

// Test that loader service can use `metadata.json` and `libvulkan_fake.json` to load the ICD, and
// that the ICD VMO returned has the correct properties.
TEST_P(VulkanLoader, ManifestLoad) {
  // manifest.json remaps 0-libvulkan_fake.so to bin/pkg-server.
  zx::result icd = GetIcd(kIcdFilename);
  ASSERT_TRUE(icd.is_ok()) << icd.status_string();
  ASSERT_TRUE(icd->is_valid());
  zx_info_handle_basic_t handle_info;
  ASSERT_EQ(
      icd->get_info(ZX_INFO_HANDLE_BASIC, &handle_info, sizeof(handle_info), nullptr, nullptr),
      ZX_OK);
  EXPECT_TRUE(handle_info.rights & ZX_RIGHT_EXECUTE);
  EXPECT_FALSE(handle_info.rights & ZX_RIGHT_WRITE);

  zx::result not_present = GetIcd("not-present");
  ASSERT_TRUE(not_present.is_ok()) << not_present.status_string();
  EXPECT_FALSE(not_present->is_valid());
}

// Check that writes to one VMO returned by the server will not modify a separate VMO returned by
// the service. Requires that zx_process_write_memory is enabled on the device for the result to be
// meaningful.
TEST_P(VulkanLoader, VmosIndependent) {
  // manifest.json remaps this to bin/pkg-server.
  zx::result icd = GetIcd(kIcdFilename);
  ASSERT_TRUE(icd.is_ok()) << icd.status_string();
  ASSERT_TRUE(icd->is_valid());

  fzl::VmoMapper mapper;
  ASSERT_EQ(mapper.Map(*icd, 0, 0, ZX_VM_PERM_EXECUTE | ZX_VM_PERM_READ | ZX_VM_ALLOW_FAULTS),
            ZX_OK);
  uint8_t original_value = *static_cast<uint8_t*>(mapper.start());
  uint8_t byte_to_write = original_value + 1;
  size_t actual;
  // zx_process_write_memory can write to memory mapped without ZX_VM_PERM_WRITE. If that ever
  // changes, this test can probably be removed.
  zx_status_t status = zx::process::self()->write_memory(
      reinterpret_cast<uint64_t>(mapper.start()), &byte_to_write, sizeof(byte_to_write), &actual);

  // zx_process_write_memory may be disabled using a kernel command-line flag.
  if (status == ZX_ERR_NOT_SUPPORTED) {
    EXPECT_EQ(original_value, *static_cast<uint8_t*>(mapper.start()));
  } else {
    EXPECT_EQ(ZX_OK, status);

    EXPECT_EQ(byte_to_write, *static_cast<uint8_t*>(mapper.start()));
  }

  // Ensure that the new clone is unaffected.
  zx::result icd2 = GetIcd(kIcdFilename);
  ASSERT_TRUE(icd2.is_ok()) << icd2.status_string();
  ASSERT_TRUE(icd2->is_valid());

  fzl::VmoMapper mapper2;
  ASSERT_EQ(mapper2.Map(*icd2, 0, 0, ZX_VM_PERM_EXECUTE | ZX_VM_PERM_READ | ZX_VM_ALLOW_FAULTS),
            ZX_OK);
  EXPECT_EQ(original_value, *static_cast<uint8_t*>(mapper2.start()));
}

// Test that the DeviceFs returned from `ConnectToDeviceFs` looks as expected.
TEST_P(VulkanLoader, DeviceFs) {
  auto dev_fs = fidl::Endpoints<fuchsia_io::Directory>::Create();
  {
    auto response = loader()->ConnectToDeviceFs(dev_fs.server.TakeChannel());
    ASSERT_TRUE(response.ok()) << response;
  }
  ASSERT_TRUE(GetIcd(kIcdFilename).is_ok());  // Wait for idle.

  auto device = fidl::CreateEndpoints<fuchsia_gpu_magma::Device>();
  ASSERT_TRUE(device.is_ok()) << device.status_string();
  zx_status_t status =
      fdio_service_connect_at(dev_fs.client.channel().get(), "svc/magma/some-instance-name/device",
                              device->server.TakeChannel().release());
  ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);

  auto response =
      fidl::WireCall(device->client)->Query(fuchsia_gpu_magma::wire::QueryId::kVendorId);
  ASSERT_TRUE(response.ok()) << response;
  ASSERT_TRUE(response->is_ok()) << zx_status_get_string(response->error_value());
  ASSERT_TRUE((*response)->is_simple_result());
  EXPECT_EQ((*response)->simple_result(), 5u);
}

// Test that `GetSupportedFeatures` returns the correct values.
TEST_P(VulkanLoader, Features) {
  auto response = loader()->GetSupportedFeatures();
  ASSERT_TRUE(response.ok()) << response;
  constexpr fuchsia_vulkan_loader::Features kExpectedFeatures =
      fuchsia_vulkan_loader::Features::kConnectToDeviceFs |
      fuchsia_vulkan_loader::Features::kConnectToManifestFs | fuchsia_vulkan_loader::Features::kGet;
  EXPECT_EQ(response->features, kExpectedFeatures);
}

// Test that ConnectToManifestFs gives access to a manifest fs with the expected files.
// https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0205_vulkan_loader?hl=en#filesystem_serving
// describes the contents of this filesystem.
TEST_P(VulkanLoader, ManifestFs) {
  auto manifest_fs = fidl::Endpoints<fuchsia_io::Directory>::Create();
  {
    auto response =
        loader()->ConnectToManifestFs(fuchsia_vulkan_loader::ConnectToManifestOptions::kWaitForIdle,
                                      manifest_fs.server.TakeChannel());
    ASSERT_TRUE(response.ok()) << response;
  }

  fbl::unique_fd dir_fd;
  zx_status_t status =
      fdio_fd_create(manifest_fs.client.TakeChannel().release(), dir_fd.reset_and_get_address());
  ASSERT_EQ(status, ZX_OK) << zx_status_get_string(status);

  fbl::unique_fd manifest_fd(
      openat(dir_fd.get(), (std::string(kIcdFilename) + ".json").c_str(), O_RDONLY));
  ASSERT_TRUE(manifest_fd.is_valid()) << strerror(errno);

  constexpr int kManifestFileSize = 135;
  char manifest_data[kManifestFileSize + 1];
  ssize_t read_size = read(manifest_fd.get(), manifest_data, sizeof(manifest_data) - 1);
  EXPECT_EQ(kManifestFileSize, read_size);
}

// Test that the goldfish files in the device-fs are connected correctly.
TEST_P(VulkanLoader, GoldfishSyncDeviceFs) {
  auto dev_fs = fidl::Endpoints<fuchsia_io::Directory>::Create();
  {
    auto response = loader()->ConnectToDeviceFs(dev_fs.server.TakeChannel());
    ASSERT_TRUE(response.ok()) << response;
  }
  ASSERT_TRUE(GetIcd(kIcdFilename).is_ok());  // Wait for idle.

  const char* kDeviceClassList[] = {
      "class/goldfish-sync",
      "class/goldfish-pipe",
      "class/goldfish-address-space",
  };

  for (auto& device_class : kDeviceClassList) {
    fuchsia::io::DirectorySyncPtr device_ptr;
    EXPECT_EQ(ZX_OK, fdio_open3_at(dev_fs.client.channel().get(), device_class,
                                   uint64_t{fuchsia::io::PERM_READABLE},
                                   device_ptr.NewRequest().TakeChannel().release()));

    // Check that the directory is connected to something.
    std::vector<uint8_t> protocol;
    zx_status_t status = device_ptr->Query(&protocol);
    EXPECT_EQ(status, ZX_OK) << "class=" << device_class
                             << " status=" << zx_status_get_string(status);
  }
}

// Test that the manifest FS and device FS are exposed through out/debug from the component, and
// that they have the right contents (see src/graphics/bin/vulkan_loader/README.md for details).
TEST_P(VulkanLoader, DebugFilesystems) {
  ASSERT_TRUE(GetIcd(kIcdFilename).is_ok());  // Wait for idle.

  auto client_end = realm().component().Connect<fuchsia_sys2::RealmQuery>();
  ASSERT_TRUE(client_end.is_ok()) << client_end.status_string();

  auto [outgoing_client, outgoing_server] = fidl::Endpoints<fuchsia_io::Directory>::Create();

  auto response = fidl::WireCall(*client_end)
                      ->OpenDirectory("./vulkan_loader", fuchsia_sys2::OpenDirType::kOutgoingDir,
                                      std::move(outgoing_server));
  ASSERT_TRUE(response.ok()) << response;
  ASSERT_TRUE(response->is_ok()) << static_cast<uint32_t>(response->error_value());

  fdio_ns_t* ns;
  EXPECT_EQ(ZX_OK, fdio_ns_get_installed(&ns));
  EXPECT_EQ(ZX_OK, fdio_ns_bind(ns, "/loader_out", outgoing_client.TakeChannel().release()));
  auto cleanup_binding = fit::defer([&]() { fdio_ns_unbind(ns, "/loader_out"); });

  const std::string debug_path("/loader_out/debug/");

  EXPECT_TRUE(std::filesystem::exists(debug_path + "device-fs/class/goldfish-sync/000"));
  EXPECT_TRUE(std::filesystem::exists(debug_path + "manifest-fs/" + kIcdFilename + ".json"));
}

INSTANTIATE_TEST_SUITE_P(, VulkanLoader, testing::Values(true, false),
                         [](testing::TestParamInfo<bool> info) {
                           return info.param ? "Trusted" : "Untrusted";
                         });
