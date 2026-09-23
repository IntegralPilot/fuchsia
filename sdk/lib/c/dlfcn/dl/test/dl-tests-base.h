// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_DLFCN_DL_TEST_DL_TESTS_BASE_H_
#define LIB_C_DLFCN_DL_TEST_DL_TESTS_BASE_H_

#include <lib/elfldltl/resolve.h>

#include <gtest/gtest.h>

namespace dl::testing {

using DlIteratePhdrCallback = int(dl_phdr_info*, size_t, void*);

// The main purpose of this base class is provide default values for shared
// feature flags.
class DlTestsBase : public ::testing::Test {
 public:
  // These variables are indicators to GTEST of whether the test fixture
  // supports the associated feature so that it may skip related tests if not
  // supported:
  // Whether the test fixture can support matching error text exactly. This
  // allows different system implementations to pass tests that check whether an
  // expected error occurred without needing to adhere to the exact error
  // verbiage.
  static constexpr bool kCanMatchExactError = true;

  // A "Symbol not found" error is emitted for any undefined symbol error.
  static constexpr bool kEmitsSymbolNotFound = false;

  // Whether the dlopen implementation validates the mode argument.
  static constexpr bool kCanValidateMode = true;

  // Whether the test fixture will always prioritize a loaded module in symbol
  // resolution, regardless of whether it is a global module.
  static constexpr bool kStrictLoadOrderPriority = false;

  // Whether the test fixture's dlclose function will run finalizers.
  static constexpr bool kDlCloseCanRunFinalizers = true;

  // Whether the test fixture's dlclose function will unload the module.
  static constexpr bool kDlCloseUnloadsModules = true;

  // This is false if the test fixture's dl_iterate_pdhr output reflects the
  // true number of loaded modules after a module is looked up by DT_SONAME.
  static constexpr bool kInaccurateLoadCountAfterSonameMatch = false;

  // Whether the test fixture can match against the DT_SONAME of a dep module
  // pending to be loaded in a linking session.
  static constexpr bool kSonameLookupInPendingDeps = true;

  // Whether the test fixture can match against the DT_SONAME of a dep module
  // that was already loaded in a linking session.
  static constexpr bool kSonameLookupInLoadedDeps = true;

  // Whether the test fixture will resolve to the first symbol it finds, even if
  // the symbol is marked weak.
  static constexpr elfldltl::ResolverPolicy kResolverPolicy =
      elfldltl::ResolverPolicy::kStrictLinkOrder;

  // Whether the test fixture implementation emits the unsupported flag value
  // in its error message.
  static constexpr bool kEmitsDlInfoUnsupportedValue = false;

  // Whether the test fixture supports RTLD_DI_* extension flags. Older glibc
  // versions don't support these options.
  static constexpr bool kSupportsDlInfoExtensionFlags = true;

  // Whether the test fixture's dlclose will run destructors out of order. Older
  // glibc versions resort the order that destructors are run.
  static constexpr bool kDestructorsRunOutOfOrder = false;
};

}  // namespace dl::testing

#endif  // LIB_C_DLFCN_DL_TEST_DL_TESTS_BASE_H_
