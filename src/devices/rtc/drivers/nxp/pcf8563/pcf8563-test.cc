// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "pcf8563.h"

#include <fidl/fuchsia.hardware.i2c/cpp/fidl.h>
#include <fidl/fuchsia.hardware.rtc/cpp/fidl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fdio/directory.h>
#include <lib/fit/result.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>

#include <functional>
#include <optional>
#include <sstream>
#include <string>
#include <utility>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace fi2c = fuchsia_hardware_i2c;
namespace frtc = fuchsia_hardware_rtc;

class MockI2c : public fidl::Server<fi2c::Device> {
 public:
  fi2c::Service::InstanceHandler GetInstanceHandler() {
    return fi2c::Service::InstanceHandler({
        .device = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                          fidl::kIgnoreBindingClosure),
    });
  }

  // I2c FIDL protocol methods.
  MOCK_METHOD(void, Transfer, (TransferRequest&, TransferCompleter::Sync&), (override));
  MOCK_METHOD(void, GetName, (GetNameCompleter::Sync&), (override));

 private:
  fidl::ServerBindingGroup<fi2c::Device> bindings_;
};

class SimpleDriverTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    return to_driver_vfs.AddService<fi2c::Service>(i2c_.GetInstanceHandler());
  }

  MockI2c& i2c() { return i2c_; }

 private:
  MockI2c i2c_;
};

class TestConfig final {
 public:
  using DriverType = pcf8563::RtcDriver;
  using EnvironmentType = SimpleDriverTestEnvironment;
};

// Class template parameters:
//   manage_lifetime: True if the test should provide DFv2 driver lifetime management. If false, it
//     will be the individual test(s) responsibility to start and stop the driver instance manually.
//   gtest_base: The base GTest fixture class. Defaults to testing::Test (see Parameterized tests
//     towards the end of this module).
template <bool manage_lifetime, typename gtest_base = testing::Test>
class BaseTest : public gtest_base {
 public:
  void SetUp() override {
    if (manage_lifetime) {
      driver_test().RunInEnvironmentTypeContext([](SimpleDriverTestEnvironment& env) {
        // Set up the test such that TESTC-bit is cleared on entry, and the chip contains a sensical
        // datetime.
        auto csr_read = [&](MockI2c::TransferRequest& req, MockI2c::TransferCompleter::Sync& comp) {
          // TESTC-bit cleared.
          std::vector<uint8_t> testc_clear{0};  // TESTC-bit cleared.
          comp.Reply(zx::ok(fi2c::DeviceTransferResponse{{.read_data = {std::move(testc_clear)}}}));
        };

        auto time0_read = [&](MockI2c::TransferRequest& req,
                              MockI2c::TransferCompleter::Sync& comp) {
          std::vector<uint8_t> date{0, 0, 0, 1, 0, 1, 0};  // 1900-01-01T00:00:00
          comp.Reply(zx::ok(fi2c::DeviceTransferResponse{{.read_data = {std::move(date)}}}));
        };
        EXPECT_CALL(env.i2c(), Transfer).WillOnce(csr_read).WillOnce(time0_read);
      });

      zx::result<> result = driver_test().StartDriver();
      ASSERT_EQ(ZX_OK, result.status_value());

      zx::result connect_result = driver_test().template Connect<frtc::Service::Device>();
      ASSERT_EQ(ZX_OK, connect_result.status_value());
      client_.Bind(std::move(connect_result.value()));
      ASSERT_TRUE(client_.is_valid());
    }
  }

  void TearDown() override {
    if (manage_lifetime) {
      zx::result<> result = driver_test().StopDriver();
      ASSERT_EQ(ZX_OK, result.status_value());
      driver_test().ShutdownAndDestroyDriver();
    }
  }

  fdf_testing::BackgroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

  fidl::SyncClient<frtc::Device>& client() { return client_; }

 private:
  fdf_testing::BackgroundDriverTest<TestConfig> driver_test_;
  fidl::SyncClient<frtc::Device> client_;
};

using UnmanagedRtcDriverTest = BaseTest<false>;
using ManagedRtcDriverTest = BaseTest<true>;

