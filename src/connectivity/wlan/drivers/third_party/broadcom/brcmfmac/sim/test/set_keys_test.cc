// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zxtest/zxtest.h>

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"
#include "zircon/errors.h"

namespace wlan::brcmfmac {
namespace {
fidl::Array<uint8_t, 3> kIeeeOui = {0x00, 0x0F, 0xAC};
fidl::Array<uint8_t, 3> kMsftOui = {0x00, 0x50, 0xF2};
}  // namespace

class SetKeysTest : public SimTest {
 public:
  SetKeysTest() = default;
  void SetUp() override {
    ASSERT_OK(SimTest::Init());
    ASSERT_OK(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc_));
  }
  void TearDown() override { EXPECT_OK(SimTest::DeleteInterface(&client_ifc_)); }

  SimInterface client_ifc_;
};

wlan_fullmac_wire::WlanFullmacImplSetKeysRequest FakeSetKeysRequest(
    const uint8_t keys[][wlan_ieee80211::kMaxKeyLen], size_t n, fidl::AnyArena& arena) {
  fidl::VectorView<fuchsia_wlan_ieee80211::wire::SetKeyDescriptor> key_descriptors(arena, n);

  for (size_t i = 0; i < n; i++) {
    key_descriptors[i] =
        fuchsia_wlan_ieee80211::wire::SetKeyDescriptor::Builder(arena)
            .key(fidl::VectorView<uint8_t>::FromExternal(
                const_cast<uint8_t*>(keys[i]), strlen(reinterpret_cast<const char*>(keys[i]))))
            .key_id(static_cast<uint8_t>(i))
            .key_type(fuchsia_wlan_ieee80211::wire::KeyType::kPairwise)
            .cipher_type(wlan_ieee80211::CipherSuiteType::kCcmp128)
            .cipher_oui(kIeeeOui)
            .rsc({})
            .peer_addr({})
            .Build();
  }
  auto builder = wlan_fullmac_wire::WlanFullmacImplSetKeysRequest::Builder(arena).key_descriptors(
      key_descriptors);
  return builder.Build();
}

TEST_F(SetKeysTest, MultipleKeys) {
  const uint8_t keys[wlan_fullmac_wire::kWlanMaxKeylistSize][wlan_ieee80211::kMaxKeyLen] = {
      "One", "Two", "Three", "Four"};
  wlan_fullmac_wire::WlanFullmacImplSetKeysRequest set_keys_req =
      FakeSetKeysRequest(keys, wlan_fullmac_wire::kWlanMaxKeylistSize, client_ifc_.test_arena_);
  auto result = client_ifc_.client_.buffer(client_ifc_.test_arena_)->SetKeys(set_keys_req);
  EXPECT_TRUE(result.ok());

  auto& set_keys_resp = result->resp;

  std::vector<brcmf_wsec_key_le> firmware_keys;
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    firmware_keys = device->GetSim()->sim_fw->GetKeyList(client_ifc_.iface_id_);
  });
  ASSERT_EQ(firmware_keys.size(), wlan_fullmac_wire::kWlanMaxKeylistSize);

  for (size_t i = 0; i < firmware_keys.size(); i++) {
    EXPECT_EQ(strlen(reinterpret_cast<const char*>(keys[i])), firmware_keys[i].len);
    EXPECT_BYTES_EQ(firmware_keys[i].data, keys[i], firmware_keys[i].len);
  }

  ASSERT_EQ(set_keys_resp.statuslist.size(), wlan_fullmac_wire::kWlanMaxKeylistSize);
  for (size_t i = 0; i < set_keys_resp.statuslist.size(); i++) {
    ASSERT_EQ(set_keys_resp.statuslist[i], ZX_OK);
  }
}

