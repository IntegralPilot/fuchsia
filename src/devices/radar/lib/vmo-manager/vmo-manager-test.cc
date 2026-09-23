// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "vmo-manager.h"

#include <array>

#include <zxtest/zxtest.h>

namespace {

constexpr size_t kTestBurstSize = 10'000;

}  // namespace

namespace radar {

void RegisterVmos(VmoManager& manager, std::array<zx::unowned_vmo, 10>& out_vmos) {
  zx::vmo registered_vmos[10];
  uint32_t vmo_ids[10];

  for (uint32_t i = 0; i < std::size(registered_vmos); i++) {
    vmo_ids[i] = i + 1;
    ASSERT_OK(zx::vmo::create(kTestBurstSize, 0, &registered_vmos[i]));
    out_vmos[i] = registered_vmos[i].borrow();
  }

  const auto status = manager.RegisterVmos(
      fidl::VectorView<const uint32_t>::FromExternal(vmo_ids, std::size(vmo_ids)),
      fidl::VectorView<zx::vmo>::FromExternal(registered_vmos, std::size(registered_vmos)));
  EXPECT_EQ(status, fuchsia_hardware_radar::wire::StatusCode::kSuccess);
}

void KoidsEqual(zx_handle_t handle1, zx_handle_t handle2) {
  zx_info_handle_basic_t info1{};
  EXPECT_OK(
      zx_object_get_info(handle1, ZX_INFO_HANDLE_BASIC, &info1, sizeof(info1), nullptr, nullptr));

  zx_info_handle_basic_t info2{};
  EXPECT_OK(
      zx_object_get_info(handle2, ZX_INFO_HANDLE_BASIC, &info2, sizeof(info2), nullptr, nullptr));

  EXPECT_EQ(info1.koid, info2.koid);
}

TEST(VmoManagerTest, VmoAlreadyRegistered) {
  VmoManager manager(kTestBurstSize);

  std::array<zx::unowned_vmo, 10> vmos;
  EXPECT_NO_FAILURES(RegisterVmos(manager, vmos));

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kTestBurstSize, 0, &vmo));

  uint32_t vmo_id = 5;
  auto status = manager.RegisterVmos(fidl::VectorView<const uint32_t>::FromExternal(&vmo_id, 1),
                                     fidl::VectorView<zx::vmo>::FromExternal(&vmo, 1));
  EXPECT_EQ(status, fuchsia_hardware_radar::wire::StatusCode::kVmoAlreadyRegistered);

  ASSERT_OK(zx::vmo::create(kTestBurstSize, 0, &vmo));
  vmo_id = 10;
  status = manager.RegisterVmos(fidl::VectorView<const uint32_t>::FromExternal(&vmo_id, 1),
                                fidl::VectorView<zx::vmo>::FromExternal(&vmo, 1));
  EXPECT_EQ(status, fuchsia_hardware_radar::wire::StatusCode::kVmoAlreadyRegistered);

  // Make sure the existing VMOs stayed valid when registration failed.
  uint32_t vmo_ids[] = {5, 10};
  zx::vmo unregistered_vmos[2];
  status = manager.UnregisterVmos(
      fidl::VectorView<const uint32_t>::FromExternal(vmo_ids, std::size(vmo_ids)),
      fidl::VectorView<zx::vmo>::FromExternal(unregistered_vmos, std::size(unregistered_vmos)));
  EXPECT_EQ(status, fuchsia_hardware_radar::wire::StatusCode::kSuccess);

  EXPECT_NO_FAILURES(KoidsEqual(unregistered_vmos[0].get(), vmos[4]->get()));
  EXPECT_NO_FAILURES(KoidsEqual(unregistered_vmos[1].get(), vmos[9]->get()));
}

TEST(VmoManagerTest, VmoTooSmall) {
  VmoManager manager(kTestBurstSize);

  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(kTestBurstSize / 2, 0, &vmo));

  uint32_t vmo_id = 1;
  const auto status =
      manager.RegisterVmos(fidl::VectorView<const uint32_t>::FromExternal(&vmo_id, 1),
                           fidl::VectorView<zx::vmo>::FromExternal(&vmo, 1));
  EXPECT_EQ(status, fuchsia_hardware_radar::wire::StatusCode::kVmoTooSmall);
}

TEST(VmoManagerTest, UndoRegistrationOnError) {
  VmoManager manager(kTestBurstSize);

  zx::vmo vmos[10];
  uint32_t vmo_ids[10];

  for (uint32_t i = 0; i < std::size(vmos); i++) {
    vmo_ids[i] = i + 1;
    if (i != 5) {
      ASSERT_OK(zx::vmo::create(kTestBurstSize, 0, &vmos[i]));
    }
  }

  auto status = manager.RegisterVmos(
      fidl::VectorView<const uint32_t>::FromExternal(vmo_ids, std::size(vmo_ids)),
      fidl::VectorView<zx::vmo>::FromExternal(vmos, std::size(vmos)));
  EXPECT_EQ(status, fuchsia_hardware_radar::wire::StatusCode::kVmoBadHandle);

  for (const uint32_t vmo_id : vmo_ids) {
    zx::vmo vmo;
    status = manager.UnregisterVmos(fidl::VectorView<const uint32_t>::FromExternal(&vmo_id, 1),
                                    fidl::VectorView<zx::vmo>::FromExternal(&vmo, 1));
    EXPECT_EQ(status, fuchsia_hardware_radar::wire::StatusCode::kVmoNotFound);
  }
}