TEST_F(UnmanagedRtcDriverTest, TestDfv2StartStop) {
  driver_test().RunInEnvironmentTypeContext([](SimpleDriverTestEnvironment& env) {
    auto csr_read = [&](MockI2c::TransferRequest& req, MockI2c::TransferCompleter::Sync& comp) {
      std::vector<uint8_t> testc_clear{0};
      comp.Reply(zx::ok(fi2c::DeviceTransferResponse{{.read_data = {std::move(testc_clear)}}}));
    };

    auto time0_read = [&](MockI2c::TransferRequest& req, MockI2c::TransferCompleter::Sync& comp) {
      std::vector<uint8_t> date{0, 0, 0, 1, 0, 1, 0};  // 1900-01-01T00:00:00
      comp.Reply(zx::ok(fi2c::DeviceTransferResponse{{.read_data = {std::move(date)}}}));
    };
    EXPECT_CALL(env.i2c(), Transfer).WillOnce(csr_read).WillOnce(time0_read);
  });

  zx::result<> result = driver_test().StartDriver();
  ASSERT_EQ(ZX_OK, result.status_value());

  result = driver_test().StopDriver();
  ASSERT_EQ(ZX_OK, result.status_value());
  driver_test().ShutdownAndDestroyDriver();
}

TEST_F(UnmanagedRtcDriverTest, TestPorClearLogic) {
  driver_test().RunInEnvironmentTypeContext([](SimpleDriverTestEnvironment& env) {
    auto read_action = [&](MockI2c::TransferRequest& req, MockI2c::TransferCompleter::Sync& comp) {
      // TESTC-bit set.
      std::vector<uint8_t> testc_clear{0x08};  // TESTC-bit cleared.
      comp.Reply(zx::ok(fi2c::DeviceTransferResponse{{.read_data = {std::move(testc_clear)}}}));
    };

    auto reset_action = [&](MockI2c::TransferRequest& req, MockI2c::TransferCompleter::Sync& comp) {
      fi2c::Transaction& txn = req.transactions()[0];
      std::vector<uint8_t> data = txn.data_transfer()->write_data().value();

      // 1900-01-01T00:00:00
      EXPECT_EQ(0x02, data[0]);
      EXPECT_EQ(0x00, data[1]);
      EXPECT_EQ(0x00, data[2]);
      EXPECT_EQ(0x00, data[3]);
      EXPECT_EQ(0x01, data[4]);
      EXPECT_EQ(0x00, data[5]);
      EXPECT_EQ(0x01, data[6]);
      EXPECT_EQ(0x00, data[7]);

      fi2c::DeviceTransferResponse resp;
      comp.Reply(zx::ok(std::move(resp)));
    };

    auto clear_action = [&](MockI2c::TransferRequest& req, MockI2c::TransferCompleter::Sync& comp) {
      fi2c::Transaction& txn = req.transactions()[0];
      std::vector<uint8_t> data = txn.data_transfer()->write_data().value();

      EXPECT_EQ(0x00, data[0]);
      EXPECT_EQ(0x00, data[1]);

      fi2c::DeviceTransferResponse resp;
      comp.Reply(zx::ok(std::move(resp)));
    };

    auto time0_action = [&](MockI2c::TransferRequest& req, MockI2c::TransferCompleter::Sync& comp) {
      std::vector<uint8_t> date{0, 0, 0, 1, 0, 1, 0};  // 1900-01-01T00:00:00
      comp.Reply(zx::ok(fi2c::DeviceTransferResponse{{.read_data = {std::move(date)}}}));
    };

    EXPECT_CALL(env.i2c(), Transfer)
        .WillOnce(read_action)
        .WillOnce(reset_action)
        .WillOnce(clear_action)
        .WillOnce(time0_action);
  });

  zx::result<> result = driver_test().StartDriver();
  ASSERT_EQ(ZX_OK, result.status_value());

  result = driver_test().StopDriver();
  ASSERT_EQ(ZX_OK, result.status_value());
  driver_test().ShutdownAndDestroyDriver();
}

