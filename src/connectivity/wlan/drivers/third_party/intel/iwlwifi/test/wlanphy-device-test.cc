// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// To test PHY device callback functions.

#include <lib/async/cpp/task.h>
#include <lib/fdio/directory.h>
#include <lib/sync/cpp/completion.h>
#include <zircon/listnode.h>
#include <zircon/syscalls.h>

#include <iterator>

#include <gtest/gtest.h>

#include "third_party/iwlwifi/platform/banjo/common.h"

extern "C" {
#include "third_party/iwlwifi/mvm/mvm.h"
}

#include "third_party/iwlwifi/platform/ieee80211.h"
#include "third_party/iwlwifi/platform/mvm-mlme.h"
#include "third_party/iwlwifi/platform/wlanphyimpl-device.h"
#include "third_party/iwlwifi/test/fake-ucode-test.h"

namespace wlan::testing {
namespace {

constexpr size_t kListenInterval = 100;
constexpr zx_handle_t kDummyMlmeChannel = 73939133;  // An arbitrary value not ZX_HANDLE_INVALID

class WlanPhyDeviceTest : public FakeUcodeTest {
 public:
  WlanPhyDeviceTest()
      : FakeUcodeTest({IWL_UCODE_TLV_CAPA_LAR_SUPPORT}, {IWL_UCODE_TLV_API_WIFI_MCC_UPDATE}),
        mvmvif_sta_{
            .mvm = iwl_trans_get_mvm(sim_trans_.iwl_trans()),
            .mac_role = WLAN_MAC_ROLE_CLIENT,
            .bss_conf =
                {
                    .beacon_int = kListenInterval,
                },
        } {
    sim_driver_ = sim_trans_.sim_driver();

    auto endpoints = fidl::CreateEndpoints<fuchsia_wlan_phy::WlanPhy>();
    EXPECT_FALSE(endpoints.is_error());

    client_ = fidl::SyncClient<fuchsia_wlan_phy::WlanPhy>(std::move(endpoints->client));

    libsync::Completion connected;
    async::PostTask(sim_trans_.async_driver_dispatcher(), [&]() {
      sim_driver_->ServiceConnectHandler(sim_trans_.async_driver_dispatcher(),
                                         std::move(endpoints->server));
      connected.Signal();
    });
    connected.Wait();
  }

  ~WlanPhyDeviceTest() = default;

  zx::result<uint16_t> CreateIface(fuchsia_wlan_common::WlanMacRole role) {
    zx::channel local, remote;
    zx::channel::create(0, &local, &remote);
    auto result = client_->CreateIface({{.role = role, .mlme_channel = std::move(local)}});
    if (result.is_error()) {
      if (result.error_value().is_domain_error()) {
        return zx::error(result.error_value().domain_error());
      }
      return zx::error(result.error_value().framework_error().status());
    }
    return zx::ok(result.value().iface_id().value());
  }

  zx::result<> DestroyIface(uint16_t iface_id) {
    auto result = client_->DestroyIface({{.iface_id = iface_id}});
    if (result.is_error()) {
      if (result.error_value().is_domain_error()) {
        return zx::error(result.error_value().domain_error());
      }
      return zx::error(result.error_value().framework_error().status());
    }
    return zx::ok();
  }

 protected:
  struct iwl_mvm_vif mvmvif_sta_;  // The mvm_vif settings for station role.
  wlan::iwlwifi::SimTransIwlwifiDriver* sim_driver_;

