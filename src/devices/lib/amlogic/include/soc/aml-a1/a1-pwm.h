// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_LIB_AMLOGIC_INCLUDE_SOC_AML_A1_A1_PWM_H_
#define SRC_DEVICES_LIB_AMLOGIC_INCLUDE_SOC_AML_A1_A1_PWM_H_

#include <soc/aml-a1/a1-hw.h>
#include <soc/aml-common/aml-pwm-regs.h>

#define A1_PWM_A 0
#define A1_PWM_B 1
#define A1_PWM_C 2
#define A1_PWM_D 3
#define A1_PWM_E 4
#define A1_PWM_F 5
#define A1_PWM_COUNT 6

namespace aml_pwm {

// Offsets
static_assert((A1_PWM_PWM_A == kAOffset) && (A1_PWM_PWM_C == kAOffset) &&
                  (A1_PWM_PWM_E == kAOffset),
              "PWM_PWM_A offset incorrect!\n");
static_assert((A1_PWM_PWM_B == kBOffset) && (A1_PWM_PWM_D == kBOffset) &&
                  (A1_PWM_PWM_F == kBOffset),
              "PWM_PWM_B offset incorrect!\n");
static_assert((A1_PWM_MISC_REG_AB == kMiscOffset) && (A1_PWM_MISC_REG_CD == kMiscOffset) &&
                  (A1_PWM_MISC_REG_EF == kMiscOffset),
              "MISC offset incorrect!\n");
static_assert((A1_DS_A_B == kDSOffset) && (A1_DS_C_D == kDSOffset) && (A1_DS_E_F == kDSOffset),
              "DS offset incorrect!\n");
static_assert((A1_PWM_TIME_AB == kTimeOffset) && (A1_PWM_TIME_CD == kTimeOffset) &&
                  (A1_PWM_TIME_EF == kTimeOffset),
              "Time offset incorrect!\n");
static_assert((A1_PWM_A2 == kA2Offset) && (A1_PWM_C2 == kA2Offset) && (A1_PWM_E2 == kA2Offset),
              "A2 offset incorrect!\n");
static_assert((A1_PWM_B2 == kB2Offset) && (A1_PWM_D2 == kB2Offset) && (A1_PWM_F2 == kB2Offset),
              "B2 offset incorrect!\n");
static_assert((A1_PWM_BLINK_AB == kBlinkOffset) && (A1_PWM_BLINK_CD == kBlinkOffset) &&
                  (A1_PWM_BLINK_EF == kBlinkOffset),
              "Blink offset incorrect!\n");
static_assert((A1_PWM_LOCK_AB == kLockOffset) && (A1_PWM_LOCK_CD == kLockOffset) &&
                  (A1_PWM_LOCK_EF == kLockOffset),
              "Lock offset incorrect!\n");

}  // namespace aml_pwm

#endif  // SRC_DEVICES_LIB_AMLOGIC_INCLUDE_SOC_AML_A1_A1_PWM_H_
