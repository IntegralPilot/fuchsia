// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ld-remote-process-tests.h"

#include <lib/elfldltl/testing/diagnostics.h>
#include <lib/ld/abi.h>
#include <lib/ld/remote-abi-stub.h>
#include <zircon/process.h>

#include <string_view>

namespace ld::testing {

LdRemoteProcessTests::LdRemoteProcessTests() = default;

void LdRemoteProcessTests::SetUp() {
  ASSERT_NO_FATAL_FAILURE(stub_ld_vmo_ =
                              elfldltl::testing::GetTestLibVmo(RemoteAbiStub<>::kFilename));
}

LdRemoteProcessTests::~LdRemoteProcessTests() = default;

void LdRemoteProcessTests::Init(std::initializer_list<std::string_view> args,
                                std::initializer_list<std::string_view> env) {
  LdLoadZirconLdsvcTestsBase::Init(args, env);
  ASSERT_NO_FATAL_FAILURE(CreateProcess());
  ASSERT_NO_FATAL_FAILURE(InitVmarReservation(VmarBottomHalf(RootVmarInfo(), kPageSize)));
}

}  // namespace ld::testing
