// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/panel-config.h"

#include <lib/device-protocol/display-panel.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-types/cpp/display-timing.h"

namespace amlogic_display {

namespace {

TEST(PanelConfig, BoeTv070wsmFitipowerJd9364Astro) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kBoeTv070wsmFitipowerJd9364Astro);
  ASSERT_NE(config, nullptr);
  EXPECT_STREQ(config->name, "BOE_TV070WSM_FITIPOWER_JD9364_ASTRO");
}

TEST(PanelConfig, InnoluxP070acbFitipowerJd9364) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kInnoluxP070acbFitipowerJd9364);
  ASSERT_NE(config, nullptr);
  EXPECT_STREQ(config->name, "INNOLUX_P070ACB_FITIPOWER_JD9364");
}

TEST(PanelConfig, BoeTv101wxmFitipowerJd9364) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kBoeTv101wxmFitipowerJd9364);
  ASSERT_NE(config, nullptr);
  EXPECT_STREQ(config->name, "BOE_TV101WXM_FITIPOWER_JD9364");
}

TEST(PanelConfig, InnoluxP101dezFitipowerJd9364) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kInnoluxP101dezFitipowerJd9364);
  ASSERT_NE(config, nullptr);
  EXPECT_STREQ(config->name, "INNOLUX_P101DEZ_FITIPOWER_JD9364");
}

TEST(PanelConfig, BoeTv101wxmFitipowerJd9365) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kBoeTv101wxmFitipowerJd9365);
  ASSERT_NE(config, nullptr);
  EXPECT_STREQ(config->name, "BOE_TV101WXM_FITIPOWER_JD9365");
}

TEST(PanelConfig, BoeTv070wsmFitipowerJd9365) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kBoeTv070wsmFitipowerJd9365);
  ASSERT_NE(config, nullptr);
  EXPECT_STREQ(config->name, "BOE_TV070WSM_FITIPOWER_JD9365");
}

TEST(PanelConfig, KdKd070d82FitipowerJd9364) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kKdKd070d82FitipowerJd9364);
  ASSERT_NE(config, nullptr);
  EXPECT_STREQ(config->name, "KD_KD070D82_FITIPOWER_JD9364");
}

TEST(PanelConfig, KdKd070d82FitipowerJd9365) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kKdKd070d82FitipowerJd9365);
  ASSERT_NE(config, nullptr);
  EXPECT_STREQ(config->name, "KD_KD070D82_FITIPOWER_JD9365");
}

TEST(PanelConfig, MicrotechMtf050fhdi03NovatekNt35596) {
  const PanelConfig* config =
      GetPanelConfig(display::PanelType::kMicrotechMtf050fhdi03NovatekNt35596);
  ASSERT_NE(config, nullptr);
  EXPECT_STREQ(config->name, "MICROTECH_MTF050FHDI03_NOVATEK_NT35596");
}

TEST(PanelConfig, BoeTv070wsmFitipowerJd9364Nelson) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kBoeTv070wsmFitipowerJd9364Nelson);
  ASSERT_NE(config, nullptr);
  EXPECT_STREQ(config->name, "BOE_TV070WSM_FITIPOWER_JD9364_NELSON");
}

TEST(PanelConfig, InvalidPanels) {
  const PanelConfig* config_0x04 = GetPanelConfig(static_cast<display::PanelType>(0x04));
  EXPECT_EQ(config_0x04, nullptr);

  const PanelConfig* config_0x05 = GetPanelConfig(static_cast<display::PanelType>(0x05));
  EXPECT_EQ(config_0x05, nullptr);

  const PanelConfig* config_0x06 = GetPanelConfig(static_cast<display::PanelType>(0x06));
  EXPECT_EQ(config_0x06, nullptr);

  const PanelConfig* config_0x0b = GetPanelConfig(static_cast<display::PanelType>(0x0b));
  EXPECT_EQ(config_0x0b, nullptr);

  const PanelConfig* config_overly_large = GetPanelConfig(static_cast<display::PanelType>(0x0e));
  EXPECT_EQ(config_overly_large, nullptr);

  const PanelConfig* config_unknown = GetPanelConfig(display::PanelType::kUnknown);
  EXPECT_EQ(config_unknown, nullptr);
}

