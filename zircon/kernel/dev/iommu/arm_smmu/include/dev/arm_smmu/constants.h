// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_CONSTANTS_H_
#define ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_CONSTANTS_H_

namespace arm_smmu {

// See kernel.arm-smmu-mode
enum class ArmSmmuMode {
  kDisabled,
  kPassthru,
  kEnforced,
};

// The addressing mode for a context bank determined using values in CBA2R and TCR.
enum class AddrMode {
  k32Bit,     // AArch32 Short-descriptor
  kExt32Bit,  // AArch32 Long-descriptor
  k64Bit,     // AArch64
  kInvalid,
};

// Operational mode for a SmmuBti.
//
// This enum is used in two places, at the BTI level, and at the context bank level for the (single)
// context bank owned by a BTI.  Note that when operating in fully enforced mode, the BTI may be in
// the Fault state (preventing new pin operations) while its ContextBank remains in Translation mode
// in order to continue to allow access to actively pinned PMTs, while specifically denying access
// to leaked PMT regions.
//
// In the context of a Context bank, the values used in this enum are not specific to any one
// register value. Instead, they reflect the way that we choose to model the three modes of
// operation using the CBAR (gr1), SCTLR (cb(N)), and TCR (cb(N)) registers.
//
// Their definitions for each mode are as follows.
//
// Mode        | CBAR.TYPE   | SCTLR.M | TCR.EPD0 | TCR.EPD1 | Notes
// ------------+-------------+---------+----------+----------+-------------------------------------
// Translation | S1TS2Bypass |    1    |    0     |    1     | MMU enabled, TTBR0 enabled
// Bypass      | S1TS2Bypass |    0    |    1     |    1     | MMU + TTBRs Disabled
// Fault       | S1TS2Bypass |    1    |    0     |    0     | MMU enabled, TTBRs disabled
// Adopted     |    ???      |   ???   |   ???    |   ???    | Register config at time of adoption.
// Shutdown    | S1TS2Bypass |    1    |    0     |    0     | Same as fault, deny all access.
//
// Note that all of the modes (except adopted) use Stage 1 Translate, Stage 2 Bypass as their
// CBAR type.  Stage 2's configuration is frequently under the control of either the hypervisor or
// the secure monitor, which may deny any attempt to put the system into a S1TS2Fault mode in order
// to force faulting.  Additionally, while we might specify either S2Bypass or S2Translate, we
// typically have no control over Stage 2 behavior from EL1.  We configure for S2Bypass, assuming
// that there is no S2 translation going on, however it may be the case that the system chooses to
// either change our written value to S1TS2Translate if it wants to perform translation, or to lie
// to us by reporting that our CBAR.TYPE is S1TS2Bypass even though (under the hood) it has actually
// configured for Stage 2 Translation.
//
// Either way, we are still able to represent all three of the primary modes with S1TS2Bypass.  For
// full translation, we enable the MMU, and TTBR0 which is configured to point to our page tables.
// TTBR1 is not currently used in any mode.  For bypass mode, we disable the MMU, which means that
// all transactions which reach this context bank are simply accepted and passed through as is with
// no translation.  Finally, when configured for Fault/Shutdown, the MMU is enabled, but both TTBRs
// are disabled, meaning there are no valid/active translation table entries, ensuring that
// "translation" in Fault mode always fails.
//
enum class BtiMode {
  // clang-format off
  kFault,       // Fault mode   : A fault has occurred.  Either a PMT has been leaked, or the HW
                //                attempted to access something it didn't have access to and we
                //                noticed.  Access is restricted until user-mode takes control of
                //                their HW and signals that it has by calling
                //                `zx_bti_release_quarantine`.
  kBypass,      // Passthru mode: No translation is performed, all accesses are allowed.
  kTranslation, // Enforced mode: Translation is performed, only pinned memory can be accessed.
  kAdopted,     // Adopted mode : The configuration is whatever was passed to us by our bootloader.
  kShutdown,    // Shutdown     : The BTI has been shutdown.  HW should be in Fault mode, but
                //                cannot ever return to an operational mode.  The BTI is about to
                //                be destroyed.
  kInvalid,
  // clang-format on
};

}  // namespace arm_smmu

#endif  // ZIRCON_KERNEL_DEV_IOMMU_ARM_SMMU_INCLUDE_DEV_ARM_SMMU_CONSTANTS_H_