TEST_F(UnmanagedRtcDriverTest, TestNonsenseDateStartup) {
  driver_test().RunInEnvironmentTypeContext([](SimpleDriverTestEnvironment& env) {
    auto read_action = [&](MockI2c::TransferRequest& req, MockI2c::TransferCompleter::Sync& comp) {
      std::vector<uint8_t> testc_clear{0};
      comp.Reply(zx::ok(fi2c::DeviceTransferResponse{{.read_data = {std::move(testc_clear)}}}));
    };

    auto time0_action = [&](MockI2c::TransferRequest& req, MockI2c::TransferCompleter::Sync& comp) {
      std::vector<uint8_t> date{0, 0, 0, 0, 0, 0, 0};  // 1900-0-0T00:00:00
      comp.Reply(zx::ok(fi2c::DeviceTransferResponse{{.read_data = {std::move(date)}}}));
    };

    auto write_action = [&](MockI2c::TransferRequest& req,
                            MockI2c::TransferCompleter::Sync& comp) mutable {
      fi2c::Transaction& txn = req.transactions()[0];
      std::vector<uint8_t> data = txn.data_transfer()->write_data().value();

      // 1900-01-01T00:00:00
      EXPECT_EQ(0x02, data[0]);
      EXPECT_EQ(0x00, data[1]);
      EXPECT_EQ(0x00, data[2]);
      EXPECT_EQ(0x00, data[3]);
      EXPECT_EQ(0x01, data[4]);
      EXPECT_EQ(0x00, data[5]);
      EXPECT_EQ(0x01, data[6]);
      EXPECT_EQ(0x00, data[7]);

      fi2c::DeviceTransferResponse resp;
      comp.Reply(zx::ok(std::move(resp)));
    };

    EXPECT_CALL(env.i2c(), Transfer)
        .WillOnce(read_action)
        .WillOnce(time0_action)
        .WillOnce(write_action);
  });

  zx::result<> result = driver_test().StartDriver();
  ASSERT_EQ(ZX_OK, result.status_value());

  result = driver_test().StopDriver();
  ASSERT_EQ(ZX_OK, result.status_value());
  driver_test().ShutdownAndDestroyDriver();
}

TEST_F(ManagedRtcDriverTest, TestGetFailsOnUpsteramError) {
  driver_test().RunInEnvironmentTypeContext([](SimpleDriverTestEnvironment& env) {
    auto action = [&](MockI2c::TransferRequest& req, MockI2c::TransferCompleter::Sync& comp) {
      comp.Reply(zx::error(ZX_ERR_NO_MEMORY));
    };

    EXPECT_CALL(env.i2c(), Transfer).WillOnce(action);
  });

  fidl::Result result = client()->Get();
  ASSERT_TRUE(result.is_error());
  ASSERT_TRUE(result.error_value().is_domain_error());
  ASSERT_EQ(ZX_ERR_NO_MEMORY, result.error_value().domain_error());
}

TEST_F(ManagedRtcDriverTest, TestSetFailsOnUpsteramError) {
  driver_test().RunInEnvironmentTypeContext([](SimpleDriverTestEnvironment& env) {
    auto action = [&](MockI2c::TransferRequest& req, MockI2c::TransferCompleter::Sync& comp) {
      comp.Reply(zx::error(ZX_ERR_NO_MEMORY));
    };

    EXPECT_CALL(env.i2c(), Transfer).WillOnce(action);
  });

  frtc::Time time{{.seconds = 0, .minutes = 0, .hours = 0, .day = 1, .month = 1, .year = 1900}};

  fidl::Result result = client()->Set2(time);
  ASSERT_TRUE(result.is_error());
  EXPECT_EQ(ZX_ERR_NO_MEMORY, result.error_value().domain_error());
}

typedef struct {
  // The description here will get rendered in the test name, along with a stringified variant of
  // the testing input. In total, it can be used to identify each individual test case in the suite.
  const char* test_desc;

  // Expected test result, true if ZX_OK is expected. If false, expected status is defined by the
  // test. E.g. set-based tests result in a status of ZX_ERR_OUT_OF_RANGE while get-based tests
  // result in ZX_ERR_INTERNAL.
  bool zx_ok;

  // Test input.
  int year;
  int month;
  int day;
  int hours;
  int minutes;
  int seconds;
} Param;