TEST(PanelConfigTimingIsValid, BoeTv070wsmFitipowerJd9364Astro) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kBoeTv070wsmFitipowerJd9364Astro);
  ASSERT_NE(config, nullptr);

  EXPECT_EQ(display::FieldsPerFrame::kProgressive, config->display_timing.fields_per_frame);
  EXPECT_EQ(false, config->display_timing.vblank_alternates);
  EXPECT_EQ(0, config->display_timing.pixel_repetition);
  EXPECT_TRUE(config->display_timing.IsValid());
}

TEST(PanelConfigTimingIsValid, InnoluxP070acbFitipowerJd9364) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kInnoluxP070acbFitipowerJd9364);
  ASSERT_NE(config, nullptr);

  EXPECT_EQ(display::FieldsPerFrame::kProgressive, config->display_timing.fields_per_frame);
  EXPECT_EQ(false, config->display_timing.vblank_alternates);
  EXPECT_EQ(0, config->display_timing.pixel_repetition);
  EXPECT_TRUE(config->display_timing.IsValid());
}

TEST(PanelConfigTimingIsValid, BoeTv101wxmFitipowerJd9364) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kBoeTv101wxmFitipowerJd9364);
  ASSERT_NE(config, nullptr);

  EXPECT_EQ(display::FieldsPerFrame::kProgressive, config->display_timing.fields_per_frame);
  EXPECT_EQ(false, config->display_timing.vblank_alternates);
  EXPECT_EQ(0, config->display_timing.pixel_repetition);
  EXPECT_TRUE(config->display_timing.IsValid());
}

TEST(PanelConfigTimingIsValid, InnoluxP101dezFitipowerJd9364) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kInnoluxP101dezFitipowerJd9364);
  ASSERT_NE(config, nullptr);

  EXPECT_EQ(display::FieldsPerFrame::kProgressive, config->display_timing.fields_per_frame);
  EXPECT_EQ(false, config->display_timing.vblank_alternates);
  EXPECT_EQ(0, config->display_timing.pixel_repetition);
  EXPECT_TRUE(config->display_timing.IsValid());
}

TEST(PanelConfigTimingIsValid, BoeTv101wxmFitipowerJd9365) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kBoeTv101wxmFitipowerJd9365);
  ASSERT_NE(config, nullptr);

  EXPECT_EQ(display::FieldsPerFrame::kProgressive, config->display_timing.fields_per_frame);
  EXPECT_EQ(false, config->display_timing.vblank_alternates);
  EXPECT_EQ(0, config->display_timing.pixel_repetition);
  EXPECT_TRUE(config->display_timing.IsValid());
}

TEST(PanelConfigTimingIsValid, BoeTv070wsmFitipowerJd9365) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kBoeTv070wsmFitipowerJd9365);
  ASSERT_NE(config, nullptr);

  EXPECT_EQ(display::FieldsPerFrame::kProgressive, config->display_timing.fields_per_frame);
  EXPECT_EQ(false, config->display_timing.vblank_alternates);
  EXPECT_EQ(0, config->display_timing.pixel_repetition);
  EXPECT_TRUE(config->display_timing.IsValid());
}

TEST(PanelConfigTimingIsValid, KdKd070d82FitipowerJd9364) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kKdKd070d82FitipowerJd9364);
  ASSERT_NE(config, nullptr);

  EXPECT_EQ(display::FieldsPerFrame::kProgressive, config->display_timing.fields_per_frame);
  EXPECT_EQ(false, config->display_timing.vblank_alternates);
  EXPECT_EQ(0, config->display_timing.pixel_repetition);
  EXPECT_TRUE(config->display_timing.IsValid());
}

TEST(PanelConfigTimingIsValid, KdKd070d82FitipowerJd9365) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kKdKd070d82FitipowerJd9365);
  ASSERT_NE(config, nullptr);

  EXPECT_EQ(display::FieldsPerFrame::kProgressive, config->display_timing.fields_per_frame);
  EXPECT_EQ(false, config->display_timing.vblank_alternates);
  EXPECT_EQ(0, config->display_timing.pixel_repetition);
  EXPECT_TRUE(config->display_timing.IsValid());
}

