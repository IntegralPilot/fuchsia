// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_X86_CODE_PATCHES_INCLUDE_ARCH_CODE_PATCHES_CASE_ID_H_
#define ZIRCON_KERNEL_ARCH_X86_CODE_PATCHES_INCLUDE_ARCH_CODE_PATCHES_CASE_ID_H_

#include <stdint.h>

// Defines known code-patching case IDs for the kernel.
// Each should be listed below in CodePatchNames as well.
enum class CodePatchId : uint32_t {
  // This case serves as a verification that code-patching was performed before
  // the kernel was booted, `nop`ing out a trap among the kernel's earliest
  // instructions.
  kSelfTest,

  // Addresses `swapgs` speculation attacks (CVE-2019-1125):
  // https://software.intel.com/security-software-guidance/advisory-guidance/speculative-behavior-swapgs-and-segment-registers
  // Mitigation involves following `swapgs` instances with a load fence;
  // mitigation is the default and patching is equivalent to `nop`-ing it out.
  kSwapgsMitigation,

  // Addresses MDS and TAA vulnerabilities (CVE-2018-12126, CVE-2018-12127,
  // CVE-2018-12130, CVE-2019-11091, and CVE-2019-11135):
  // https://www.intel.com/content/www/us/en/architecture-and-technology/mds.html
  //
  // Mitigation involves making use of the MD_CLEAR feature, when available;
  // mitigation is the default and patching is equivalent to `nop`-ing it out.
  kMdsTaaMitigation,

  // Encodes a decision between implementations of
  // `_x86_user_copy_to_or_from_user()`, in which we try to take advantage of
  // optimizations (e.g., in the case when `movsb` is expected to be more
  // efficient than `movsq`) and securities (e.g., SMAP) when available.
  //
  // Note: the "_" is intentional as the function name has a leading underscore.
  k_X86CopyToOrFromUser,

  // Addresses Branch Target Injection / Spectre Variant 2 attacks
  // (CVE-2017-5715) by "retpolines":
  // https://software.intel.com/security-software-guidance/advisory-guidance/branch-target-injection
  //
  // Note: the "__" is intentional as the function name has two leading
  // underscores.
  k__X86IndirectThunkR11,

  // Relates to the optimizations available for C string utilities.
  //
  // Note: the "__" is intentional as the function name has two leading
  // underscores.
  k__UnsanitizedMemcpy,
  k__UnsanitizedMemset,
};

// The callback accepts an initializer-list of something constructible with
// {CodePatchId, std::string_view} and gets a list mapping kFooBar -> "FOO_BAR"
// name strings.  The names should be the kFooBar -> FOO_BAR transliteration of
// the enum names. In assembly code, these will be used as "CASE_ID_FOO_BAR".
inline constexpr auto WithCodePatchNames = [](auto&& callback) {
  return callback({
      {CodePatchId::kSelfTest, "SELF_TEST"},
      {CodePatchId::kSwapgsMitigation, "SWAPGS_MITIGATION"},
      {CodePatchId::kMdsTaaMitigation, "MDS_TAA_MITIGATION"},
      {CodePatchId::k_X86CopyToOrFromUser, "_X86_COPY_TO_OR_FROM_USER"},
      {CodePatchId::k__X86IndirectThunkR11, "__X86_INDIRECT_THUNK_R11"},
      {CodePatchId::k__UnsanitizedMemcpy, "__UNSANITIZED_MEMCPY"},
      {CodePatchId::k__UnsanitizedMemset, "__UNSANITIZED_MEMSET"},
  });
};

#endif  // ZIRCON_KERNEL_ARCH_X86_CODE_PATCHES_INCLUDE_ARCH_CODE_PATCHES_CASE_ID_H_
