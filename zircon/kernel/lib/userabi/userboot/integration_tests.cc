// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.kernel/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/userboot/testing/fixture.h>
#include <lib/zbi-format/internal/bootfs.h>
#include <lib/zbi-format/zbi.h>
#include <lib/zbitl/image.h>
#include <lib/zbitl/vmo.h>
#include <lib/zx/job.h>
#include <lib/zx/resource.h>
#include <lib/zx/vmo.h>
#include <zircon/processargs.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace {

using ::testing::HasSubstr;

using UserbootTests = userboot::testing::Fixture;

const zx::vmo& GetVdsoVmo() {
  static const zx::vmo vdso{zx_take_startup_handle(PA_HND(PA_VMO_VDSO, 0))};
  return vdso;
}

zx::vmo CreateBootfsVmo(const std::vector<std::pair<std::string, std::vector<uint8_t>>>& files) {
  uint32_t total_dirsize = 0;
  for (const auto& [name, content] : files) {
    uint32_t name_len = static_cast<uint32_t>(name.length() + 1);
    total_dirsize += ZBI_BOOTFS_DIRENT_SIZE(name_len);
  }

  uint32_t data_start_offset = ZBI_BOOTFS_PAGE_ALIGN(sizeof(zbi_bootfs_header_t) + total_dirsize);
  uint32_t total_bootfs_size = data_start_offset;
  for (const auto& [name, content] : files) {
    total_bootfs_size += ZBI_BOOTFS_PAGE_ALIGN(static_cast<uint32_t>(content.size()));
  }

  std::vector<uint8_t> bootfs_buf(total_bootfs_size, 0);

  auto* header = reinterpret_cast<zbi_bootfs_header_t*>(bootfs_buf.data());
  header->magic = ZBI_BOOTFS_MAGIC;
  header->dirsize = total_dirsize;

  uint32_t current_dir_off = sizeof(zbi_bootfs_header_t);
  uint32_t current_data_off = data_start_offset;

  for (const auto& [name, content] : files) {
    uint32_t name_len = static_cast<uint32_t>(name.length() + 1);
    auto* dirent = reinterpret_cast<zbi_bootfs_dirent_t*>(&bootfs_buf[current_dir_off]);
    dirent->name_len = name_len;
    dirent->data_len = static_cast<uint32_t>(content.size());
    dirent->data_off = current_data_off;
    memcpy(dirent->name, name.c_str(), name_len);

    if (!content.empty()) {
      memcpy(&bootfs_buf[current_data_off], content.data(), content.size());
    }

    current_dir_off += ZBI_BOOTFS_DIRENT_SIZE(name_len);
    current_data_off += ZBI_BOOTFS_PAGE_ALIGN(static_cast<uint32_t>(content.size()));
  }

  zx::vmo bootfs_vmo;
  EXPECT_EQ(zx::vmo::create(bootfs_buf.size(), 0, &bootfs_vmo), ZX_OK);
  EXPECT_EQ(bootfs_vmo.write(bootfs_buf.data(), 0, bootfs_buf.size()), ZX_OK);
  return bootfs_vmo;
}

zx::vmo CreateZbiVmo(const std::string& cmdline, const zx::vmo& bootfs_vmo) {
  zx::vmo zbi_vmo;
  EXPECT_EQ(zx::vmo::create(0, ZX_VMO_RESIZABLE, &zbi_vmo), ZX_OK);

  zbitl::Image image(zx::unowned_vmo{zbi_vmo});
  EXPECT_TRUE(image.clear().is_ok());

  if (!cmdline.empty()) {
    auto res = image.Append(zbi_header_t{.type = ZBI_TYPE_CMDLINE}, zbitl::AsBytes(cmdline));
    EXPECT_TRUE(res.is_ok());
  }

  uint64_t bootfs_size = 0;
  EXPECT_EQ(bootfs_vmo.get_size(&bootfs_size), ZX_OK);

  std::vector<uint8_t> bootfs_data(bootfs_size);
  EXPECT_EQ(bootfs_vmo.read(bootfs_data.data(), 0, bootfs_size), ZX_OK);

  auto res =
      image.Append(zbi_header_t{.type = ZBI_TYPE_STORAGE_BOOTFS}, zbitl::AsBytes(bootfs_data));
  EXPECT_TRUE(res.is_ok());

  return zbi_vmo;
}

