// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "dl-load-tests.h"
#include "startup-symbols.h"

namespace {

using dl::testing::DlTests;
TYPED_TEST_SUITE(DlTests, dl::testing::TestTypes, dl::testing::TestNames);

// This is used to check that module initializers and finalizers call the
// Callback function in a specific order.
class MockTestCallback : public TestCallback {
 public:
  MOCK_METHOD(void, Callback, (int), (override));

  // Expect the Callback function was called in the same order as
  // `expected_callbacks`.
  void ExpectCallbacks(std::initializer_list<int> expected_callbacks) {
    for (int i : expected_callbacks) {
      EXPECT_CALL(*this, Callback(i)).InSequence(sequence_);
    }
  }

 private:
  // The sequence object specific to this mock class object will only validate
  // the `EXPECT_CALL(...)`s of this mock instance.
  ::testing::Sequence sequence_;
};

// Sets the test module's gTestCallback to the given mock and runs the provided
// function under the context of the mock instance.
void RunWithMock(::testing::StrictMock<MockTestCallback>& mock, fit::function<void()>& run) {
  gTestCallback = &mock;
  run();
  gTestCallback = nullptr;
}

// Instantiate a MockTestCallback and prime it with an ordered list of expected
// callback values before running the given function. `RunWithMock` will run
// the callable function that should elicit the expected callbacks to run. The
// MockTestCallback will fail if the expected callbacks were not completed or
// called in order.
void RunWithExpectedTestCallbacks(fit::function<void()> run,
                                  std::initializer_list<int> expected_callbacks) {
  ::testing::StrictMock<MockTestCallback> mock;
  mock.ExpectCallbacks(expected_callbacks);
  ASSERT_FALSE(gTestCallback);
  RunWithMock(mock, run);
}

// dlopen a module whose initializers and finalizers are decoded by legacy
// DT_INIT and DT_FINI sections. These functions will call a callback with a
// value that is checked by the test to ensure those functions were run in order.
TYPED_TEST(DlTests, InitFiniLegacy) {
  const std::string kInitFiniLegacyFile = "init-fini-legacy.so";

  auto test = [&] {
    this->ExpectRootModule(kInitFiniLegacyFile);

    auto open = this->DlOpen(kInitFiniLegacyFile.c_str(), RTLD_NOW | RTLD_LOCAL);
    ASSERT_TRUE(open.is_ok()) << open.error_value();
    EXPECT_TRUE(open.value()) << open.error_value();

    ASSERT_TRUE(this->DlClose(open.value()).is_ok());
  };

  if (TestFixture::kDlCloseCanRunFinalizers) {
    RunWithExpectedTestCallbacks(test, {101, 102});
  } else {
    RunWithExpectedTestCallbacks(test, {101});
  }
}

// Similar to InitFiniLegacy test, except dlopen a module whose initializers and
// finalizers are decoded from DT_INIT_ARRAY/DT_FINI_ARRAY sections. This also
// tests that multiple initializers/finalizers in the dlopen-ed module are run in
// correct order.
TYPED_TEST(DlTests, InitFiniArray) {
  const std::string kInitFiniArrayFile = "init-fini-array.so";

  auto test = [&] {
    this->ExpectRootModule(kInitFiniArrayFile);

    auto open = this->DlOpen(kInitFiniArrayFile.c_str(), RTLD_NOW | RTLD_LOCAL);
    ASSERT_TRUE(open.is_ok()) << open.error_value();
    EXPECT_TRUE(open.value()) << open.error_value();

    ASSERT_TRUE(this->DlClose(open.value()).is_ok());
  };

  // Expect the three ctors to have run and three dtors to have run.
  if (TestFixture::kDlCloseCanRunFinalizers) {
    RunWithExpectedTestCallbacks(test, {0, 1, 2, 3, 4, 5});
  } else {
    RunWithExpectedTestCallbacks(test, {0, 1, 2});
  }
}

// Test that dlopen will run initializers and finalizers of a module with
// dependencies that also have initializers and finalizers. Similar to the above
// tests, each init/fini function calls a callback with a particular value that
// gets checked by the test.
//
// dlopen init-fini-array-root:
//   - init-fini-array-a:
//     - init-fini-array-a-dep
//   - init-fini-array-b:
//     - init-fini-array-b-dep
//   - init-fini-array-c
//
// Module initializers are run in this order:
//   init-fini-array-b-dep
//   init-fini-array-a-dep
//   init-fini-array-c
//   init-fini-array-b
//   init-fini-array-a
//   init-fini-array-root
//
// Module finalizers are run in reverse of the init order:
//   init-fini-array-root
//   init-fini-array-a
//   init-fini-array-b
//   init-fini-array-c
//   init-fini-array-a-dep
//   init-fini-array-b-dep
TYPED_TEST(DlTests, InitFiniArrayWithDeps) {
  const std::string kRootFile = "init-fini-array-with-deps.so";
  const std::string kAFile = "libinit-fini-array-a.so";
  const std::string kADepFile = "libinit-fini-array-a-dep.so";
  const std::string kBFile = "libinit-fini-array-b.so";
  const std::string kBDepFile = "libinit-fini-array-b-dep.so";
  const std::string kCFile = "libinit-fini-array-c.so";

  auto test = [&] {
    this->ExpectRootModule(kRootFile);
    this->Needed({kAFile, kBFile, kCFile, kADepFile, kBDepFile});

    auto open = this->DlOpen(kRootFile.c_str(), RTLD_NOW | RTLD_LOCAL);
    ASSERT_TRUE(open.is_ok()) << open.error_value();
    EXPECT_TRUE(open.value()) << open.error_value();

    ASSERT_TRUE(this->DlClose(open.value()).is_ok());
  };

  if (TestFixture::kDlCloseCanRunFinalizers) {
    RunWithExpectedTestCallbacks(test, {0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11});
  } else {
    RunWithExpectedTestCallbacks(test, {0, 1, 2, 3, 4, 5});
  }
}

// dlopen a module with a mix of DT_INIT/DT_FINI and DT_INIT_ARRAY and
// DT_FINI_ARRAY entries.
TYPED_TEST(DlTests, InitFiniArrayWithLegacy) {
  const std::string kInitFiniArrayWithLegacyFile = "init-fini-array-with-legacy.so";

  auto test = [&] {
    this->ExpectRootModule(kInitFiniArrayWithLegacyFile);

    auto open = this->DlOpen(kInitFiniArrayWithLegacyFile.c_str(), RTLD_NOW | RTLD_LOCAL);
    ASSERT_TRUE(open.is_ok()) << open.error_value();
    EXPECT_TRUE(open.value()) << open.error_value();

    ASSERT_TRUE(this->DlClose(open.value()).is_ok());
  };

  if (TestFixture::kDlCloseCanRunFinalizers) {
    RunWithExpectedTestCallbacks(test, {201, 202, 203, 204});
  } else {
    RunWithExpectedTestCallbacks(test, {201, 202});
  }
}

// Test that dlopen will only run initializers of modules when they are first
// loaded.
// dlopen init-fini-array-with-loaded-deps-a:
//  - init-fini-array-with-loaded-deps-a-dep
// dlopen init-fini-array-with-loaded-deps-a again.
// dlopen init-fini-array-with-loaded-deps-c
// dlopen init-fini-array-with-loaded-deps-with-loaded-deps:
//   - init-fini-array-with-loaded-deps-a (already loaded)
//     - init-fini-array-with-loaded-deps-a-dep (already loaded)
//   - init-fini-array-with-loaded-deps-b:
//     - init-fini-array-with-loaded-deps-b-dep
//   - init-fini-array-with-loaded-deps-c (already loaded)
//
// Module initializers are run in this order:
// ... in dlopen(init-fini-array-with-loaded-deps-with-loaded-deps-a):
//   init-fini-array-with-loaded-deps-a-dep
//   init-fini-array-with-loaded-deps-a
// ... in dlopen(init-fini-array-with-loaded-deps-with-loaded-deps-c):
//   init-fini-array-with-loaded-deps-c
// ... in dlopen(init-fini-array-with-loaded-deps-with-loaded-deps):
//   init-fini-array-with-loaded-deps-b-dep
//   init-fini-array-with-loaded-deps-b
//   init-fini-array-with-loaded-deps
//
// Note: Finalizers are run in the order in which the modules were loaded and
// this is triggered by the unloading of the last reference held by the root
// module:
//   init-fini-array-with-loaded-deps
//   init-fini-array-with-loaded-deps-a
//   init-fini-array-with-loaded-deps-a-dep
//   init-fini-array-with-loaded-deps-c
//   init-fini-array-with-loaded-deps-b
//   init-fini-array-with-loaded-deps-b-dep
TYPED_TEST(DlTests, InitFiniArrayWithLoadedDeps) {
  const std::string kRootFile = "init-fini-array-with-loaded-deps.so";
  const std::string kAFile = "libinit-fini-array-with-loaded-deps-a.so";
  const std::string kADepFile = "libinit-fini-array-with-loaded-deps-a-dep.so";
  const std::string kBFile = "libinit-fini-array-with-loaded-deps-b.so";
  const std::string kBDepFile = "libinit-fini-array-with-loaded-deps-b-dep.so";
  const std::string kCDepFile = "libinit-fini-array-with-loaded-deps-c.so";

  void* first_open_a;
  auto first_open_a_test = [&] {
    this->Needed({kAFile, kADepFile});
    auto open_a = this->DlOpen(kAFile.c_str(), RTLD_NOW | RTLD_LOCAL);
    ASSERT_TRUE(open_a.is_ok()) << open_a.error_value();
    EXPECT_TRUE(open_a.value()) << open_a.error_value();
    first_open_a = open_a.value();
  };

  RunWithExpectedTestCallbacks(first_open_a_test, {0, 1});

  // Don't expect another dlopen on dep-a will run initializers.
  void* second_open_a;
  auto second_open_a_test = [&] {
    auto open_a = this->DlOpen(kAFile.c_str(), RTLD_NOW | RTLD_LOCAL);
    ASSERT_TRUE(open_a.is_ok()) << open_a.error_value();
    EXPECT_TRUE(open_a.value()) << open_a.error_value();
    second_open_a = open_a.value();
  };

  RunWithExpectedTestCallbacks(second_open_a_test, {});

  void* open_c;
  auto open_c_test = [&] {
    this->Needed({kCDepFile});
    auto open = this->DlOpen(kCDepFile.c_str(), RTLD_NOW | RTLD_LOCAL);
    ASSERT_TRUE(open.is_ok()) << open.error_value();
    EXPECT_TRUE(open.value()) << open.error_value();
    open_c = open.value();
  };

  RunWithExpectedTestCallbacks(open_c_test, {2});

  void* open_root;
  auto open_root_test = [&] {
    this->ExpectRootModule(kRootFile);
    this->Needed({kBFile, kBDepFile});

    // This will only run initializers on the modules that are loaded by this call.
    auto open = this->DlOpen(kRootFile.c_str(), RTLD_NOW | RTLD_LOCAL);
    ASSERT_TRUE(open.is_ok()) << open.error_value();
    EXPECT_TRUE(open.value()) << open.error_value();
    open_root = open.value();
  };

  RunWithExpectedTestCallbacks(open_root_test, {3, 4, 5});

  // Don't expect these dlclose calls on dep-a will run any finalizers.
  RunWithExpectedTestCallbacks([&] { ASSERT_TRUE(this->DlClose(first_open_a).is_ok()); }, {});

  RunWithExpectedTestCallbacks([&] { ASSERT_TRUE(this->DlClose(second_open_a).is_ok()); }, {});

  RunWithExpectedTestCallbacks([&] { ASSERT_TRUE(this->DlClose(open_c).is_ok()); }, {});

  auto close_root_test = [&] { ASSERT_TRUE(this->DlClose(open_root).is_ok()); };
  if (TestFixture::kDlCloseCanRunFinalizers) {
    // Older glibc versions run destructors out of order. More information on
    // this flag setting is in dl-system-tests.h.
    if (TestFixture::kDestructorsRunOutOfOrder) {
      RunWithExpectedTestCallbacks(close_root_test, {6, 9, 7, 8, 10, 11});
    } else {
      RunWithExpectedTestCallbacks(close_root_test, {6, 7, 8, 9, 10, 11});
    }
  } else {
    RunWithExpectedTestCallbacks(close_root_test, {});
  }
}

}  // namespace
