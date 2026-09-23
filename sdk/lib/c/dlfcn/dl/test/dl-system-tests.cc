// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "dl-system-tests.h"

#include <dlfcn.h>
#include <lib/fit/defer.h>

#include <cstring>

#include <gtest/gtest.h>

namespace dl::testing {

fit::error<Error> DlSystemTests::TakeError() {
  const char* error_str = dlerror();
  EXPECT_TRUE(error_str);

  // It's possible `dlerror_` is holding a previous error string, as the case
  // for a test that produces multiple consecutive errors without checking
  // dlerror() in between. `error_str` will replace the previous contents of
  // dlerror_ and the previous error message is no longer accessible, which
  // emulates the underlying behavior of dlerror().
  dlerror_ = std::string{error_str};

  return fit::error<Error>{"%s", error_str};
}

std::optional<Error> DlSystemTests::DlError() {
  if (!dlerror_.empty()) {
    Error err = Error{"%s", dlerror_.c_str()};
    dlerror_.clear();
    return std::move(err);
  }
  return std::nullopt;
}

fit::result<Error, void*> DlSystemTests::DlOpen(const char* file, int mode) {
  // Call dlopen in an OS-specific context.
  void* result = CallDlOpen(file, mode);
  if (!result) {
    if (mode & RTLD_NOLOAD) {
      // Musl emits a "Library x is not already loaded" for RTLD_NOLOAD, so
      // consume any failure from dlerror here.
      dlerror();
      return SuccessResult(result);
    }
    return TakeError();
  }
  TrackModule(result, file);
  return SuccessResult(result);
}

fit::result<Error> DlSystemTests::DlClose(void* module) {
  auto untrack_file = fit::defer([&]() { DlSystemLoadTestsBase::UntrackModule(module); });
  if (dlclose(module)) {
    return TakeError();
  }
  return SuccessResult();
}

fit::result<Error, void*> DlSystemTests::DlSym(void* module, const char* ref) {
  void* result = dlsym(module, ref);
  if (!result) {
    return TakeError();
  }
  return SuccessResult(result);
}

int DlSystemTests::DlIteratePhdr(DlIteratePhdrCallback* callback, void* data) {
  return dl_iterate_phdr(callback, data);
}

fit::result<Error, int> DlSystemTests::DlInfo(void* module, int request, void* info) {
  int result = dlinfo(module, request, info);
  if (result < 0) {
    EXPECT_EQ(result, -1);
    return TakeError();
  }
  return SuccessResult(result);
}

#ifdef __Fuchsia__

// Call dlopen with the mock fuchsia_ldsvc::Loader installed and check that all
// its Needed/Expect* expectations were satisfied before clearing them.
void* DlSystemTests::CallDlOpen(const char* file, int mode) {
  void* result;
  CallWithLdsvcInstalled([&]() { result = dlopen(file, mode); });
  VerifyAndClearNeeded();
  return result;
}

#else  // POSIX, not __Fuchsia__

// Call dlopen with the unadorned name, which the DT_RUNPATH in the host test
// executable will find in a subdirectory relative to that test executable.
void* DlSystemTests::CallDlOpen(const char* file, int mode) {
  if (file) {
    EXPECT_EQ(strchr(file, '/'), nullptr) << file;
  }
  return dlopen(file, mode);
}

#endif  // __Fuchsia__

void DlSystemTests::NoLoadCheck(std::string_view name) {
  auto result = DlOpen(std::string{name}.c_str(), RTLD_NOW | RTLD_LOCAL | RTLD_NOLOAD);
  ASSERT_TRUE(result.is_ok());
  ASSERT_EQ(result.value(), nullptr);
}

void DlSystemTests::ExpectRootModule(std::string_view name) {
  NoLoadCheck(name);
  DlSystemLoadTestsBase::ExpectRootModule(name);
}

void DlSystemTests::Needed(std::initializer_list<std::string_view> names) {
  for (auto name : names) {
    NoLoadCheck(name);
  }
  // Now add the expectation that the deps will be loaded from the filesystem.
  DlSystemLoadTestsBase::Needed(names);
}

void DlSystemTests::Needed(
    std::initializer_list<std::pair<std::string_view, bool>> name_found_pairs) {
  for (auto [name, found] : name_found_pairs) {
    if (found) {
      NoLoadCheck(name);
    }
  }
  // Now add the expectation that the deps will be loaded from the filesystem.
  DlSystemLoadTestsBase::Needed(name_found_pairs);
}

}  // namespace dl::testing
