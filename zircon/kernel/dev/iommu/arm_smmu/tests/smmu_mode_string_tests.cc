// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <dev/arm_smmu/smmu_mode.h>
#include <zxtest/zxtest.h>

namespace arm_smmu {
namespace {
constexpr ArmSmmuMode kInvalidArmSmmuMode = static_cast<ArmSmmuMode>(0xbaadf00d);

TEST(SmmuModeStringTest, Defaults) {
  // Test basic token parsing for the default mode.
  EXPECT_EQ(ArmSmmuMode::kDisabled, GetSmmuMode("disabled").value_or(kInvalidArmSmmuMode));
  EXPECT_EQ(ArmSmmuMode::kPassthru, GetSmmuMode("passthru").value_or(kInvalidArmSmmuMode));
  EXPECT_EQ(ArmSmmuMode::kEnforced, GetSmmuMode("enforced").value_or(kInvalidArmSmmuMode));

  // Test some invalid a strings.  We expect anything invalid to come back as "kDisabled".
  EXPECT_FALSE(GetSmmuMode("").has_value());
  EXPECT_FALSE(GetSmmuMode("pass thru").has_value());
  EXPECT_FALSE(GetSmmuMode("pass-thru").has_value());
  EXPECT_FALSE(GetSmmuMode("asdfjkldsf").has_value());
}

TEST(SmmuModeStringTest, Advanced) {
  // Test base_addr matching.
  EXPECT_EQ(ArmSmmuMode::kPassthru,
            GetSmmuMode("disabled,0x1234=passthru", 0x1234).value_or(kInvalidArmSmmuMode));
  EXPECT_EQ(ArmSmmuMode::kDisabled, GetSmmuMode("disabled,0x1234=passthru", 0x5678)
                                        .value_or(kInvalidArmSmmuMode));  // Fallback to default

  // Test multiple entries.
  EXPECT_EQ(ArmSmmuMode::kEnforced, GetSmmuMode("disabled,0x1234=passthru,0x5678=enforced", 0x5678)
                                        .value_or(kInvalidArmSmmuMode));
  EXPECT_EQ(ArmSmmuMode::kPassthru, GetSmmuMode("disabled,0x1234=passthru,0x5678=enforced", 0x1234)
                                        .value_or(kInvalidArmSmmuMode));

  // Test first match wins.
  EXPECT_EQ(ArmSmmuMode::kPassthru, GetSmmuMode("disabled,0x1234=passthru,0x1234=enforced", 0x1234)
                                        .value_or(kInvalidArmSmmuMode));

  // Test invalid entries.
  EXPECT_EQ(ArmSmmuMode::kDisabled,
            GetSmmuMode("disabled,0x1234passthru", 0x1234).value_or(kInvalidArmSmmuMode));
  EXPECT_FALSE(GetSmmuMode("disabled,0x1234=invalid", 0x1234).has_value());

  // Test hex vs decimal parsing.
  EXPECT_EQ(ArmSmmuMode::kPassthru,
            GetSmmuMode("disabled,4660=passthru", 0x1234).value_or(kInvalidArmSmmuMode));

  // Test skipping bad address segments to find a match.
  EXPECT_EQ(ArmSmmuMode::kPassthru, GetSmmuMode("disabled,invalid_segment,0x1234=passthru", 0x1234)
                                        .value_or(kInvalidArmSmmuMode));
  EXPECT_EQ(
      ArmSmmuMode::kEnforced,
      GetSmmuMode("disabled,0x1234passthru,0x5678=enforced", 0x5678).value_or(kInvalidArmSmmuMode));

  // Test empty specific mode token.
  EXPECT_FALSE(GetSmmuMode("disabled,0x1234=", 0x1234).has_value());
}

TEST(SmmuModeStringTest, CaseInsensitive) {
  // Test mixed case for default mode.
  EXPECT_EQ(ArmSmmuMode::kDisabled, GetSmmuMode("DiSaBlEd").value_or(kInvalidArmSmmuMode));
  EXPECT_EQ(ArmSmmuMode::kPassthru, GetSmmuMode("PASSTHRU").value_or(kInvalidArmSmmuMode));
  EXPECT_EQ(ArmSmmuMode::kEnforced, GetSmmuMode("EnFoRcEd").value_or(kInvalidArmSmmuMode));

  // Test mixed case in specific entries.
  EXPECT_EQ(ArmSmmuMode::kPassthru,
            GetSmmuMode("disabled,0x1234=PaSsThRu", 0x1234).value_or(kInvalidArmSmmuMode));
  EXPECT_EQ(ArmSmmuMode::kEnforced,
            GetSmmuMode("disabled,0x1234=ENFORCED", 0x1234).value_or(kInvalidArmSmmuMode));
}

TEST(SmmuModeStringTest, Whitespace) {
  // Test leading/trailing whitespace for default mode.
  EXPECT_EQ(ArmSmmuMode::kPassthru, GetSmmuMode("  passthru  ").value_or(kInvalidArmSmmuMode));
  EXPECT_EQ(ArmSmmuMode::kDisabled, GetSmmuMode(" disabled ").value_or(kInvalidArmSmmuMode));

  // Test whitespace around delimiters in specific entries.
  EXPECT_EQ(ArmSmmuMode::kPassthru,
            GetSmmuMode("disabled , 0x1234 = passthru", 0x1234).value_or(kInvalidArmSmmuMode));
  EXPECT_EQ(ArmSmmuMode::kEnforced,
            GetSmmuMode("disabled, 0x1234 = enforced ", 0x1234).value_or(kInvalidArmSmmuMode));
  EXPECT_EQ(ArmSmmuMode::kPassthru,
            GetSmmuMode("disabled ,0x1234= passthru", 0x1234).value_or(kInvalidArmSmmuMode));

  // Test mixed tabs and spaces.
  EXPECT_EQ(ArmSmmuMode::kPassthru,
            GetSmmuMode(" \t disabled \t , \t 0x1234 \t = \t passthru \t ", 0x1234)
                .value_or(kInvalidArmSmmuMode));

  // Test whitespace in multiple entries.
  EXPECT_EQ(ArmSmmuMode::kEnforced,
            GetSmmuMode("disabled, 0x1234 = passthru , 0x5678 = enforced", 0x5678)
                .value_or(kInvalidArmSmmuMode));
  EXPECT_EQ(ArmSmmuMode::kPassthru,
            GetSmmuMode("disabled, 0x1234 = passthru , 0x5678 = enforced", 0x1234)
                .value_or(kInvalidArmSmmuMode));

  // Test whitespace with Fallback.
  EXPECT_EQ(ArmSmmuMode::kPassthru,
            GetSmmuMode(" passthru , 0x1234 = enforced", 0x5678).value_or(kInvalidArmSmmuMode));
}

TEST(SmmuModeStringTest, Validation) {
  // Test valid strings.
  EXPECT_TRUE(ValidateSmmuModeString("disabled"));
  EXPECT_TRUE(ValidateSmmuModeString("passthru"));
  EXPECT_TRUE(ValidateSmmuModeString("enforced"));
  EXPECT_TRUE(ValidateSmmuModeString("disabled,0x1234=passthru"));
  EXPECT_TRUE(ValidateSmmuModeString("disabled,0x1234=passthru,0x5678=enforced"));
  EXPECT_TRUE(ValidateSmmuModeString("  disabled  "));
  EXPECT_TRUE(ValidateSmmuModeString("disabled , 0x1234 = passthru"));

  // Test invalid strings.
  EXPECT_FALSE(ValidateSmmuModeString(""));
  EXPECT_FALSE(ValidateSmmuModeString("pass thru"));
  EXPECT_FALSE(ValidateSmmuModeString("disabled,0x1234passthru"));          // Malformed address
  EXPECT_FALSE(ValidateSmmuModeString("disabled,0x1234=invalid"));          // Invalid mode
  EXPECT_FALSE(ValidateSmmuModeString("disabled,0x1234="));                 // Empty mode
  EXPECT_FALSE(ValidateSmmuModeString("disabled,"));                        // Trailing comma
  EXPECT_FALSE(ValidateSmmuModeString("disabled,,"));                       // Double comma
  EXPECT_FALSE(ValidateSmmuModeString("invalid_segment,0x1234=passthru"));  // Bad segment first
}

}  // namespace
}  // namespace arm_smmu
