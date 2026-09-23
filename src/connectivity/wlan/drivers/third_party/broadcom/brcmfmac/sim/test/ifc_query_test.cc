// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include <zircon/errors.h>

#include <wlan/drivers/macaddr.h>
#include <zxtest/zxtest.h>

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/sim/test/sim_test.h"
#include "src/connectivity/wlan/lib/common/cpp/include/wlan/common/element.h"

using ::wlan::common::MacAddr;

namespace wlan::brcmfmac {
namespace {
const MacAddr kDefaultMac({0x12, 0x34, 0x56, 0x65, 0x43, 0x21});
constexpr zx::duration kSimulatedClockDuration = zx::sec(10);
}  // namespace

class QueryTest : public SimTest {
 public:
  void SetUp() override {
    ASSERT_EQ(Init(), ZX_OK);

    ASSERT_EQ(StartInterface(wlan_common::WlanMacRole::kClient, &client_ifc_, kDefaultMac), ZX_OK);
  }

  fuchsia_wlan_fullmac::WlanFullmacImplQueryResponse Query() {
    fuchsia_wlan_fullmac::WlanFullmacImplQueryResponse ifc_query_result;
    env_->ScheduleNotification([&]() { ifc_query_result = client_ifc_.Query(); }, zx::sec(1));

    env_->Run(kSimulatedClockDuration);
    return ifc_query_result;
  }

  void SetCapabilityIovars(SimFirmware::CapabilityIovars& iovars) {
    WithSimDevice([&](brcmfmac::SimDevice* sim_device) {
      std::unique_ptr<SimFirmware>& sim_fw = sim_device->GetSim()->sim_fw;
      sim_fw->SetCapabilityIovars(iovars);
    });
  }

  void AssertBandCapsSize(
      std::optional<std::vector<fuchsia_wlan_fullmac::BandCapability>>& band_caps) {
    ASSERT_TRUE(band_caps.has_value());
    // Number of bands shouldn't exceed the maximum allowable
    ASSERT_LE(band_caps->size(), (size_t)wlan_common::kMaxBands);
    ASSERT_GT(band_caps->size(), 0);
  }