// Ensure that group key is set correctly by the driver in firmware.
TEST_F(SetKeysTest, SetGroupKey) {
  const uint8_t group_key[wlan_ieee80211::kMaxKeyLen] = "Group Key";
  const size_t group_keylen = strlen(reinterpret_cast<const char*>(group_key));
  const uint8_t ucast_key[wlan_ieee80211::kMaxKeyLen] = "My Key";
  const size_t ucast_keylen = strlen(reinterpret_cast<const char*>(ucast_key));

  fidl::VectorView<fuchsia_wlan_ieee80211::wire::SetKeyDescriptor> key_descriptors(
      client_ifc_.test_arena_, 2);
  key_descriptors[0] =
      fuchsia_wlan_ieee80211::wire::SetKeyDescriptor::Builder(client_ifc_.test_arena_)
          .key(fidl::VectorView<uint8_t>::FromExternal(const_cast<uint8_t*>(group_key),
                                                       group_keylen))
          .key_id(static_cast<uint8_t>(0))
          .key_type(fuchsia_wlan_ieee80211::wire::KeyType::kGroup)
          .cipher_type(wlan_ieee80211::CipherSuiteType::kCcmp128)
          .cipher_oui(kIeeeOui)
          .rsc({})
          .peer_addr({0xff, 0xff, 0xff, 0xff, 0xff, 0xff})
          .Build();

  key_descriptors[1] =
      fuchsia_wlan_ieee80211::wire::SetKeyDescriptor::Builder(client_ifc_.test_arena_)
          .key(fidl::VectorView<uint8_t>::FromExternal(const_cast<uint8_t*>(ucast_key),
                                                       ucast_keylen))
          .key_id(static_cast<uint8_t>(1))
          .key_type(fuchsia_wlan_ieee80211::wire::KeyType::kPairwise)
          .cipher_type(wlan_ieee80211::CipherSuiteType::kCcmp128)
          .cipher_oui(kIeeeOui)
          .rsc({})
          .peer_addr({0xde, 0xad, 0xbe, 0xef, 0xab, 0xcd})
          .Build();

  auto builder = wlan_fullmac_wire::WlanFullmacImplSetKeysRequest::Builder(client_ifc_.test_arena_)
                     .key_descriptors(key_descriptors);

  auto result = client_ifc_.client_.buffer(client_ifc_.test_arena_)->SetKeys(builder.Build());
  EXPECT_OK(result);

  auto& set_keys_resp = result->resp;
  ASSERT_EQ(set_keys_resp.statuslist.size(), 2ul);
  ASSERT_OK(set_keys_resp.statuslist.data()[0]);
  ASSERT_OK(set_keys_resp.statuslist.data()[1]);

  std::vector<brcmf_wsec_key_le> firmware_keys;
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    firmware_keys = device->GetSim()->sim_fw->GetKeyList(client_ifc_.iface_id_);
  });

  ASSERT_EQ(firmware_keys.size(), 2U);
  EXPECT_STREQ(reinterpret_cast<const char*>(firmware_keys[0].data),
               reinterpret_cast<const char*>(group_key));
  // Group key should have been set as the PRIMARY KEY
  ASSERT_EQ(firmware_keys[0].flags, (const unsigned int)BRCMF_PRIMARY_KEY);
  EXPECT_STREQ(reinterpret_cast<const char*>(firmware_keys[1].data),
               reinterpret_cast<const char*>(ucast_key));
  ASSERT_NE(firmware_keys[1].flags, (const unsigned int)BRCMF_PRIMARY_KEY);
}

TEST_F(SetKeysTest, CustomOuiNotSupported) {
  const uint8_t key[wlan_ieee80211::kMaxKeyLen] = "My Key";

  fidl::Array<uint8_t, 3> kCustomOui = {1, 2, 3};

  fidl::VectorView<fuchsia_wlan_ieee80211::wire::SetKeyDescriptor> key_descriptors(
      client_ifc_.test_arena_, 1);
  key_descriptors[0] =
      fuchsia_wlan_ieee80211::wire::SetKeyDescriptor::Builder(client_ifc_.test_arena_)
          .key(fidl::VectorView<uint8_t>::FromExternal(const_cast<uint8_t*>(key),
                                                       strlen(reinterpret_cast<const char*>(key))))
          .key_id(static_cast<uint8_t>(0))
          .key_type(fuchsia_wlan_ieee80211::wire::KeyType::kPairwise)
          .cipher_type(wlan_ieee80211::CipherSuiteType::kCcmp128)
          .cipher_oui(kCustomOui)
          .rsc({})
          .peer_addr({0xde, 0xad, 0xbe, 0xef, 0xab, 0xcd})
          .Build();

  auto builder = wlan_fullmac_wire::WlanFullmacImplSetKeysRequest::Builder(client_ifc_.test_arena_)
                     .key_descriptors(key_descriptors);

  auto result = client_ifc_.client_.buffer(client_ifc_.test_arena_)->SetKeys(builder.Build());
  EXPECT_OK(result);

  auto& set_keys_resp = result->resp;
  ASSERT_EQ(set_keys_resp.statuslist.size(), 1ul);
  ASSERT_EQ(set_keys_resp.statuslist.data()[0], ZX_ERR_NOT_SUPPORTED);

  std::vector<brcmf_wsec_key_le> firmware_keys;
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    firmware_keys = device->GetSim()->sim_fw->GetKeyList(client_ifc_.iface_id_);
  });

  ASSERT_EQ(firmware_keys.size(), 0U);
}

