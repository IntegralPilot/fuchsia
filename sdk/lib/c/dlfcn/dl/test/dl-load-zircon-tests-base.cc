// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "dl-load-zircon-tests-base.h"

#include <fidl/fuchsia.ldsvc/cpp/wire.h>
#include <lib/ld/testing/vdso.h>

namespace dl::testing {

void DlLoadZirconTestsBase::FileCheck(std::string_view filename) {
  Base::FileCheck(filename);
  EXPECT_NE(filename, ld::testing::GetVdsoSoname().str());
}

fit::result<std::optional<DlLoadZirconTestsBase::SystemError>, DlLoadZirconTestsBase::File>
DlLoadZirconTestsBase::RetrieveFile(Diagnostics& diag, std::string_view filename) {
  FileCheck(filename);

  // Borrow the client channel handle to the mock loader service and make a
  // fuchsia.ldsvc/Loader.LoadObject request to it.
  fidl::Arena arena;
  auto result = fidl::WireCall(mock_.client())->LoadObject({arena, filename});
  EXPECT_TRUE(result.ok());
  auto load_result = result.Unwrap();
  if (load_result->rv == ZX_OK) {
    return fit::ok(File{std::move(load_result->object), diag});
  }
  // The only expected failure is a "not found" error.
  EXPECT_EQ(load_result->rv, ZX_ERR_NOT_FOUND);
  return fit::error(std::nullopt);
}

}  // namespace dl::testing