  SimInterface client_ifc_;
};

// Verify that a query operation works on a client interface
TEST_F(QueryTest, CheckRoleAndAddr) {
  fuchsia_wlan_fullmac::WlanFullmacImplQueryResponse ifc_query_result = Query();

  // Mac address returned should match the one we specified when we created the interface
  static_assert(wlan_ieee80211::kMacAddrLen == common::kMacAddrLen);
  EXPECT_BYTES_EQ(kDefaultMac.byte, ifc_query_result.sta_addr()->data(),
                  wlan_ieee80211::kMacAddrLen);

  EXPECT_EQ(ifc_query_result.role(), wlan_common::WlanMacRole::kClient);
}

TEST_F(QueryTest, CheckBasicRates) {
  fuchsia_wlan_fullmac::WlanFullmacImplQueryResponse ifc_query_result = Query();
  AssertBandCapsSize(ifc_query_result.band_caps());

  for (auto& band_cap : *ifc_query_result.band_caps()) {
    // Band id should be in valid range
    ASSERT_TRUE(band_cap.band() == wlan_ieee80211::WlanBand::kTwoGhz ||
                band_cap.band() == wlan_ieee80211::WlanBand::kFiveGhz);

    if (band_cap.band() == wlan_ieee80211::WlanBand::kTwoGhz) {
      constexpr uint8_t kExpectedBasic2gRates[] = {2, 4, 11, 22, 12, 18, 24, 36, 48, 72, 96, 108};
      ASSERT_EQ(band_cap.basic_rates()->size(), std::size(kExpectedBasic2gRates));
      EXPECT_BYTES_EQ(band_cap.basic_rates()->data(), kExpectedBasic2gRates,
                      band_cap.basic_rates()->size());
    } else {
      constexpr uint8_t kExpectedBasic5gRates[] = {12, 18, 24, 36, 48, 72, 96, 108};
      ASSERT_EQ(band_cap.basic_rates()->size(), std::size(kExpectedBasic5gRates));
      EXPECT_BYTES_EQ(band_cap.basic_rates()->data(), kExpectedBasic5gRates,
                      band_cap.basic_rates()->size());
    }
  }
}

TEST_F(QueryTest, CheckDefaultHtCaps) {
  fuchsia_wlan_fullmac::WlanFullmacImplQueryResponse ifc_query_result = Query();
  AssertBandCapsSize(ifc_query_result.band_caps());
  bool checked_2ghz_band = false;
  bool checked_5ghz_band = false;

  for (auto& band_cap : *ifc_query_result.band_caps()) {
    ASSERT_TRUE(band_cap.band() == wlan_ieee80211::WlanBand::kTwoGhz ||
                band_cap.band() == wlan_ieee80211::WlanBand::kFiveGhz);

    ASSERT_TRUE(band_cap.ht_caps().has_value());

    wlan::HtCapabilities* ht_caps =
        wlan::HtCapabilities::ViewFromRawBytes(band_cap.ht_caps()->bytes().data());

    // From the ldpc_cap iovar. Supported by default.
    EXPECT_TRUE(ht_caps->ht_cap_info.ldpc_coding_cap());

    // Always set
    EXPECT_TRUE(ht_caps->ht_cap_info.short_gi_20());
    EXPECT_TRUE(ht_caps->ht_cap_info.dsss_in_40());

    // TODO(https://fxbug.dev/382112237): brcmf_update_ht_cap actually sets this to
    // IEEE80211_HT_CAPS_SMPS_DISABLED, which has a value of 0xc.
    EXPECT_EQ(ht_caps->ht_cap_info.sm_power_save(), 0);

    // From the stbc_rx iovar.
    EXPECT_EQ(ht_caps->ht_cap_info.rx_stbc(), SimFirmware::kDefaultStbcRx);
    EXPECT_TRUE(ht_caps->ht_cap_info.tx_stbc());

    // From ampdu_rx_density iovar.
    EXPECT_EQ(ht_caps->ampdu_params.min_start_spacing(), SimFirmware::kDefaultAmpduRxDensity);

    // From ampdu_rx_factor iovar.
    // brcmfmac caps the exponent to 3.
    EXPECT_EQ(ht_caps->ampdu_params.exponent(), std::min(3u, SimFirmware::kDefaultAmpduRxFactor));

    if (band_cap.band() == wlan_ieee80211::WlanBand::kFiveGhz) {
      EXPECT_TRUE(ht_caps->ht_cap_info.chan_width_set());
      EXPECT_TRUE(ht_caps->ht_cap_info.short_gi_40());
      checked_5ghz_band = true;
    } else {
      EXPECT_FALSE(ht_caps->ht_cap_info.chan_width_set());
      EXPECT_FALSE(ht_caps->ht_cap_info.short_gi_40());
      checked_2ghz_band = true;
    }
  }

  EXPECT_TRUE(checked_2ghz_band);
  EXPECT_TRUE(checked_5ghz_band);
}

TEST_F(QueryTest, CheckDefaultVhtCaps) {
  fuchsia_wlan_fullmac::WlanFullmacImplQueryResponse ifc_query_result = Query();
  AssertBandCapsSize(ifc_query_result.band_caps());
  bool checked_2ghz_band = false;
  bool checked_5ghz_band = false;

  for (auto& band_cap : *ifc_query_result.band_caps()) {
    if (band_cap.band() == wlan_ieee80211::WlanBand::kTwoGhz) {
      // 2Ghz bands don't have VHT caps.
      EXPECT_FALSE(band_cap.vht_caps().has_value());
      checked_2ghz_band = true;
      continue;
    }

    ASSERT_EQ(band_cap.band(), wlan_ieee80211::WlanBand::kFiveGhz);
    ASSERT_TRUE(band_cap.vht_caps().has_value());

    checked_5ghz_band = true;
    // Check VHT caps for 5GHz band.
    wlan::VhtCapabilities* vht_caps =
        wlan::VhtCapabilities::ViewFromRawBytes(band_cap.vht_caps()->bytes().data());

    // TODO(https://fxbug.dev/42103822): Value hardcoded from firmware behavior of the BCM4356 and
    // BCM4359 chips.
    EXPECT_EQ(vht_caps->vht_cap_info.max_mpdu_len(), 2);

    EXPECT_TRUE(vht_caps->vht_cap_info.sgi_cbw80());
    EXPECT_EQ(vht_caps->vht_cap_info.supported_cbw_set(), 0);
    EXPECT_FALSE(vht_caps->vht_cap_info.sgi_cbw160());

    EXPECT_TRUE(vht_caps->vht_cap_info.rx_ldpc());

    // Note: only enabled for BCM4359 chip, but SimFirmware uses BCM4356.
    EXPECT_FALSE(vht_caps->vht_cap_info.tx_stbc());

    // nchain is the number of bits set in the rxstreams cap
    size_t nchain = std::popcount(SimFirmware::kDefaultRxStreamsCap);
    for (size_t ss_num = 1; ss_num <= 8; ss_num++) {
      if (ss_num <= nchain) {
        EXPECT_EQ(vht_caps->vht_mcs_nss.get_rx_max_mcs_ss(ss_num), IEEE80211_VHT_MCS_0_9);
        EXPECT_EQ(vht_caps->vht_mcs_nss.get_tx_max_mcs_ss(ss_num), IEEE80211_VHT_MCS_0_9);
      } else {
        EXPECT_EQ(vht_caps->vht_mcs_nss.get_rx_max_mcs_ss(ss_num), IEEE80211_VHT_MCS_NONE);
        EXPECT_EQ(vht_caps->vht_mcs_nss.get_tx_max_mcs_ss(ss_num), IEEE80211_VHT_MCS_NONE);
      }
    }

    // Note: value of 0 for max data rate means that the STA does not specify the max data rate it
    // is able to transmit or receive.
    EXPECT_EQ(vht_caps->vht_mcs_nss.rx_max_data_rate(), 0);
    EXPECT_EQ(vht_caps->vht_mcs_nss.tx_max_data_rate(), 0);

    EXPECT_EQ(vht_caps->vht_mcs_nss.max_nsts(), 0);
    EXPECT_EQ(vht_caps->vht_mcs_nss.ext_nss_bw(), 0);

    EXPECT_TRUE(vht_caps->vht_cap_info.su_bfee());
    EXPECT_FALSE(vht_caps->vht_cap_info.mu_bfee());
    EXPECT_TRUE(vht_caps->vht_cap_info.su_bfer());
    EXPECT_FALSE(vht_caps->vht_cap_info.mu_bfer());

    // These are hardcoded in cfg80211.cc
    EXPECT_EQ(vht_caps->vht_cap_info.bfee_sts(), 2);
    EXPECT_EQ(vht_caps->vht_cap_info.num_sounding(), SimFirmware::kDefaultTxStreamsCap - 1);
    EXPECT_EQ(vht_caps->vht_cap_info.link_adapt(), 3);

    EXPECT_EQ(vht_caps->vht_cap_info.max_ampdu_exp(), SimFirmware::kDefaultAmpduRxFactor);
  }

  EXPECT_TRUE(checked_2ghz_band);
  EXPECT_TRUE(checked_5ghz_band);
}

TEST_F(QueryTest, CheckCapsWithLdpcDisabled) {
  SimFirmware::CapabilityIovars iovars{};
  iovars.ldpc_cap = 0;
  SetCapabilityIovars(iovars);

  fuchsia_wlan_fullmac::WlanFullmacImplQueryResponse ifc_query_result = Query();
  AssertBandCapsSize(ifc_query_result.band_caps());

  for (auto& band_cap : *ifc_query_result.band_caps()) {
    ASSERT_TRUE(band_cap.ht_caps().has_value());
    wlan::HtCapabilities* ht_caps =
        wlan::HtCapabilities::ViewFromRawBytes(band_cap.ht_caps()->bytes().data());
    EXPECT_FALSE(ht_caps->ht_cap_info.ldpc_coding_cap());
    if (band_cap.vht_caps().has_value()) {
      wlan::VhtCapabilities* vht_caps =
          wlan::VhtCapabilities::ViewFromRawBytes(band_cap.vht_caps()->bytes().data());
      EXPECT_FALSE(vht_caps->vht_cap_info.rx_ldpc());
    }
  }
}

TEST_F(QueryTest, CheckCapsWithStbcRxDisabled) {
  SimFirmware::CapabilityIovars iovars{};
  iovars.stbc_rx = 0;
  SetCapabilityIovars(iovars);
  fuchsia_wlan_fullmac::WlanFullmacImplQueryResponse ifc_query_result = Query();
  AssertBandCapsSize(ifc_query_result.band_caps());

  for (auto& band_cap : *ifc_query_result.band_caps()) {
    ASSERT_TRUE(band_cap.ht_caps().has_value());
    wlan::HtCapabilities* ht_caps =
        wlan::HtCapabilities::ViewFromRawBytes(band_cap.ht_caps()->bytes().data());
    EXPECT_FALSE(ht_caps->ht_cap_info.rx_stbc());
    EXPECT_FALSE(ht_caps->ht_cap_info.tx_stbc());
  }
}

TEST_F(QueryTest, CheckCapsWithTxbfHwIovarsFallback) {
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    sim->sim_fw->err_inj_.AddErrInjIovar("txbf_bfe_cap_hw", ZX_OK, BCME_ERROR,
                                         client_ifc_.iface_id_, nullptr);
    sim->sim_fw->err_inj_.AddErrInjIovar("txbf_bfr_cap_hw", ZX_OK, BCME_ERROR,
                                         client_ifc_.iface_id_, nullptr);
  });

  fuchsia_wlan_fullmac::WlanFullmacImplQueryResponse ifc_query_result = Query();
  AssertBandCapsSize(ifc_query_result.band_caps());

  for (auto& band_cap : *ifc_query_result.band_caps()) {
    if (band_cap.band() == wlan_ieee80211::WlanBand::kFiveGhz) {
      ASSERT_TRUE(band_cap.vht_caps().has_value());
      wlan::VhtCapabilities* vht_caps =
          wlan::VhtCapabilities::ViewFromRawBytes(band_cap.vht_caps()->bytes().data());
      EXPECT_EQ(vht_caps->vht_cap_info.bfee_sts(), 2);
      EXPECT_EQ(vht_caps->vht_cap_info.num_sounding(), SimFirmware::kDefaultTxStreamsCap - 1);
      EXPECT_EQ(vht_caps->vht_cap_info.link_adapt(), 3);
    }
  }
}