// clang-format off
const auto kCases = testing::Values(
    // Some good cases just to exercise the various codepaths.
    Param{"GoodDate", true, 1986, 9,  17, 4,  22, 0},
    Param{"GoodDate", true, 1985, 5,  5,  16, 45, 0},
    Param{"GoodDate", true, 1955, 11, 5,  1,  20, 0},
    Param{"GoodDate", true, 1990, 12, 26, 8,  0,  0},
    Param{"GoodDate", true, 1900, 1,  2,  3,  4,  5},
    Param{"GoodDate", true, 2000, 1,  2,  3,  4,  5},
    // All tests to follow are based on 1900-01-01T00:00:00 and then modified accordingly.
    //
    // Century bounds checking.
    Param{"Year", false, 1899, 1, 1, 0, 0, 0},
    Param{"Year", true,  1900, 1, 1, 0, 0, 0},
    Param{"Year", true,  2099, 1, 1, 0, 0, 0},
    Param{"Year", false, 2100, 1, 1, 0, 0, 0},
    // Leap-year bounds checking.
    Param{"Leap", false, 1901, 2, 29, 0, 0, 0},
    Param{"Leap", true,  1904, 2, 29, 0, 0, 0},
    Param{"Leap", true,  2000, 2, 29, 0, 0, 0},
    Param{"Leap", false, 2001, 2, 29, 0, 0, 0},
    Param{"Leap", true,  2004, 2, 29, 0, 0, 0},
    Param{"Leap", true,  2024, 2, 29, 0, 0, 0},
    // Month bounds checking.
    Param{"Month", false, 1900, 0,  1, 0, 0, 0},
    Param{"Month", true,  1900, 1,  1, 0, 0, 0},
    Param{"Month", true,  1900, 12, 1, 0, 0, 0},
    Param{"Month", false, 1900, 13, 1, 0, 0, 0},
    // 28-day bounds checking.
    Param{"Day28", false, 1901, 2, 0,  0, 0, 0},
    Param{"Day28", true,  1901, 2, 1,  0, 0, 0},
    Param{"Day28", true,  1901, 2, 28, 0, 0, 0},
    Param{"Day28", false, 1901, 2, 29, 0, 0, 0},
    // 29-day bounds checking.
    Param{"Day29", false, 1900, 2, 0,  0, 0, 0},
    Param{"Day29", true,  1900, 2, 1,  0, 0, 0},
    Param{"Day29", true,  1900, 2, 29, 0, 0, 0},
    Param{"Day29", false, 1900, 2, 30, 0, 0, 0},
    // 31-day bounds checking.
    Param{"Day31", false, 1900, 1,  0,  0, 0, 0},
    Param{"Day31", true,  1900, 1,  1,  0, 0, 0},
    Param{"Day31", true,  1900, 1,  31, 0, 0, 0},
    Param{"Day31", true,  1900, 3,  31, 0, 0, 0},
    Param{"Day31", true,  1900, 5,  31, 0, 0, 0},
    Param{"Day31", true,  1900, 7,  31, 0, 0, 0},
    Param{"Day31", true,  1900, 8,  31, 0, 0, 0},
    Param{"Day31", true,  1900, 10, 31, 0, 0, 0},
    Param{"Day31", true,  1900, 12, 31, 0, 0, 0},
    Param{"Day31", false, 1900, 2,  31, 0, 0, 0},
    Param{"Day31", false, 1900, 4,  31, 0, 0, 0},
    Param{"Day31", false, 1900, 6,  31, 0, 0, 0},
    Param{"Day31", false, 1900, 9,  31, 0, 0, 0},
    Param{"Day31", false, 1900, 11, 31, 0, 0, 0},
    // Hours bounds checking.
    Param{"Hours", true,  1900, 1, 1, 0,  0, 0},
    Param{"Hours", true,  1900, 1, 1, 23, 0, 0},
    Param{"Hours", false, 1900, 1, 1, 24, 0, 0},
    // Minutes bounds checking.
    Param{"Minutes", true,  1900, 1, 1, 0, 0,  0},
    Param{"Minutes", true,  1900, 1, 1, 0, 59, 0},
    Param{"Minutes", false, 1900, 1, 1, 0, 60, 0},
    // Seconds bounds checking.
    Param{"Seconds", true,  1900, 1, 1, 0, 0, 0},
    Param{"Seconds", true,  1900, 1, 1, 0, 0, 59},
    Param{"Seconds", false, 1900, 1, 1, 0, 0, 60});
// clang-format on

using Parameterized = BaseTest<true, testing::TestWithParam<Param>>;