TEST_F(SetKeysTest, OptionalOuiSupported) {
  const uint8_t key[wlan_ieee80211::kMaxKeyLen] = "My Key";
  const size_t keylen = strlen(reinterpret_cast<const char*>(key));

  fidl::VectorView<fuchsia_wlan_ieee80211::wire::SetKeyDescriptor> key_descriptors(
      client_ifc_.test_arena_, 1);
  key_descriptors[0] =
      fuchsia_wlan_ieee80211::wire::SetKeyDescriptor::Builder(client_ifc_.test_arena_)
          .key(fidl::VectorView<uint8_t>::FromExternal(const_cast<uint8_t*>(key), keylen))
          .key_id(static_cast<uint8_t>(0))
          .key_type(fuchsia_wlan_ieee80211::wire::KeyType::kPairwise)
          .cipher_type(wlan_ieee80211::CipherSuiteType::kCcmp128)
          .rsc({})
          .peer_addr({0xde, 0xad, 0xbe, 0xef, 0xab, 0xcd})
          .Build();

  auto builder = wlan_fullmac_wire::WlanFullmacImplSetKeysRequest::Builder(client_ifc_.test_arena_)
                     .key_descriptors(key_descriptors);

  auto result = client_ifc_.client_.buffer(client_ifc_.test_arena_)->SetKeys(builder.Build());
  EXPECT_OK(result);

  auto& set_keys_resp = result->resp;
  ASSERT_EQ(set_keys_resp.statuslist.size(), 1ul);
  ASSERT_OK(set_keys_resp.statuslist.data()[0]);

  std::vector<brcmf_wsec_key_le> firmware_keys;
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    firmware_keys = device->GetSim()->sim_fw->GetKeyList(client_ifc_.iface_id_);
  });

  ASSERT_EQ(firmware_keys.size(), 1U);

  EXPECT_EQ(keylen, firmware_keys[0].len);
  EXPECT_BYTES_EQ(firmware_keys[0].data, key, firmware_keys[0].len);
}

TEST_F(SetKeysTest, MsftOuiSupported) {
  const uint8_t key[wlan_ieee80211::kMaxKeyLen] = "My Key";
  const size_t keylen = strlen(reinterpret_cast<const char*>(key));

  fidl::VectorView<fuchsia_wlan_ieee80211::wire::SetKeyDescriptor> key_descriptors(
      client_ifc_.test_arena_, 1);
  key_descriptors[0] =
      fuchsia_wlan_ieee80211::wire::SetKeyDescriptor::Builder(client_ifc_.test_arena_)
          .key(fidl::VectorView<uint8_t>::FromExternal(const_cast<uint8_t*>(key), keylen))
          .key_id(static_cast<uint8_t>(0))
          .key_type(fuchsia_wlan_ieee80211::wire::KeyType::kPairwise)
          .cipher_type(wlan_ieee80211::CipherSuiteType::kCcmp128)
          .cipher_oui(kMsftOui)
          .rsc({})
          .peer_addr({0xde, 0xad, 0xbe, 0xef, 0xab, 0xcd})
          .Build();

  auto builder = wlan_fullmac_wire::WlanFullmacImplSetKeysRequest::Builder(client_ifc_.test_arena_)
                     .key_descriptors(key_descriptors);

  auto result = client_ifc_.client_.buffer(client_ifc_.test_arena_)->SetKeys(builder.Build());
  EXPECT_OK(result);

  auto& set_keys_resp = result->resp;
  ASSERT_EQ(set_keys_resp.statuslist.size(), 1ul);
  ASSERT_OK(set_keys_resp.statuslist.data()[0]);

  std::vector<brcmf_wsec_key_le> firmware_keys;
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    firmware_keys = device->GetSim()->sim_fw->GetKeyList(client_ifc_.iface_id_);
  });

  ASSERT_EQ(firmware_keys.size(), 1U);

  EXPECT_EQ(keylen, firmware_keys[0].len);
  EXPECT_BYTES_EQ(firmware_keys[0].data, key, firmware_keys[0].len);
}

}  // namespace wlan::brcmfmac
