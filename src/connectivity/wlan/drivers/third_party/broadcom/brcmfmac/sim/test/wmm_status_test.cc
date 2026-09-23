// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zxtest/zxtest.h>

#include "src/connectivity/wlan/drivers/testing/lib/sim-env/sim-env.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/sim.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"

namespace wlan::brcmfmac {

class WmmStatusInterface : public SimInterface {
 public:
  void OnWmmStatusResp(OnWmmStatusRespRequestView request,
                       OnWmmStatusRespCompleter::Sync& completer) override;

  bool on_wmm_status_resp_called_ = false;
};

class WmmStatusTest : public SimTest {
 public:
  WmmStatusTest() = default;
  void Init();

  WmmStatusInterface client_ifc_;
};

void WmmStatusTest::Init() {
  ASSERT_EQ(SimTest::Init(), ZX_OK);
  ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc_), ZX_OK);
}

void WmmStatusInterface::OnWmmStatusResp(OnWmmStatusRespRequestView request,
                                         OnWmmStatusRespCompleter::Sync& completer) {
  ASSERT_EQ(request->status, ZX_OK);
  auto* resp = &request->wmm_params;

  EXPECT_TRUE(resp->apsd);

  EXPECT_EQ(resp->ac_be_params.aifsn, 4);
  EXPECT_EQ(resp->ac_be_params.ecw_min, 5);
  EXPECT_EQ(resp->ac_be_params.ecw_max, 10);
  EXPECT_EQ(resp->ac_be_params.txop_limit, 0);
  EXPECT_FALSE(resp->ac_be_params.acm);

  EXPECT_EQ(resp->ac_bk_params.aifsn, 7);
  EXPECT_EQ(resp->ac_bk_params.ecw_min, 6);
  EXPECT_EQ(resp->ac_bk_params.ecw_max, 11);
  EXPECT_EQ(resp->ac_bk_params.txop_limit, 0);
  EXPECT_FALSE(resp->ac_bk_params.acm);

  EXPECT_EQ(resp->ac_vi_params.aifsn, 3);
  EXPECT_EQ(resp->ac_vi_params.ecw_min, 4);
  EXPECT_EQ(resp->ac_vi_params.ecw_max, 5);
  EXPECT_EQ(resp->ac_vi_params.txop_limit, 94);
  EXPECT_FALSE(resp->ac_vi_params.acm);

  EXPECT_EQ(resp->ac_vo_params.aifsn, 2);
  EXPECT_EQ(resp->ac_vo_params.ecw_min, 2);
  EXPECT_EQ(resp->ac_vo_params.ecw_max, 4);
  EXPECT_EQ(resp->ac_vo_params.txop_limit, 47);
  EXPECT_TRUE(resp->ac_vo_params.acm);

  on_wmm_status_resp_called_ = true;
  completer.Reply();
}

TEST_F(WmmStatusTest, WmmStatus) {
  Init();

  auto result = client_ifc_.client_.buffer(client_ifc_.test_arena_)->WmmStatusReq();
  EXPECT_TRUE(client_ifc_.on_wmm_status_resp_called_);
  EXPECT_OK(result);
}

}  // namespace wlan::brcmfmac
