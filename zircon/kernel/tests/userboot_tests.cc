// Copyright 2019 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/unittest/unittest.h>
#include <lib/userabi/userboot.h>

#include <object/dispatcher.h>
#include <object/handle.h>
#include <object/resource_dispatcher.h>

namespace {

bool GetRangedResourceTest() {
  BEGIN_TEST;
  HandleOwner rsrc_handle = get_resource_handle(ZX_RSRC_KIND_MMIO);
  auto rsrc_dispatcher = DownCastDispatcher<ResourceDispatcher>(rsrc_handle->dispatcher().get());

  zx_info_resource_t info = rsrc_dispatcher->GetInfo();
  ASSERT_TRUE(info.kind == ZX_RSRC_KIND_MMIO);
  ASSERT_TRUE(info.base == 0);
  ASSERT_TRUE(info.size == 0);

  END_TEST;
}

}  // namespace

UNITTEST_START_TESTCASE(userboot_tests)
UNITTEST("get_ranged_resource", GetRangedResourceTest)
UNITTEST_END_TESTCASE(userboot_tests, "userboot", "userboot tests")