TEST_P(Parameterized, TestSet) {
  Param param{GetParam()};

  driver_test().RunInEnvironmentTypeContext([&](SimpleDriverTestEnvironment& env) {
    auto action = [&](MockI2c::TransferRequest& req, MockI2c::TransferCompleter::Sync& comp) {
      fi2c::Transaction& txn = req.transactions()[0];
      std::vector<uint8_t> data = txn.data_transfer()->write_data().value();

      EXPECT_EQ(0x02, data[0]);  // I2c time register.
      EXPECT_EQ(to_bcd(param.seconds), data[1]);
      EXPECT_EQ(to_bcd(param.minutes), data[2]);
      EXPECT_EQ(to_bcd(param.hours), data[3]);
      EXPECT_EQ(to_bcd(param.day), data[4]);
      // data[5] day-of-week is unused.
      uint8_t century_bit = param.year > 1999 ? 1 : 0;
      EXPECT_EQ(century_bit << 7 | to_bcd(param.month), data[6]);
      EXPECT_EQ(to_bcd(param.year - (century_bit ? 2000 : 1900)), data[7]);

      fi2c::DeviceTransferResponse resp;
      comp.Reply(zx::ok(std::move(resp)));
    };

    if (param.zx_ok) {
      // Note that Set() short-circuits on invalid dates before calling into i2c. If the return
      // value is ZX_ERR_OUT_OF_RANGE, the parent protocol won't be invoked (i.e. no mock
      // invocation to record).
      EXPECT_CALL(env.i2c(), Transfer).WillOnce(action);
    }
  });

  // Shove the testing input into a frtc::Time in preparation of invoking Set().
  frtc::Time time{{.seconds = static_cast<uint8_t>(param.seconds),
                   .minutes = static_cast<uint8_t>(param.minutes),
                   .hours = static_cast<uint8_t>(param.hours),
                   .day = static_cast<uint8_t>(param.day),
                   .month = static_cast<uint8_t>(param.month),
                   .year = static_cast<uint16_t>(param.year)}};

  fidl::Result result = client()->Set2(time);

  if (param.zx_ok) {
    ASSERT_TRUE(result.is_ok());
  } else {
    ASSERT_TRUE(result.is_error());
    EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, result.error_value().domain_error());
  }
}

TEST_P(Parameterized, TestGet) {
  Param param{GetParam()};

  if (param.year < 1900 || param.year > 2099) {
    // There is no sequence of bytes that can represent years outside the operable range. Skip
    // any get-based tests with nonsensical year values. Nonsensical month, day, hour, minute,
    // and second values will be validated.
    GTEST_SKIP();
  }

  driver_test().RunInEnvironmentTypeContext([&](SimpleDriverTestEnvironment& env) {
    auto action = [&](MockI2c::TransferRequest& req, MockI2c::TransferCompleter::Sync& comp) {
      uint8_t century_bit = param.year > 1999 ? 1 : 0;
      int year = param.year - (century_bit ? 2000 : 1900);

      fi2c::DeviceTransferResponse resp{{
          .read_data = {std::vector<uint8_t>{
              to_bcd(param.seconds),
              to_bcd(param.minutes),
              to_bcd(param.hours),
              to_bcd(param.day),
              0,  // Weekday unused.
              static_cast<uint8_t>(century_bit << 7 | to_bcd(param.month)),
              to_bcd(year),
          }},
      }};
      comp.Reply(zx::ok(std::move(resp)));
    };

    EXPECT_CALL(env.i2c(), Transfer).WillOnce(action);
  });

  fidl::Result result = client()->Get();
  if (param.zx_ok) {
    ASSERT_TRUE(result.is_ok());
    EXPECT_EQ(param.year, result.value().rtc().year());
    EXPECT_EQ(param.month, result.value().rtc().month());
    EXPECT_EQ(param.day, result.value().rtc().day());
    EXPECT_EQ(param.hours, result.value().rtc().hours());
    EXPECT_EQ(param.minutes, result.value().rtc().minutes());
    EXPECT_EQ(param.seconds, result.value().rtc().seconds());
  } else {
    ASSERT_TRUE(result.is_error());
    EXPECT_EQ(ZX_ERR_OUT_OF_RANGE, result.error_value().domain_error());
  }
}

// clang-format off
INSTANTIATE_TEST_SUITE_P(
    RtcDriverTest,
    Parameterized,
    kCases,
    [](const testing::TestParamInfo<Parameterized::ParamType>& info) {
      // Of the form i_TestDesc_OK_1900_01_01T00_00_00, only alphanumeric and underscores supported.
      // The index is simply the i'th row in the Values above to ensure unique test names (a
      // requirement of GTest).
      std::stringstream test_name;

      test_name << info.index << "_"
          << info.param.test_desc << "_"
          << (info.param.zx_ok ? "OK_" : "ERR_")
          << info.param.year << "_" << info.param.month << "_" << info.param.day
          << "T" << info.param.hours << "_" << info.param.minutes << "_" << info.param.seconds;

      return test_name.str();
    });
// clang-format on