TEST(PanelConfigTimingIsValid, MicrotechMtf050fhdi03NovatekNt35596) {
  const PanelConfig* config =
      GetPanelConfig(display::PanelType::kMicrotechMtf050fhdi03NovatekNt35596);
  ASSERT_NE(config, nullptr);

  EXPECT_EQ(display::FieldsPerFrame::kProgressive, config->display_timing.fields_per_frame);
  EXPECT_EQ(false, config->display_timing.vblank_alternates);
  EXPECT_EQ(0, config->display_timing.pixel_repetition);
  EXPECT_TRUE(config->display_timing.IsValid());
}

TEST(PanelConfigTimingIsValid, BoeTv070wsmFitipowerJd9364Nelson) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kBoeTv070wsmFitipowerJd9364Nelson);
  ASSERT_NE(config, nullptr);

  EXPECT_EQ(display::FieldsPerFrame::kProgressive, config->display_timing.fields_per_frame);
  EXPECT_EQ(false, config->display_timing.vblank_alternates);
  EXPECT_EQ(0, config->display_timing.pixel_repetition);
  EXPECT_TRUE(config->display_timing.IsValid());
}

TEST(PanelConfigRefreshRateMatchesSpec, BoeTv070wsmFitipowerJd9364Astro) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kBoeTv070wsmFitipowerJd9364Astro);
  ASSERT_NE(config, nullptr);
  EXPECT_EQ(config->display_timing.vertical_field_refresh_rate_millihertz(), 60'000);
}

TEST(PanelConfigRefreshRateMatchesSpec, InnoluxP070acbFitipowerJd9364) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kInnoluxP070acbFitipowerJd9364);
  ASSERT_NE(config, nullptr);
  EXPECT_EQ(config->display_timing.vertical_field_refresh_rate_millihertz(), 60'000);
}

TEST(PanelConfigRefreshRateMatchesSpec, BoeTv101wxmFitipowerJd9364) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kBoeTv101wxmFitipowerJd9364);
  ASSERT_NE(config, nullptr);
  EXPECT_EQ(config->display_timing.vertical_field_refresh_rate_millihertz(), 60'000);
}

TEST(PanelConfigRefreshRateMatchesSpec, InnoluxP101dezFitipowerJd9364) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kInnoluxP101dezFitipowerJd9364);
  ASSERT_NE(config, nullptr);
  EXPECT_EQ(config->display_timing.vertical_field_refresh_rate_millihertz(), 60'000);
}

TEST(PanelConfigRefreshRateMatchesSpec, BoeTv101wxmFitipowerJd9365) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kBoeTv101wxmFitipowerJd9365);
  ASSERT_NE(config, nullptr);
  EXPECT_EQ(config->display_timing.vertical_field_refresh_rate_millihertz(), 60'000);
}

TEST(PanelConfigRefreshRateMatchesSpec, BoeTv070wsmFitipowerJd9365) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kBoeTv070wsmFitipowerJd9365);
  ASSERT_NE(config, nullptr);
  EXPECT_EQ(config->display_timing.vertical_field_refresh_rate_millihertz(), 60'000);
}

TEST(PanelConfigRefreshRateMatchesSpec, KdKd070d82FitipowerJd9364) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kKdKd070d82FitipowerJd9364);
  ASSERT_NE(config, nullptr);
  EXPECT_EQ(config->display_timing.vertical_field_refresh_rate_millihertz(), 60'000);
}

TEST(PanelConfigRefreshRateMatchesSpec, KdKd070d82FitipowerJd9365) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kKdKd070d82FitipowerJd9365);
  ASSERT_NE(config, nullptr);
  EXPECT_EQ(config->display_timing.vertical_field_refresh_rate_millihertz(), 60'000);
}

TEST(PanelConfigRefreshRateMatchesSpec, BoeTv070wsmFitipowerJd9364Nelson) {
  const PanelConfig* config = GetPanelConfig(display::PanelType::kBoeTv070wsmFitipowerJd9364Nelson);
  ASSERT_NE(config, nullptr);
  EXPECT_EQ(config->display_timing.vertical_field_refresh_rate_millihertz(), 60'000);
}

}  // namespace

}  // namespace amlogic_display