TEST_F(QueryTest, CheckOperatingChannels) {
  constexpr uint8_t kExpected2gChannel = 7;
  constexpr uint8_t kExpected5gChannel = 32;
  bool checked_2ghz_band = false;
  bool checked_5ghz_band = false;

  SimFirmware::CapabilityIovars iovars{};
  // Note: this chanspec format is specific to SimFirmware::kIoType.
  // This also contains duplicates to verify that the driver filters out duplicate channels.
  ASSERT_TRUE(iovars
                  .SetChanspecList({
                      // 2Ghz channel
                      BRCMU_CHSPEC_D11AC_BND_2G | BRCMU_CHSPEC_D11AC_BW_20 |
                          (kExpected2gChannel & BRCMU_CHSPEC_CH_MASK),
                      BRCMU_CHSPEC_D11AC_BND_2G | BRCMU_CHSPEC_D11AC_BW_20 |
                          (kExpected2gChannel & BRCMU_CHSPEC_CH_MASK),

                      // 5Ghz channel
                      BRCMU_CHSPEC_D11AC_BND_5G | BRCMU_CHSPEC_D11AC_BW_20 |
                          (kExpected5gChannel & BRCMU_CHSPEC_CH_MASK),
                      BRCMU_CHSPEC_D11AC_BND_5G | BRCMU_CHSPEC_D11AC_BW_20 |
                          (kExpected5gChannel & BRCMU_CHSPEC_CH_MASK),
                  })
                  .is_ok());
  SetCapabilityIovars(iovars);
  fuchsia_wlan_fullmac::WlanFullmacImplQueryResponse ifc_query_result = Query();
  AssertBandCapsSize(ifc_query_result.band_caps());

  for (auto& band_cap : *ifc_query_result.band_caps()) {
    if (band_cap.band() == wlan_ieee80211::WlanBand::kTwoGhz) {
      ASSERT_EQ(band_cap.primary_channels()->size(), 1);
      EXPECT_EQ(band_cap.primary_channels()->at(0).number(), kExpected2gChannel);
      checked_2ghz_band = true;
    } else {
      ASSERT_EQ(band_cap.primary_channels()->size(), 1);
      EXPECT_EQ(band_cap.primary_channels()->at(0).number(), kExpected5gChannel);
      checked_5ghz_band = true;
    }
  }

  EXPECT_TRUE(checked_2ghz_band);
  EXPECT_TRUE(checked_5ghz_band);
}

