// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/mmu_rcu/simple_generational_rcu.h>
#include <lib/unittest/unittest.h>

namespace rcu {
namespace {

bool TestBasicRead() {
  BEGIN_TEST;

  SimpleGenerational rcu;
  {
    AutoSimpleGenerationalReader reader(rcu);
  }

  END_TEST;
}

bool TestSynchronizeNoReaders() {
  BEGIN_TEST;

  SimpleGenerational rcu;
  // Should return immediately without readers.
  rcu.Synchronize();

  END_TEST;
}

bool TestReadThenSynchronize() {
  BEGIN_TEST;

  SimpleGenerational rcu;
  {
    AutoSimpleGenerationalReader reader(rcu);
  }
  rcu.Synchronize();

  END_TEST;
}

bool TestMultipleReaders() {
  BEGIN_TEST;

  SimpleGenerational rcu;
  {
    AutoSimpleGenerationalReader reader1(rcu);
    {
      AutoSimpleGenerationalReader reader2(rcu);
      {
        AutoSimpleGenerationalReader reader3(rcu);
      }
    }
  }
  rcu.Synchronize();

  END_TEST;
}

}  // namespace
}  // namespace rcu

UNITTEST_START_TESTCASE(simple_generational_rcu_tests)
UNITTEST("basic_read", rcu::TestBasicRead)
UNITTEST("synchronize_no_readers", rcu::TestSynchronizeNoReaders)
UNITTEST("read_then_synchronize", rcu::TestReadThenSynchronize)
UNITTEST("multiple_readers", rcu::TestMultipleReaders)
UNITTEST_END_TESTCASE(simple_generational_rcu_tests, "simple_generational_rcu",
                      "Tests for the simple generational RCU primitive.")
