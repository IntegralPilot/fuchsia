// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Used to test mvm/phy-ctxt.c

#include <zircon/compiler.h>

#include <gtest/gtest.h>

#include "third_party/iwlwifi/test/single-ap-test.h"

extern "C" {
#include "third_party/iwlwifi/mvm/mvm.h"
}

namespace wlan {
namespace testing {
namespace {

class PhyContextTest : public SingleApTest {
 public:
  PhyContextTest() __TA_NO_THREAD_SAFETY_ANALYSIS {
    mvm_ = iwl_trans_get_mvm(sim_trans_.iwl_trans());
    mtx_lock(&mvm_->mutex);
  }
  ~PhyContextTest() __TA_NO_THREAD_SAFETY_ANALYSIS { mtx_unlock(&mvm_->mutex); }

 protected:
  struct iwl_mvm* mvm_;
};

TEST_F(PhyContextTest, GetControlPosition) {
  struct ieee80211_channel chan = {
      .band = NL80211_BAND_2GHZ,
      .ch_num = 6,
      .hw_value = 6,
  };
  struct cfg80211_chan_def chandef = {
      .chan = &chan,
      .width = NL80211_CHAN_WIDTH_20_NOHT,
  };

  // Invalid channels. Expect the default value.
  chandef.width = NL80211_CHAN_WIDTH_20_NOHT;
  chan.ch_num = chan.hw_value = 0;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 34;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 68;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 96;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 165;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 255;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);

  // 2.4GHz channels. Expect the default value.
  chandef.width = NL80211_CHAN_WIDTH_20_NOHT;
  chan.ch_num = chan.hw_value = 1;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 14;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);

  // 5GHz channels. But different bandwitdh.

  // 20Mhz primary channels. Expect the default value.
  chandef.width = NL80211_CHAN_WIDTH_20_NOHT;
  chan.ch_num = chan.hw_value = 36;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 64;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 100;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 128;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 132;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 149;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 157;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 161;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);

  // HT40 primary channels.
  chandef.width = NL80211_CHAN_WIDTH_40;
  chan.ch_num = chan.hw_value = 36;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 40;  // not allowed
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 44;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 48;  // not allowed
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  // Try channel 149~161 group.
  chan.ch_num = chan.hw_value = 149;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 161;  // not allowed
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);

  // HT40 primary channels.
  chandef.width = NL80211_CHAN_WIDTH_40_BELOW;
  chan.ch_num = chan.hw_value = 36;  // invalid case since secondary cannot be below 36.
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 40;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_ABOVE);
  chan.ch_num = chan.hw_value = 44;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 48;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_ABOVE);
  chan.ch_num = chan.hw_value = 52;  // invalid case since channel 52 cannot do HT40-.
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  // Try channel 100~128 group.
  chan.ch_num = chan.hw_value = 100;  // invalid case since channel 100 cannot do HT40-.
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 128;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_ABOVE);

  // 80Mhz primary channels.
  chandef.width = NL80211_CHAN_WIDTH_80;
  chan.ch_num = chan.hw_value = 36;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_2_BELOW);
  chan.ch_num = chan.hw_value = 40;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 44;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_ABOVE);
  chan.ch_num = chan.hw_value = 48;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_2_ABOVE);

  // 160Mhz primary channels.
  chandef.width = NL80211_CHAN_WIDTH_160;
  chan.ch_num = chan.hw_value = 36;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_4_BELOW);
  chan.ch_num = chan.hw_value = 40;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_3_BELOW);
  chan.ch_num = chan.hw_value = 44;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_2_BELOW);
  chan.ch_num = chan.hw_value = 48;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 52;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_ABOVE);
  chan.ch_num = chan.hw_value = 56;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_2_ABOVE);
  chan.ch_num = chan.hw_value = 60;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_3_ABOVE);
  chan.ch_num = chan.hw_value = 64;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_4_ABOVE);
  // channel 132+ doesn't support 160Mhz channel. Use default value.
  chan.ch_num = chan.hw_value = 140;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
  chan.ch_num = chan.hw_value = 153;
  EXPECT_EQ(iwl_mvm_get_ctrl_pos(&chandef), PHY_VHT_CTRL_POS_1_BELOW);
}

TEST_F(PhyContextTest, Ref) {
  struct ieee80211_channel channel = {
      .band = NL80211_BAND_2GHZ,
      .ch_num = 5,
      .hw_value = 5,
  };

  struct iwl_mvm_phy_ctxt ctxt = {
      .width = NL80211_CHAN_WIDTH_40,
      .channel = &channel,
  };

  iwl_mvm_phy_ctxt_ref(mvm_, &ctxt);
  ASSERT_EQ(1, ctxt.ref);

  iwl_mvm_phy_ctxt_ref(mvm_, &ctxt);
  ASSERT_EQ(2, ctxt.ref);

  ASSERT_EQ(ZX_OK, iwl_mvm_phy_ctxt_unref(mvm_, &ctxt));
  ASSERT_EQ(1, ctxt.ref);
  ASSERT_EQ(5, ctxt.channel->hw_value);

  // Once the ref count goes to 0, it will be reset back to default value.
  ASSERT_EQ(ZX_OK, iwl_mvm_phy_ctxt_unref(mvm_, &ctxt));
  ASSERT_EQ(0, ctxt.ref);
  ASSERT_EQ(1, ctxt.channel->hw_value);
  ASSERT_EQ(NL80211_CHAN_WIDTH_20_NOHT, ctxt.width);
}

TEST_F(PhyContextTest, Changed) {
  struct ieee80211_channel old_chan = {
      .band = NL80211_BAND_2GHZ,
      .ch_num = 5,
      .hw_value = 5,
  };
  struct iwl_mvm_phy_ctxt old_ctxt = {
      .width = NL80211_CHAN_WIDTH_40,
      .channel = &old_chan,
  };

  struct ieee80211_channel new_chan = {
      .band = NL80211_BAND_2GHZ,
      .ch_num = 14,
      .hw_value = 14,
  };
  struct cfg80211_chan_def new_def = {
      .chan = &new_chan,
      .width = NL80211_CHAN_WIDTH_80,
  };

  ASSERT_EQ(ZX_OK, iwl_mvm_phy_ctxt_changed(mvm_, &old_ctxt, &new_def, 1, 1));
  EXPECT_EQ(14, old_ctxt.channel->hw_value);
  EXPECT_EQ(CHANNEL_BANDWIDTH_CBW80, old_ctxt.width);
}

}  // namespace
}  // namespace testing
}  // namespace wlan