// Verify that we send up an empty list of primary channels. The platform expects that primary
// channels is always present.
TEST_F(QueryTest, CheckOperatingChannelsPresentIfEmpty) {
  SimFirmware::CapabilityIovars iovars{};
  ASSERT_TRUE(iovars.SetChanspecList({}).is_ok());
  SetCapabilityIovars(iovars);
  fuchsia_wlan_fullmac::WlanFullmacImplQueryResponse ifc_query_result = Query();
  AssertBandCapsSize(ifc_query_result.band_caps());

  bool checked_2ghz_band = false;
  bool checked_5ghz_band = false;
  for (auto& band_cap : *ifc_query_result.band_caps()) {
    if (band_cap.band() == wlan_ieee80211::WlanBand::kTwoGhz) {
      ASSERT_EQ(band_cap.primary_channels()->size(), 0);
      checked_2ghz_band = true;
    } else {
      ASSERT_EQ(band_cap.primary_channels()->size(), 0);
      checked_5ghz_band = true;
    }
  }

  EXPECT_TRUE(checked_2ghz_band);
  EXPECT_TRUE(checked_5ghz_band);
}

// Verify that we can retrieve interface attributes even if the nchain iovar value is too large
TEST_F(QueryTest, BadNchainIovar) {
  // This invalid value of rxchain data has the potential to overflow the driver's internal
  // data structures
  const std::vector<uint8_t> alt_rxchain_data = {0xff, 0xff, 0xff, 0xff};
  WithSimDevice([&](brcmfmac::SimDevice* device) {
    brcmf_simdev* sim = device->GetSim();
    sim->sim_fw->err_inj_.AddErrInjIovar("rxstreams_cap", ZX_OK, BCME_OK, client_ifc_.iface_id_,
                                         &alt_rxchain_data);
  });

  [[maybe_unused]] fuchsia_wlan_fullmac::WlanFullmacImplQueryResponse ifc_query_result = Query();
  // This test just verifies that we don't crash when the iovar is retrieved
}

}  // namespace wlan::brcmfmac