std::vector<zx::handle> CreateUserbootHandles(zx::job root_job, const zx::vmo& zbi_vmo) {
  std::vector<zx::handle> handles;

  EXPECT_TRUE(root_job.is_valid());
  EXPECT_EQ(root_job.set_property(ZX_PROP_NAME, "root", 4), ZX_OK);
  handles.push_back(std::move(root_job));

  zx::result client = component::Connect<fuchsia_kernel::VmexResource>();
  EXPECT_TRUE(client.is_ok());
  if (client.is_ok()) {
    fidl::WireResult result = fidl::WireCall(client.value())->Get();
    EXPECT_TRUE(result.ok());
    if (result.ok()) {
      zx::resource system_resource = std::move(result.value().resource);
      handles.push_back(std::move(system_resource));
    }
  }

  zx::vmo zbi_dup;
  EXPECT_EQ(zbi_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &zbi_dup), ZX_OK);
  EXPECT_EQ(zbi_dup.set_property(ZX_PROP_NAME, "zbi", 3), ZX_OK);
  handles.push_back(std::move(zbi_dup));

  const zx::vmo& vdso_vmo = GetVdsoVmo();
  EXPECT_TRUE(vdso_vmo.is_valid());
  zx::vmo vdso_dup;
  EXPECT_EQ(vdso_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vdso_dup), ZX_OK);
  EXPECT_EQ(vdso_dup.set_property(ZX_PROP_NAME, "vdso/stable", 11), ZX_OK);
  handles.push_back(std::move(vdso_dup));

  return handles;
}

TEST_F(UserbootTests, UserbootRust) {
  auto child_vmo = userboot::testing::GetExecutable("/pkg/test/userboot-lib-static-pie-test");
  ASSERT_TRUE(child_vmo.is_ok()) << child_vmo.status_string();

  uint64_t child_size = 0;
  ASSERT_EQ(child_vmo->get_size(&child_size), ZX_OK);
  std::vector<uint8_t> child_bytes(child_size);
  ASSERT_EQ(child_vmo->read(child_bytes.data(), 0, child_size), ZX_OK);

  zx::vmo bootfs_vmo = CreateBootfsVmo({{"bin/userboot-child", child_bytes}});
  zx::vmo zbi_vmo = CreateZbiVmo("userboot.next=bin/userboot-child", bootfs_vmo);

  userboot::testing::TestJob test_job;
  test_job.Init();

  std::vector<zx::handle> handles = CreateUserbootHandles(test_job.Get(), zbi_vmo);

  ASSERT_NO_FATAL_FAILURE(Launch("/pkg/bin/userboot_rust", std::move(handles)));

  auto result = Wait();
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(*result, 0);

  std::string log = FinishLog();
  EXPECT_THAT(log, HasSubstr("Started child process: bin/userboot-child")) << log;
  EXPECT_THAT(log, HasSubstr("Hello from userland!")) << log;
}

TEST_F(UserbootTests, UserbootRustTest) {
  auto child_vmo = userboot::testing::GetExecutable("/pkg/test/userboot-lib-static-pie-test");
  ASSERT_TRUE(child_vmo.is_ok()) << child_vmo.status_string();

  uint64_t child_size = 0;
  ASSERT_EQ(child_vmo->get_size(&child_size), ZX_OK);
  std::vector<uint8_t> child_bytes(child_size);
  ASSERT_EQ(child_vmo->read(child_bytes.data(), 0, child_size), ZX_OK);

  zx::vmo bootfs_vmo = CreateBootfsVmo({{"test/userboot-child", child_bytes}});
  zx::vmo zbi_vmo = CreateZbiVmo("userboot.test.next=test/userboot-child", bootfs_vmo);

  userboot::testing::TestJob test_job;
  test_job.Init();

  std::vector<zx::handle> handles = CreateUserbootHandles(test_job.Get(), zbi_vmo);

  ASSERT_NO_FATAL_FAILURE(Launch("/pkg/bin/userboot_test_rust", std::move(handles)));

  auto result = Wait();
  ASSERT_TRUE(result.is_ok());
  EXPECT_EQ(*result, 0);

  std::string log = FinishLog();
  EXPECT_THAT(log, HasSubstr("Started child process: test/userboot-child")) << log;
  EXPECT_THAT(log, HasSubstr("Hello from userland!")) << log;
}

}  // namespace
