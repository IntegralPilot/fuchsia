// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/analytics_event.h"

#include <stdlib.h>

#include <gtest/gtest.h>

#include "src/developer/debug/zxdb/client/remote_api_test.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/common/scoped_test_env.h"

namespace zxdb {

class AnalyticsEventTest : public RemoteAPITest {};

TEST_F(AnalyticsEventTest, DoNotLeakUsername) {
  const std::string kUsername = "BobAlice";
  ScopedTestEnv env;

  ASSERT_EQ(env.Set("USER", kUsername), 0);

  // A valid verb with an invalid argument that contains a username should be anonymized.
  CommandReport report;
  report.verb = "jump";
  report.arguments.emplace_back("/path/home/" + kUsername);

  CommandEvent event("1234");
  event.FromCommandReport(report);

  ASSERT_TRUE(event.parameters_opt());
  auto parameters = *event.parameters_opt();

  EXPECT_NE(parameters.find("argument0"), parameters.end());
  EXPECT_STREQ(std::get<std::string>(parameters.find("argument0")->second).c_str(),
               "/path/home/$USER");

  // An invalid verb prints an error message with the text verbatim from the command line, which
  // should also be anonymized. This is also important for testing that the replacement works
  // properly. The string could be enclosed in escaped double quotes by different error paths, and
  // if the username appears immediately preceding an escaped double quote, the rest of the string
  // following that would be lost without careful handling.
  report = CommandReport();
  report.err = Err("Invalid command \"pasted/with/" + kUsername + "\" is not a valid verb.");
  event = CommandEvent("6789");
  event.FromCommandReport(report);

  ASSERT_TRUE(event.parameters_opt());
  parameters = *event.parameters_opt();

  EXPECT_NE(parameters.find("error_message"), parameters.end());
  EXPECT_STREQ(std::get<std::string>(parameters.find("error_message")->second).c_str(),
               "Invalid command \"pasted/with/$USER\" is not a valid verb.");

  // Edge cases.
  report = CommandReport();
  report.err = Err(kUsername + " error.");
  event = CommandEvent("1357");
  event.FromCommandReport(report);

  ASSERT_TRUE(event.parameters_opt());
  parameters = *event.parameters_opt();

  EXPECT_NE(parameters.find("error_message"), parameters.end());
  EXPECT_STREQ(std::get<std::string>(parameters.find("error_message")->second).c_str(),
               "$USER error.");

  report = CommandReport();
  report.err = Err("Error: " + kUsername);
  event = CommandEvent("0246");
  event.FromCommandReport(report);

  ASSERT_TRUE(event.parameters_opt());
  parameters = *event.parameters_opt();

  EXPECT_NE(parameters.find("error_message"), parameters.end());
  EXPECT_STREQ(std::get<std::string>(parameters.find("error_message")->second).c_str(),
               "Error: $USER");

  report = CommandReport();
  report.err = Err(kUsername);
  event = CommandEvent("9876");
  event.FromCommandReport(report);

  ASSERT_TRUE(event.parameters_opt());
  parameters = *event.parameters_opt();

  EXPECT_NE(parameters.find("error_message"), parameters.end());
  EXPECT_STREQ(std::get<std::string>(parameters.find("error_message")->second).c_str(), "$USER");

  // The username should be escaped every time that it appears in the string.
  report = CommandReport();
  report.err = Err(kUsername + " " + kUsername);
  event = CommandEvent("4321");
  event.FromCommandReport(report);

  ASSERT_TRUE(event.parameters_opt());
  parameters = *event.parameters_opt();

  EXPECT_NE(parameters.find("error_message"), parameters.end());
  EXPECT_STREQ(std::get<std::string>(parameters.find("error_message")->second).c_str(),
               "$USER $USER");

  report = CommandReport();
  report.err = Err(kUsername + kUsername);
  event = CommandEvent("1000");
  event.FromCommandReport(report);

  ASSERT_TRUE(event.parameters_opt());
  parameters = *event.parameters_opt();

  EXPECT_NE(parameters.find("error_message"), parameters.end());
  EXPECT_STREQ(std::get<std::string>(parameters.find("error_message")->second).c_str(),
               "$USER$USER");
}
}  // namespace zxdb
