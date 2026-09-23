// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_TESTS_SYSCALLS_CPP_BINDER_MANAGER_H_
#define SRC_STARNIX_TESTS_SYSCALLS_CPP_BINDER_MANAGER_H_

#include <lib/fit/defer.h>
#include <lib/fit/function.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace starnix_binder {

fit::deferred_action<fit::closure> ManagerProcess(
    std::string_view binder_dir,
    fit::function<pid_t(test_helper::ForkHelper&, fit::closure)> spawn_manager,
    test_helper::Poker ready);

}  // namespace starnix_binder

#endif  // SRC_STARNIX_TESTS_SYSCALLS_CPP_BINDER_MANAGER_H_
