// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_LD_TEST_LOAD_TESTS_H_
#define LIB_LD_TEST_LOAD_TESTS_H_

#include <lib/elfldltl/testing/typed-test.h>

#include <gtest/gtest.h>

#ifdef __Fuchsia__
#include "ld-remote-process-tests.h"
#include "ld-startup-create-process-tests.h"
#include "ld-startup-in-process-tests-zircon.h"
#include "ld-startup-spawn-process-tests-zircon.h"
#else
#include "ld-startup-in-process-tests-posix.h"
#include "ld-startup-spawn-process-tests-posix.h"
#endif

namespace ld::testing {

template <class Fixture>
using LdLoadTests = Fixture;

template <class Fixture>
using LdLoadFailureTests = Fixture;

// This lists all the types that are compatible with both LdLoadTests and
// LdLoadFailureTests.
template <class... Tests>
using TestTypes = ::testing::Types<
#ifdef __Fuchsia__
    ld::testing::LdStartupCreateProcessTests<>,  //
    // TODO(https://fxbug.dev/513259643): Shared process tests using the startup
    // dynamic linker are disabled because the current system program loader
    // (//src/lib/process_builder) only reserves the lower half of the full
    // address space ([0, aspace_size/2)). In a ZX_PROCESS_SHARED process,
    // the root VMAR starts in the top half of the address space, so the
    // system loader performs no reservation inside the root VMAR itself. The
    // startup dynamic linker expects the bottom half of its root VMAR to be
    // free so it can reserve it during startup, which conflicts if the loader
    // maps the linker into that space. Unlike in-process tests, the
    // LdStartupCreate* fixtures are intended to match actual system program
    // loader behavior rather than diverging from it. This fixture can be
    // re-enabled once process_builder is updated to reserve the bottom half
    // of the root VMAR for shared processes.
    // ld::testing::LdStartupCreateSharedProcessTests<>,  //
    ld::testing::LdRemoteProcessTests,        //
    ld::testing::LdRemoteSharedProcessTests,  //
#endif
    ld::testing::LdStartupSpawnProcessTests, Tests...>;

// These types are meaningul for the successful tests, LdLoadTests.
using LoadTypes = TestTypes<ld::testing::LdStartupInProcessTests>;

// These types are the types which are compatible with the failure tests,
// LdLoadFailureTests.
using FailTypes = TestTypes<>;

TYPED_TEST_SUITE(LdLoadTests, LoadTypes, elfldltl::testing::TestNames);
TYPED_TEST_SUITE(LdLoadFailureTests, FailTypes, elfldltl::testing::TestNames);

}  // namespace ld::testing

#endif  // LIB_LD_TEST_LOAD_TESTS_H_
