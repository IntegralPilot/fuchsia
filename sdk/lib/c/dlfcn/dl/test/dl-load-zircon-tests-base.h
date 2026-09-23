// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DL_TEST_DL_LOAD_ZIRCON_TESTS_BASE_H_
#define LIB_DL_TEST_DL_LOAD_ZIRCON_TESTS_BASE_H_

#include <lib/elfldltl/vmar-loader.h>
#include <lib/elfldltl/vmo.h>
#include <lib/ld/testing/mock-loader-service.h>

#include "dl-load-tests-base.h"

namespace dl::testing {

// DlLoadZirconTestsBase contains Fuchsia-specific types and logic for running
// tests on Fuchsia. This subclass overrides methods from DlLoadTestsBase as
// needed to test Fuchsia-specific behavior. In particular, this class provides
// a MockLoaderServiceForTest that tests should use in the place of the normal
// system loader service so as to interact with the Expect/Needed API. The
// Expect/Needed API verifies that modules and/or dependencies were loaded by
// the fuchsia.ldsvc.Loader as expected.
class DlLoadZirconTestsBase : public DlLoadTestsBase {
 public:
  static constexpr std::string_view kName = "DlImplLoadZirconTests";

  using Base = DlLoadTestsBase;
  using File = elfldltl::VmoFile<Diagnostics>;
  using Loader = elfldltl::LocalVmarLoader;
  using SystemError = elfldltl::ZirconError;

  // `ExpectRootModule` will prime the mock loader with the root module and
  // register an expectation for it. Usually, `ExpectRootModule` is used for the
  // module that is being `dlopen`-ed and should be called before `Needed` for
  // the mock loader to expect to load the root module before its dependencies.
  constexpr void ExpectRootModule(std::string_view name) { mock_.ExpectRootModule(name); }

  constexpr void ExpectMissing(std::string_view name) { mock_.ExpectMissing(name); }

  // `Needed` registers the ordered set of dependencies the mock loader is
  // expected to load. Likewise, tests must intersperse calls to all
  // Expect*/Needed methods in the order they are expected to be received by
  // the mock loader.
  constexpr void Needed(std::initializer_list<std::string_view> names) { mock_.Needed(names); }

  constexpr void Needed(std::initializer_list<std::pair<std::string_view, bool>> name_found_pairs) {
    mock_.Needed(name_found_pairs);
  }

  void CallWithLdsvcInstalled(fit::closure func) { mock_.CallWithLdsvcInstalled(std::move(func)); }

  void VerifyAndClearNeeded() { mock_.VerifyAndClearExpectations(); }

  // Retrieve a VMO from the test package. If the file is not found, a
  // fit::error{std::nullopt} is returned to the caller, otherwise a
  // fit::error{ZirconError} is returned containing information about the
  // encountered system error.
  fit::result<std::optional<SystemError>, DlLoadZirconTestsBase::File> RetrieveFile(
      Diagnostics& diag, std::string_view filename);

 private:
  // Calls Base::FileCheck and includes an additional check for Fuchsia-specific
  // startup modules.
  static void FileCheck(std::string_view filename);

  ld::testing::MockLoaderServiceForTest mock_;
};

}  // namespace dl::testing

#endif  // LIB_DL_TEST_DL_LOAD_ZIRCON_TESTS_BASE_H_
