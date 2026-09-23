// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYSMEM_TESTS_SYSMEM_COMMON_H_
#define SRC_SYSMEM_TESTS_SYSMEM_COMMON_H_

#include <lib/fidl/cpp/wire/traits.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <string>

// To dump a v1 corpus file for sysmem_fuzz.cc test, enable SYSMEM_FUZZ_CORPUS. Files can be found
// under /data/cache/r/sys/fuchsia.com:sysmem-test-v1:0#meta:sysmem.cm/ on the device.
//
// TODO(https://fxbug.dev/42067191): Make this and sysmem_fuzz.cc work for sysmem2.
#define SYSMEM_FUZZ_CORPUS 0

#define SYSMEM_CLASS_PATH "/dev/class/sysmem"

#define IF_FAILURES_RETURN()           \
  do {                                 \
    if (CURRENT_TEST_HAS_FAILURES()) { \
      return;                          \
    }                                  \
  } while (0)

#define IF_FAILURES_RETURN_FALSE()     \
  do {                                 \
    if (CURRENT_TEST_HAS_FAILURES()) { \
      return false;                    \
    }                                  \
  } while (0)

zx_koid_t get_koid(zx_handle_t handle);
zx_koid_t get_related_koid(zx_handle_t handle);

const std::string& GetBoardName();
bool is_board_astro();
bool is_board_sherlock();
bool is_board_luis();
bool is_board_nelson();
bool is_board_with_amlogic_secure();
bool is_board_with_amlogic_secure_vdec();

void nanosleep_duration(zx::duration duration);

#endif  // SRC_SYSMEM_TESTS_SYSMEM_COMMON_H_
