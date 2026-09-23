/*
 * Copyright (c) 2019 The Fuchsia Authors
 *
 * Permission to use, copy, modify, and/or distribute this software for any
 * purpose with or without fee is hereby granted, provided that the above
 * copyright notice and this permission notice appear in all copies.
 *
 * THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
 * WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
 * MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY
 * SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
 * WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION
 * OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN
 * CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
 */

#include <fidl/fuchsia.wlan.ieee80211/cpp/wire.h>

#include <array>

#include <gtest/gtest.h>

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/brcmu_d11.h"
#include "third_party/bcmdhd/crossdriver/bcmwifi_channels.h"

namespace {

static void verify_channel_to_chanspec(const fuchsia_wlan_ieee80211::wire::ChannelNumber& in_ch,
                                       fuchsia_wlan_ieee80211::wire::ChannelBandwidth cbw,
                                       const brcmu_chan& expected) {
  brcmu_d11inf d11_inf = {.io_type = BRCMU_D11AC_IOTYPE};
  brcmu_d11_attach(&d11_inf);

  auto result = channel_to_chanspec(&d11_inf, in_ch.number, in_ch.band, cbw);
  ASSERT_TRUE(result.is_ok());
  brcmu_chan actual = {.chspec = result.value()};
  d11_inf.decchspec(&actual);

  EXPECT_EQ(actual.chnum, expected.chnum);
  EXPECT_EQ(actual.band, expected.band);
  EXPECT_EQ(actual.bw, expected.bw);
  EXPECT_EQ(actual.sb, expected.sb);
}

TEST(ChannelConversion, ChannelToChanspec) {
  brcmu_chan out_ch;
  using fuchsia_wlan_ieee80211::wire::ChannelBandwidth;

  {
    // Try a simple 20 MHz channel in the 2.4 GHz band
    fuchsia_wlan_ieee80211::wire::ChannelNumber in_ch = {
        .band = fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz, .number = 11};
    out_ch = {
        .chnum = 11, .band = BRCMU_CHAN_BAND_2G, .bw = BRCMU_CHAN_BW_20, .sb = BRCMU_CHAN_SB_NONE};
    verify_channel_to_chanspec(in_ch, ChannelBandwidth::kCbw20, out_ch);
  }

  {
    // Try a 40+ MHz channel in the 5 GHz band
    fuchsia_wlan_ieee80211::wire::ChannelNumber in_ch = {
        .band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 44};
    out_ch = {
        .chnum = 46, .band = BRCMU_CHAN_BAND_5G, .bw = BRCMU_CHAN_BW_40, .sb = BRCMU_CHAN_SB_L};
    verify_channel_to_chanspec(in_ch, ChannelBandwidth::kCbw40, out_ch);
  }

  {
    // Try a 40- MHz channel in the 5 GHz band
    fuchsia_wlan_ieee80211::wire::ChannelNumber in_ch = {
        .band = fuchsia_wlan_ieee80211::wire::WlanBand::kFiveGhz, .number = 112};
    out_ch = {
        .chnum = 110, .band = BRCMU_CHAN_BAND_5G, .bw = BRCMU_CHAN_BW_40, .sb = BRCMU_CHAN_SB_U};
    verify_channel_to_chanspec(in_ch, ChannelBandwidth::kCbw40Below, out_ch);
  }
}

static void verify_chanspec_to_operating_channel(
    const brcmu_chan& in_ch, const fuchsia_wlan_ieee80211::wire::ChannelNumber& expected_channel,
    fuchsia_wlan_ieee80211::wire::ChannelBandwidth expected_cbw,
    const fuchsia_wlan_ieee80211::wire::ChannelNumber& expected_secondary80) {
  brcmu_d11inf d11_inf = {.io_type = BRCMU_D11AC_IOTYPE};
  brcmu_d11_attach(&d11_inf);

  brcmu_chan in_ch_temp = in_ch;
  d11_inf.encchspec(&in_ch_temp);

  auto actual_channel = chanspec_to_operating_channel_number(&d11_inf, in_ch_temp.chspec);
  auto actual_cbw = chanspec_to_channel_bandwidth(&d11_inf, in_ch_temp.chspec);
  auto actual_secondary80 = chanspec_to_secondary80(&d11_inf, in_ch_temp.chspec);

  EXPECT_EQ(actual_channel.number, expected_channel.number);
  EXPECT_EQ(actual_channel.band, expected_channel.band);
  EXPECT_EQ(actual_cbw, expected_cbw);
  EXPECT_EQ(actual_secondary80.number, expected_secondary80.number);
  EXPECT_EQ(actual_secondary80.band, expected_secondary80.band);
}

static void verify_chanspec_to_primary_channel(
    const brcmu_chan& in_ch, const fuchsia_wlan_ieee80211::wire::ChannelNumber& expected_channel) {
  brcmu_d11inf d11_inf = {.io_type = BRCMU_D11AC_IOTYPE};
  brcmu_d11_attach(&d11_inf);

  brcmu_chan in_ch_temp = in_ch;
  d11_inf.encchspec(&in_ch_temp);

  auto actual_channel = chanspec_to_primary_channel_number(&d11_inf, in_ch_temp.chspec);

  EXPECT_EQ(actual_channel.number, expected_channel.number);
  EXPECT_EQ(actual_channel.band, expected_channel.band);
}

TEST(ChannelConversion, ChanspecToOperatingChannel) {
  brcmu_chan in_ch;
  using fuchsia_wlan_ieee80211::wire::ChannelBandwidth;
  using fuchsia_wlan_ieee80211::wire::WlanBand;

  {
    // Try a simple 20 MHz channel in the 2.4 GHz band
    in_ch = {
        .chnum = 11, .band = BRCMU_CHAN_BAND_2G, .bw = BRCMU_CHAN_BW_20, .sb = BRCMU_CHAN_SB_NONE};
    fuchsia_wlan_ieee80211::wire::ChannelNumber out_ch = {.band = WlanBand::kTwoGhz, .number = 11};
    verify_chanspec_to_operating_channel(in_ch, out_ch, ChannelBandwidth::kCbw20,
                                         {.band = out_ch.band, .number = 0});
  }

  {
    // Try a 40+ MHz channel in the 2.4 GHz band (SB_L => secondary above)
    in_ch = {.chnum = 3, .band = BRCMU_CHAN_BAND_2G, .bw = BRCMU_CHAN_BW_40, .sb = BRCMU_CHAN_SB_L};
    fuchsia_wlan_ieee80211::wire::ChannelNumber out_ch = {.band = WlanBand::kTwoGhz, .number = 3};
    verify_chanspec_to_operating_channel(in_ch, out_ch, ChannelBandwidth::kCbw40,
                                         {.band = out_ch.band, .number = 0});
  }

  {
    // Try a 40- MHz channel in the 2.4 GHz band (SB_U => secondary below)
    in_ch = {.chnum = 9, .band = BRCMU_CHAN_BAND_2G, .bw = BRCMU_CHAN_BW_40, .sb = BRCMU_CHAN_SB_U};
    fuchsia_wlan_ieee80211::wire::ChannelNumber out_ch = {.band = WlanBand::kTwoGhz, .number = 9};
    verify_chanspec_to_operating_channel(in_ch, out_ch, ChannelBandwidth::kCbw40Below,
                                         {.band = out_ch.band, .number = 0});
  }

  {
    // Try a 40+ MHz channel in the 5 GHz band (SB_L => secondary above)
    in_ch = {
        .chnum = 46, .band = BRCMU_CHAN_BAND_5G, .bw = BRCMU_CHAN_BW_40, .sb = BRCMU_CHAN_SB_L};
    fuchsia_wlan_ieee80211::wire::ChannelNumber out_ch = {.band = WlanBand::kFiveGhz, .number = 46};
    verify_chanspec_to_operating_channel(in_ch, out_ch, ChannelBandwidth::kCbw40,
                                         {.band = out_ch.band, .number = 0});
  }

  {
    // Try a 40- MHz channel in the 5 GHz band (SB_U => secondary below)
    in_ch = {
        .chnum = 112, .band = BRCMU_CHAN_BAND_5G, .bw = BRCMU_CHAN_BW_40, .sb = BRCMU_CHAN_SB_U};
    fuchsia_wlan_ieee80211::wire::ChannelNumber out_ch = {.band = WlanBand::kFiveGhz,
                                                          .number = 112};
    verify_chanspec_to_operating_channel(in_ch, out_ch, ChannelBandwidth::kCbw40Below,
                                         {.band = out_ch.band, .number = 0});
  }
}

TEST(ChannelConversion, ChanspecToPrimaryChannel) {
  brcmu_chan in_ch;
  using fuchsia_wlan_ieee80211::wire::WlanBand;

  {
    // Try a simple 20 MHz channel in the 2.4 GHz band
    in_ch = {
        .chnum = 11, .band = BRCMU_CHAN_BAND_2G, .bw = BRCMU_CHAN_BW_20, .sb = BRCMU_CHAN_SB_NONE};
    fuchsia_wlan_ieee80211::wire::ChannelNumber out_ch = {.band = WlanBand::kTwoGhz, .number = 11};
    verify_chanspec_to_primary_channel(in_ch, out_ch);
  }

  {
    // Try a 40+ MHz channel in the 5 GHz band (center 46, SB Upper => control 48)
    in_ch = {
        .chnum = 46, .band = BRCMU_CHAN_BAND_5G, .bw = BRCMU_CHAN_BW_40, .sb = BRCMU_CHAN_SB_U};
    fuchsia_wlan_ieee80211::wire::ChannelNumber out_ch = {.band = WlanBand::kFiveGhz, .number = 48};
    verify_chanspec_to_primary_channel(in_ch, out_ch);
  }

  {
    // Try a 40- MHz channel in the 5 GHz band (center 110, SB Lower => control 108)
    in_ch = {
        .chnum = 110, .band = BRCMU_CHAN_BAND_5G, .bw = BRCMU_CHAN_BW_40, .sb = BRCMU_CHAN_SB_L};
    fuchsia_wlan_ieee80211::wire::ChannelNumber out_ch = {.band = WlanBand::kFiveGhz,
                                                          .number = 108};
    verify_chanspec_to_primary_channel(in_ch, out_ch);
  }
}

TEST(ChannelConversion, Override80P80) {
  using fuchsia_wlan_ieee80211::ChannelBandwidth;
  using fuchsia_wlan_ieee80211::WlanBand;

  const auto out_cbw =
      enforce_bandwidth_limitations(36, WlanBand::kFiveGhz, ChannelBandwidth::kCbw80P80);
  // Override should only change the bandwidth.
  EXPECT_EQ(out_cbw, ChannelBandwidth::kCbw20);
}

TEST(ChannelConversion, Override80P80IgnoresOtherBandwidths) {
  using fuchsia_wlan_ieee80211::ChannelBandwidth;
  using fuchsia_wlan_ieee80211::WlanBand;
  const std::array<ChannelBandwidth, 4> bandwidths{
      ChannelBandwidth::kCbw20, ChannelBandwidth::kCbw40, ChannelBandwidth::kCbw80,
      ChannelBandwidth::kCbw160};
  for (const auto& bandwidth : bandwidths) {
    const auto out_cbw = enforce_bandwidth_limitations(36, WlanBand::kFiveGhz, bandwidth);
    EXPECT_EQ(out_cbw, bandwidth);
  }
}

TEST(ChannelConversion, OverrideWideBandwidthForChannel165) {
  using fuchsia_wlan_ieee80211::ChannelBandwidth;
  using fuchsia_wlan_ieee80211::WlanBand;
  const std::array<ChannelBandwidth, 2> bandwidths{ChannelBandwidth::kCbw40,
                                                   ChannelBandwidth::kCbw80};

  for (const auto& bandwidth : bandwidths) {
    const auto out_cbw = enforce_bandwidth_limitations(165, WlanBand::kFiveGhz, bandwidth);
    EXPECT_EQ(out_cbw, ChannelBandwidth::kCbw20);
  }
}

TEST(ChannelConversion, OverrideWideBandwidthForChannel173) {
  using fuchsia_wlan_ieee80211::ChannelBandwidth;
  using fuchsia_wlan_ieee80211::WlanBand;
  const auto out_cbw =
      enforce_bandwidth_limitations(173, WlanBand::kFiveGhz, ChannelBandwidth::kCbw40);
  EXPECT_EQ(out_cbw, ChannelBandwidth::kCbw20);
}

static void verify_round_trip(const fuchsia_wlan_ieee80211::wire::ChannelNumber& in_channel,
                              fuchsia_wlan_ieee80211::wire::ChannelBandwidth in_cbw) {
  brcmu_d11inf d11_inf = {.io_type = BRCMU_D11AC_IOTYPE};
  brcmu_d11_attach(&d11_inf);

  auto result = channel_to_chanspec(&d11_inf, in_channel.number, in_channel.band, in_cbw);
  ASSERT_TRUE(result.is_ok());
  auto actual_channel = chanspec_to_primary_channel_number(&d11_inf, result.value());
  auto actual_cbw = chanspec_to_channel_bandwidth(&d11_inf, result.value());

  EXPECT_EQ(actual_channel.number, in_channel.number)
      << "Channel number mismatch for channel " << static_cast<int>(in_channel.number);
  EXPECT_EQ(actual_channel.band, in_channel.band)
      << "Band mismatch for channel " << static_cast<int>(in_channel.number);
  EXPECT_EQ(actual_cbw, in_cbw) << "Bandwidth mismatch for channel "
                                << static_cast<int>(in_channel.number);
}

TEST(ChannelConversion, Override2G40MHz) {
  using fuchsia_wlan_ieee80211::wire::ChannelBandwidth;
  using fuchsia_wlan_ieee80211::wire::WlanBand;

  // 2.4 GHz 40+ MHz (Cbw40) is overridden to 20 MHz
  // IEEE Std 802.11-2024 Table E-4 Operating Class 83
  for (uint8_t ch = 1; ch <= 9; ++ch) {
    brcmu_chan expected = {
        .chnum = ch, .band = BRCMU_CHAN_BAND_2G, .bw = BRCMU_CHAN_BW_20, .sb = BRCMU_CHAN_SB_NONE};
    verify_channel_to_chanspec({.band = WlanBand::kTwoGhz, .number = ch}, ChannelBandwidth::kCbw40,
                               expected);
  }

  // 2.4 GHz 40- MHz (Cbw40Below) is overridden to 20 MHz
  // IEEE Std 802.11-2024 Table E-4 Operating Class 84
  for (uint8_t ch = 5; ch <= 13; ++ch) {
    brcmu_chan expected = {
        .chnum = ch, .band = BRCMU_CHAN_BAND_2G, .bw = BRCMU_CHAN_BW_20, .sb = BRCMU_CHAN_SB_NONE};
    verify_channel_to_chanspec({.band = WlanBand::kTwoGhz, .number = ch},
                               ChannelBandwidth::kCbw40Below, expected);
  }
}

TEST(ChannelConversion, RoundTrip40MHz) {
  using fuchsia_wlan_ieee80211::wire::ChannelBandwidth;
  using fuchsia_wlan_ieee80211::wire::WlanBand;

  // 5 GHz 40+ MHz (Cbw40)
  // IEEE Std 802.11-2024 Table E-4 Operating Class 116, 119, 122, 126
  const std::array<uint8_t, 12> five_ghz_40m_plus_channels = {36,  44,  52,  60,  100, 108,
                                                              116, 124, 132, 140, 149, 157};
  for (uint8_t ch : five_ghz_40m_plus_channels) {
    verify_round_trip({.band = WlanBand::kFiveGhz, .number = ch}, ChannelBandwidth::kCbw40);
  }

  // 5 GHz 40- MHz (Cbw40Below)
  // IEEE Std 802.11-2024 Table E-4 Operating Class 117, 120, 123, 127
  const std::array<uint8_t, 12> five_ghz_40m_minus_channels = {40,  48,  56,  64,  104, 112,
                                                               120, 128, 136, 144, 153, 161};
  for (uint8_t ch : five_ghz_40m_minus_channels) {
    verify_round_trip({.band = WlanBand::kFiveGhz, .number = ch}, ChannelBandwidth::kCbw40Below);
  }
}

TEST(ChannelConversion, ChanspecD11acToD11nSuccess) {
  brcmu_d11inf d11n_inf = {.io_type = BRCMU_D11N_IOTYPE};
  brcmu_d11_attach(&d11n_inf);

  auto verify_d11ac_to_d11n = [&](uint8_t ctl_ch, uint32_t bw, uint8_t expected_center_ch,
                                  uint8_t expected_band, enum brcmu_chan_bw expected_bw,
                                  enum brcmu_chan_sb expected_sb) {
    chanspec_t d11ac_chanspec = 0;
    ASSERT_EQ(channel2chspec(ctl_ch, bw, &d11ac_chanspec), ZX_OK);

    chanspec_t d11n_chanspec = 0;
    ASSERT_EQ(chanspec_d11ac_to_d11n(d11ac_chanspec, &d11n_chanspec), ZX_OK);

    brcmu_chan decoded_d11n = {.chspec = d11n_chanspec};
    d11n_inf.decchspec(&decoded_d11n);

    EXPECT_EQ(decoded_d11n.chnum, expected_center_ch);
    EXPECT_EQ(decoded_d11n.control_ch_num, ctl_ch);
    EXPECT_EQ(decoded_d11n.bw, expected_bw);
    EXPECT_EQ(decoded_d11n.band, expected_band);
    EXPECT_EQ(decoded_d11n.sb, expected_sb);
  };

  // 2.4 GHz 20 MHz channels (1 to 14)
  for (uint8_t ch = 1; ch <= 14; ++ch) {
    verify_d11ac_to_d11n(ch, WL_CHANSPEC_BW_20, ch, BRCMU_CHAN_BAND_2G, BRCMU_CHAN_BW_20,
                         BRCMU_CHAN_SB_NONE);
  }

  // 5 GHz 20 MHz channels
  constexpr auto five_ghz_20m_channels = std::to_array<uint8_t>(
      {36,  40,  44,  48,  52,  56,  60,  64,  100, 104, 108, 112, 116, 120,
       124, 128, 132, 136, 140, 144, 149, 153, 157, 161, 165, 169, 173, 177});
  for (uint8_t ch : five_ghz_20m_channels) {
    verify_d11ac_to_d11n(ch, WL_CHANSPEC_BW_20, ch, BRCMU_CHAN_BAND_5G, BRCMU_CHAN_BW_20,
                         BRCMU_CHAN_SB_NONE);
  }

  // There is a limitation imposed on 5GHz 40MHz channel widths.
  // //third_party/bcmdhd/crossdriver/bcmwifi_channels.cc defines the allowed 40MHz 5GHz channels
  // as
  //
  // wf_5g_40m_chans[] = {38, 46, 54, 62, 102, 110, 118, 126, 134, 142, 151, 159};

  // 5 GHz 40+ MHz channels (Cbw40 / lower primary)
  constexpr auto five_ghz_40m_plus_channels =
      std::to_array<uint8_t>({36, 44, 52, 60, 100, 108, 116, 124, 132, 140, 149, 157});
  for (uint8_t ch : five_ghz_40m_plus_channels) {
    verify_d11ac_to_d11n(ch, WL_CHANSPEC_BW_40, static_cast<uint8_t>(ch + CH_10MHZ_APART),
                         BRCMU_CHAN_BAND_5G, BRCMU_CHAN_BW_40, BRCMU_CHAN_SB_L);
  }

  // 5 GHz 40- MHz channels (Cbw40Below / upper primary)
  constexpr auto five_ghz_40m_minus_channels =
      std::to_array<uint8_t>({40, 48, 56, 64, 104, 112, 120, 128, 136, 144, 153, 161});
  for (uint8_t ch : five_ghz_40m_minus_channels) {
    verify_d11ac_to_d11n(ch, WL_CHANSPEC_BW_40, static_cast<uint8_t>(ch - CH_10MHZ_APART),
                         BRCMU_CHAN_BAND_5G, BRCMU_CHAN_BW_40, BRCMU_CHAN_SB_U);
  }
}

TEST(ChannelConversion, ChanspecD11acToD11nUnsupportedBandwidth) {
  chanspec_t d11n_chanspec = 0;

  // 80 MHz channel
  chanspec_t d11ac_80m = 0;
  ASSERT_EQ(channel2chspec(36, WL_CHANSPEC_BW_80, &d11ac_80m), ZX_OK);
  EXPECT_EQ(chanspec_d11ac_to_d11n(d11ac_80m, &d11n_chanspec), ZX_ERR_NOT_SUPPORTED);

  // 160 MHz channel
  chanspec_t d11ac_160m = 0;
  ASSERT_EQ(channel2chspec(36, WL_CHANSPEC_BW_160, &d11ac_160m), ZX_OK);
  EXPECT_EQ(chanspec_d11ac_to_d11n(d11ac_160m, &d11n_chanspec), ZX_ERR_NOT_SUPPORTED);

  // 80+80 MHz channel
  const chanspec_t d11ac_8080m = WL_CHANSPEC_BAND_5G | WL_CHANSPEC_BW_8080 |
                                 (0 << WL_CHANSPEC_CHAN1_SHIFT) | (1 << WL_CHANSPEC_CHAN2_SHIFT) |
                                 WL_CHANSPEC_CTL_SB_LL;
  EXPECT_EQ(chanspec_d11ac_to_d11n(d11ac_8080m, &d11n_chanspec), ZX_ERR_NOT_SUPPORTED);
}

TEST(ChannelConversion, ChanspecD11acToD11nInvalidArgs) {
  chanspec_t d11n_chanspec = 0;

  // Nullptr output
  EXPECT_EQ(chanspec_d11ac_to_d11n(0x1006, nullptr), ZX_ERR_INVALID_ARGS);

  // Invalid band (3G)
  const chanspec_t invalid_band_chanspec = WL_CHANSPEC_BAND_3G | WL_CHANSPEC_BW_20 | 6;
  EXPECT_EQ(chanspec_d11ac_to_d11n(invalid_band_chanspec, &d11n_chanspec), ZX_ERR_INVALID_ARGS);

  // Invalid channel (> MAXCHANNEL)
  const chanspec_t invalid_chan_chanspec =
      WL_CHANSPEC_BAND_5G | WL_CHANSPEC_BW_20 | (MAXCHANNEL + 1);
  EXPECT_EQ(chanspec_d11ac_to_d11n(invalid_chan_chanspec, &d11n_chanspec), ZX_ERR_INVALID_ARGS);

  // 40 MHz with invalid sideband (> U)
  const chanspec_t invalid_sb_40m =
      WL_CHANSPEC_BAND_5G | WL_CHANSPEC_BW_40 | WL_CHANSPEC_CTL_SB_LUU | 38;
  EXPECT_EQ(chanspec_d11ac_to_d11n(invalid_sb_40m, &d11n_chanspec), ZX_ERR_INVALID_ARGS);

  // 20 MHz with invalid non-zero sideband
  const chanspec_t invalid_sb_20m =
      WL_CHANSPEC_BAND_5G | WL_CHANSPEC_BW_20 | WL_CHANSPEC_CTL_SB_U | 36;
  EXPECT_EQ(chanspec_d11ac_to_d11n(invalid_sb_20m, &d11n_chanspec), ZX_ERR_INVALID_ARGS);
}
}  // namespace