  fidl::SyncClient<fuchsia_wlan_phy::WlanPhy> client_;
  libsync::Completion completion_;
};

/////////////////////////////////////       PHY       //////////////////////////////////////////////

TEST_F(WlanPhyDeviceTest, GetSupportedMacRoles) {
  auto result = client_->GetSupportedMacRoles();
  ASSERT_TRUE(result.is_ok());
  ASSERT_TRUE(result->supported_mac_roles().has_value());
  EXPECT_EQ(result->supported_mac_roles()->size(), 1);
  EXPECT_EQ(result->supported_mac_roles()->data()[0],
            fuchsia_wlan_common::WlanMacRole::kClient);
}

TEST_F(WlanPhyDeviceTest, PartialCreateCleanup) {
  wlan_phy_impl_create_iface_req_t req = {
      .role = WLAN_MAC_ROLE_CLIENT,
      .mlme_channel = kDummyMlmeChannel,
  };
  uint16_t iface_id;
  struct iwl_trans* iwl_trans = sim_trans_.iwl_trans();

  // Test input null pointers
  ASSERT_EQ(ZX_OK, phy_create_iface(iwl_trans, &req, &iface_id));

  // Ensure mvmvif got created and indexed.
  struct iwl_mvm* mvm = iwl_trans_get_mvm(iwl_trans);
  ASSERT_NE(nullptr, mvm->mvmvif[iface_id]);

  // Ensure partial create failure removes it from the index.
  phy_create_iface_undo(iwl_trans, iface_id);
  ASSERT_EQ(nullptr, mvm->mvmvif[iface_id]);
}

TEST_F(WlanPhyDeviceTest, CreateIfaceNegativeTest) {
  static fidl::Arena arena;

  // Both role and channel not populated.
  {
    auto result = client_->CreateIface({});
    ASSERT_TRUE(result.is_error() && result.error_value().is_domain_error());
    EXPECT_EQ(ZX_ERR_INVALID_ARGS, result.error_value().domain_error());
  }

  // Role is set, but not channel.
  {
    auto result = client_->CreateIface({{.role = fuchsia_wlan_common::WlanMacRole::kClient}});
    ASSERT_TRUE(result.is_error() && result.error_value().is_domain_error());
    ASSERT_EQ(ZX_ERR_INVALID_ARGS, result.error_value().domain_error());
  }

  // Channel is set, but not the role.
  {
    zx::channel local, remote;
    zx::channel::create(0, &local, &remote);
    auto result = client_->CreateIface({{.mlme_channel = std::move(local)}});
    EXPECT_TRUE(result.is_error() && result.error_value().is_domain_error());
    ASSERT_EQ(ZX_ERR_INVALID_ARGS, result.error_value().domain_error());
  }
}

TEST_F(WlanPhyDeviceTest, DestroyIfaceNegativeTest) {
  static fidl::Arena arena;

  // iface_id not populated.
  auto result = client_->DestroyIface({});
  ASSERT_TRUE(result.is_error() && result.error_value().is_domain_error());
  ASSERT_EQ(ZX_ERR_INVALID_ARGS, result.error_value().domain_error());
}

TEST_F(WlanPhyDeviceTest, CreateDestroySingleInterface) {
  uint16_t iface_id;

  // Test invalid inputs
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, DestroyIface(MAX_NUM_MVMVIF).status_value());
  EXPECT_EQ(ZX_ERR_NOT_FOUND, DestroyIface(0).status_value());  // hasn't been added yet.

  // To verify the internal state of MVM driver.
  struct iwl_mvm* mvm = iwl_trans_get_mvm(sim_trans_.iwl_trans());

  // Add interface
  zx::result<uint16_t> result = CreateIface(fuchsia_wlan_common::WlanMacRole::kClient);
  ASSERT_TRUE(result.is_ok());
  iface_id = result.value();
  ASSERT_EQ(iface_id, 0);  // the first interface should have id 0.
  struct iwl_mvm_vif* mvmvif = mvm->mvmvif[0];
  ASSERT_NE(mvmvif, nullptr);
  ASSERT_EQ(mvmvif->mac_role,
            static_cast<wlan_mac_role_t>(fuchsia_wlan_common::WlanMacRole::kClient));
  // Count includes phy device in addition to the newly created mac device.
  ASSERT_EQ(sim_driver_->DeviceCount(), 1);

