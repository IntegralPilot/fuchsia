// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <stdint.h>

#include <dev/arm_smmu/constants.h>
#include <ktl/optional.h>

#ifndef ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_SMMU_MODE_H_
#define ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_SMMU_MODE_H_

namespace arm_smmu {

// Determines the proper ArmSmmuMode if specified, or simply returns the default
// mode otherwise.
//
// Which mode to choose is determined by the "mode_string".  It is
// a string with the following form, given in a pseudo-regular expression
// notation:
//
// <mode>(,<base_addr>=<mode>)*
//
// "<mode>" is one of three tokens, "disabled", "passthru" or "enforced",
// corresponding to each of the three defined ArmSmmuModes.  The first mode
// listed is mandatory and specifies the default operating mode.  This is
// followed by a comma separated list of zero or more modes for specific SMMU
// instances in the form "<base_addr>=<mode>".  "<base_addr>" is a 64-bit
// unsigned identifying a specific instance of an SMMU for which the given mode
// should be used.
//
// When GetSmmuMode is called, a `base_addr` may optionally be passed.  When no
// base address is provided, GetSmmuMode should return the default operating
// mode.  When a base address is passed, GetSmmuMode should return the mode for
// the first optional entry which matches that base address.  If no entry is
// found which matches the base address, the default mode should be returned
// instead.  If a chosen mode token string is invalid, a warning should be
// printed, and the value ArmSmmuMode::kDisabled should be returned instead.
//
ktl::optional<ArmSmmuMode> GetSmmuMode(const char* mode_string,
                                       ktl::optional<uint64_t> base_addr = ktl::nullopt);

bool ValidateSmmuModeString(const char* mode_string);

}  // namespace arm_smmu

#endif  // ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_SMMU_MODE_H_