TEST(VmoManagerTest, UnregisterInvalidVmos) {
  VmoManager manager(kTestBurstSize);

  std::array<zx::unowned_vmo, 10> vmos;
  EXPECT_NO_FAILURES(RegisterVmos(manager, vmos));

  // Invalid VMO ID provided, other VMOs should remain registered.
  uint32_t vmo_ids[] = {4, 5, 11, 7};
  zx::vmo unregistered_vmos[4];
  auto status = manager.UnregisterVmos(
      fidl::VectorView<const uint32_t>::FromExternal(vmo_ids, std::size(vmo_ids)),
      fidl::VectorView<zx::vmo>::FromExternal(unregistered_vmos, std::size(unregistered_vmos)));
  EXPECT_EQ(status, fuchsia_hardware_radar::wire::StatusCode::kVmoNotFound);

  vmo_ids[2] = 6;
  status = manager.UnregisterVmos(
      fidl::VectorView<const uint32_t>::FromExternal(vmo_ids, std::size(vmo_ids)),
      fidl::VectorView<zx::vmo>::FromExternal(unregistered_vmos, std::size(unregistered_vmos)));
  EXPECT_EQ(status, fuchsia_hardware_radar::wire::StatusCode::kSuccess);

  EXPECT_NO_FAILURES(KoidsEqual(unregistered_vmos[0].get(), vmos[3]->get()));
  EXPECT_NO_FAILURES(KoidsEqual(unregistered_vmos[1].get(), vmos[4]->get()));
  EXPECT_NO_FAILURES(KoidsEqual(unregistered_vmos[2].get(), vmos[5]->get()));
  EXPECT_NO_FAILURES(KoidsEqual(unregistered_vmos[3].get(), vmos[6]->get()));
}

TEST(VmoManagerTest, GetUnlockedVmo) {
  VmoManager manager(kTestBurstSize);

  std::array<zx::unowned_vmo, 10> vmos;
  EXPECT_NO_FAILURES(RegisterVmos(manager, vmos));

  constexpr uint8_t kTestBurstData[] = "test burst data";

  // Get all VMOs without unlocking any of them.
  for (uint32_t i = 0; i < std::size(vmos); i++) {
    fit::result vmo_id = manager.WriteUnlockedVmoAndGetId({kTestBurstData, sizeof(kTestBurstData)});
    ASSERT_TRUE(vmo_id.is_ok());
    ASSERT_GE(*vmo_id, 1);
    ASSERT_LE(*vmo_id, 10);

    char buffer[sizeof(kTestBurstData)];
    EXPECT_OK(vmos[*vmo_id - 1]->read(buffer, 0, sizeof(buffer)));
    EXPECT_BYTES_EQ(buffer, kTestBurstData, sizeof(kTestBurstData));
  }

  // No VMOs remain, the next call should fail.
  EXPECT_TRUE(manager.WriteUnlockedVmoAndGetId({}).is_error());

  // Unlock VMO 5 so that it can be returned again.
  manager.UnlockVmo(5);

  fit::result vmo_id = manager.WriteUnlockedVmoAndGetId({kTestBurstData, sizeof(kTestBurstData)});
  ASSERT_TRUE(vmo_id.is_ok());
  EXPECT_EQ(*vmo_id, 5);

  manager.UnlockVmo(1);
  vmo_id = manager.WriteUnlockedVmoAndGetId({kTestBurstData, sizeof(kTestBurstData)});
  ASSERT_TRUE(vmo_id.is_ok());
  EXPECT_EQ(*vmo_id, 1);

  manager.UnlockVmo(10);
  vmo_id = manager.WriteUnlockedVmoAndGetId({kTestBurstData, sizeof(kTestBurstData)});
  ASSERT_TRUE(vmo_id.is_ok());
  EXPECT_EQ(*vmo_id, 10);
}

TEST(VmoManagerTest, WriteTooMuchDataToVmo) {
  VmoManager manager(kTestBurstSize);

  {
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(kTestBurstSize, 0, &vmo));

    uint32_t vmo_id = 0;
    const auto status =
        manager.RegisterVmos(fidl::VectorView<const uint32_t>::FromExternal(&vmo_id, 1),
                             fidl::VectorView<zx::vmo>::FromExternal(&vmo, 1));
    EXPECT_EQ(status, fuchsia_hardware_radar::wire::StatusCode::kSuccess);
  }

  uint8_t byte = 0;
  const fit::result vmo_id = manager.WriteUnlockedVmoAndGetId({&byte, kTestBurstSize + 1});
  ASSERT_TRUE(vmo_id.is_error());
  EXPECT_EQ(vmo_id.error_value(), fuchsia_hardware_radar::wire::StatusCode::kVmoTooSmall);
}

TEST(VmoManagerTest, RegistrationVectorInterface) {
  VmoManager manager(kTestBurstSize);

  std::vector<uint32_t> vmo_ids;

  {
    std::vector<zx::vmo> vmos;
    for (uint32_t i = 0; i < 10; i++) {
      zx::vmo::create(kTestBurstSize, 0, &vmos.emplace_back());
      ASSERT_TRUE(vmos.back());
      vmo_ids.emplace_back(i + 1);
    }

    const auto result = manager.RegisterVmos(vmo_ids, std::move(vmos));
    EXPECT_TRUE(result.is_ok());
  }

  {
    const auto result = manager.UnregisterVmos(vmo_ids);
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(result->size(), 10);
  }
}

}  // namespace radar