  // Remove interface
  EXPECT_EQ(ZX_OK, DestroyIface(0).status_value());
  ASSERT_EQ(mvm->mvmvif[0], nullptr);
  ASSERT_EQ(sim_driver_->DeviceCount(), 0);
}

TEST_F(WlanPhyDeviceTest, CreateDestroyMultipleInterfaces) {
  struct iwl_trans* iwl_trans = sim_trans_.iwl_trans();
  struct iwl_mvm* mvm = iwl_trans_get_mvm(iwl_trans);  // To verify the internal state of MVM
  uint16_t iface_id;

  // Add 1st interface
  zx::result<uint16_t> result = CreateIface(fuchsia_wlan_common::WlanMacRole::kClient);
  ASSERT_TRUE(result.is_ok());
  iface_id = result.value();
  ASSERT_EQ(iface_id, 0);
  ASSERT_NE(mvm->mvmvif[0], nullptr);
  ASSERT_EQ(mvm->mvmvif[0]->mac_role, WLAN_MAC_ROLE_CLIENT);
  ASSERT_EQ(sim_driver_->DeviceCount(), 1);

  // Add 2nd interface
  result = CreateIface(fuchsia_wlan_common::WlanMacRole::kClient);
  ASSERT_TRUE(result.is_ok());
  iface_id = result.value();
  ASSERT_EQ(iface_id, 1);
  ASSERT_NE(mvm->mvmvif[1], nullptr);
  ASSERT_EQ(mvm->mvmvif[1]->mac_role, WLAN_MAC_ROLE_CLIENT);
  ASSERT_EQ(sim_driver_->DeviceCount(), 2);

  // Add 3rd interface
  result = CreateIface(fuchsia_wlan_common::WlanMacRole::kClient);
  ASSERT_TRUE(result.is_ok());
  iface_id = result.value();
  ASSERT_EQ(iface_id, 2);
  ASSERT_NE(mvm->mvmvif[2], nullptr);
  ASSERT_EQ(mvm->mvmvif[2]->mac_role, WLAN_MAC_ROLE_CLIENT);
  ASSERT_EQ(sim_driver_->DeviceCount(), 3);

  // Remove the 2nd interface
  EXPECT_EQ(ZX_OK, DestroyIface(1).status_value());
  ASSERT_EQ(mvm->mvmvif[1], nullptr);
  ASSERT_EQ(sim_driver_->DeviceCount(), 2);

  // Add a new interface and it should be the 2nd one.
  result = CreateIface(fuchsia_wlan_common::WlanMacRole::kClient);
  ASSERT_TRUE(result.is_ok());
  iface_id = result.value();
  ASSERT_EQ(iface_id, 1);
  ASSERT_NE(mvm->mvmvif[1], nullptr);
  ASSERT_EQ(mvm->mvmvif[1]->mac_role, WLAN_MAC_ROLE_CLIENT);
  ASSERT_EQ(sim_driver_->DeviceCount(), 3);

  // Add 4th interface
  result = CreateIface(fuchsia_wlan_common::WlanMacRole::kClient);
  ASSERT_TRUE(result.is_ok());
  iface_id = result.value();
  ASSERT_EQ(iface_id, 3);
  ASSERT_NE(mvm->mvmvif[3], nullptr);
  ASSERT_EQ(mvm->mvmvif[3]->mac_role, WLAN_MAC_ROLE_CLIENT);
  ASSERT_EQ(sim_driver_->DeviceCount(), 4);

  // Add 5th interface and it should fail
  EXPECT_EQ(ZX_ERR_NO_RESOURCES,
            CreateIface(fuchsia_wlan_common::WlanMacRole::kClient).status_value());
  ASSERT_EQ(sim_driver_->DeviceCount(), 4);

  // Remove the 2nd interface
  EXPECT_EQ(ZX_OK, DestroyIface(1).status_value());
  ASSERT_EQ(mvm->mvmvif[1], nullptr);
  ASSERT_EQ(sim_driver_->DeviceCount(), 3);

  // Remove the 3rd interface
  EXPECT_EQ(ZX_OK, DestroyIface(2).status_value());
  ASSERT_EQ(mvm->mvmvif[2], nullptr);
  ASSERT_EQ(sim_driver_->DeviceCount(), 2);

  // Remove the 4th interface
  EXPECT_EQ(ZX_OK, DestroyIface(3).status_value());
  ASSERT_EQ(mvm->mvmvif[3], nullptr);
  ASSERT_EQ(sim_driver_->DeviceCount(), 1);

  // Remove the 1st interface
  EXPECT_EQ(ZX_OK, DestroyIface(0).status_value());
  ASSERT_EQ(mvm->mvmvif[0], nullptr);
  ASSERT_EQ(sim_driver_->DeviceCount(), 0);

  // Remove the 1st interface again and it should fail.
  EXPECT_EQ(ZX_ERR_NOT_FOUND, DestroyIface(0).status_value());
  ASSERT_EQ(sim_driver_->DeviceCount(), 0);
}

TEST_F(WlanPhyDeviceTest, GetCountry) {
  auto result = client_->GetCountry();
  ASSERT_TRUE(result.is_ok());
  auto& country = result.value();
  EXPECT_EQ('Z', country.country()[0]);
  EXPECT_EQ('Z', country.country()[1]);
}

TEST_F(WlanPhyDeviceTest, SetCountry) {
  std::array<uint8_t, 2> country = {'U', 'S'};
  auto result = client_->SetCountry({country});
  ASSERT_TRUE(result.is_error() && result.error_value().is_domain_error());
  ASSERT_EQ(ZX_ERR_NOT_SUPPORTED, result.error_value().domain_error());
}

TEST_F(WlanPhyDeviceTest, ClearCountry) {
  auto result = client_->ClearCountry();
  ASSERT_TRUE(result.is_error() && result.error_value().is_domain_error());
  ASSERT_EQ(ZX_ERR_NOT_SUPPORTED, result.error_value().domain_error());
}

TEST_F(WlanPhyDeviceTest, SetPowerSaveMode) {
  auto result = client_->SetPowerSaveMode({});
  ASSERT_TRUE(result.is_error() && result.error_value().is_domain_error());
  ASSERT_EQ(ZX_ERR_NOT_SUPPORTED, result.error_value().domain_error());
}

TEST_F(WlanPhyDeviceTest, GetPowerSaveMode) {
  auto result = client_->GetPowerSaveMode();
  ASSERT_TRUE(result.is_error() && result.error_value().is_domain_error());
  ASSERT_EQ(ZX_ERR_NOT_SUPPORTED, result.error_value().domain_error());
}

}  // namespace
}  // namespace wlan::testing
