// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/starnix/tests/selinux/userspace/audit_checker.h"

#include <sys/socket.h>

#include <algorithm>
#include <regex>
#include <set>
#include <sstream>
#include <string>
#include <utility>
#include <vector>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/audit.h>
#include <linux/netlink.h>
#include <rapidjson/schema.h>
#include <rapidjson/stringbuffer.h>
#include <rapidjson/writer.h>

#include "src/lib/fxl/strings/join_strings.h"
#include "src/lib/fxl/strings/split_string.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/lib/json_parser/json_parser.h"
#include "src/starnix/tests/selinux/userspace/audit_utils.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

constexpr int kTabSize = 4;

// Buffer size for reading from the netlink socket.
constexpr int kNetlinkBufSize = 4096;

constexpr char kSuccessKey[] = "audit_success";
constexpr char kExpectedFailureKey[] = "audit_failure";
constexpr char kSkipKey[] = "audit_skip";
constexpr char kTestNameKey[] = "name";
constexpr char kTestAuditExpectationsKey[] = "audit_expectations";
constexpr char kExpectationsFile[] = "data/audit_expectations/audit_expectations.json";

const std::regex kAuditLogRegex = std::regex(
    R"(avc:\s+(denied|granted)\s+\{\s*([^}]+)\s*\}.*scontext=([^ ]+)\s+tcontext=([^ ]+)\s+tclass=([^ ]+).*)");

void EscapeAuditLog(std::string& audit_log) {
  size_t i = 0;
  while (i < audit_log.size()) {
    if (audit_log.at(i) == '"') {
      audit_log.insert(i, 1, '\\');
      i += 2;
      continue;
    }
    i += 1;
  }
}

void PrintWithTab(int multiplier, const char* format, ...) {
  printf("%*s", multiplier * kTabSize, "");
  va_list args;
  va_start(args, format);
  vprintf(format, args);
  va_end(args);
}

fit::result<std::string, AuditChecker::AuditLogEntry> ParseAuditLogString(const std::string& line) {
  std::smatch matches;
  if (std::regex_search(line, matches, kAuditLogRegex) && matches.size() == 6) {
    auto perms =
        fxl::SplitStringCopy(matches[2].str(), " ", fxl::WhiteSpaceHandling::kTrimWhitespace,
                             fxl::SplitResult::kSplitWantNonEmpty);
    bool permissive = strstr(line.c_str(), "permissive=1") != 0;
    return fit::ok(
        AuditChecker::AuditLogEntry{.denied = matches[1].str() == "denied",
                                    .permission = std::set<std::string>{perms.begin(), perms.end()},
                                    .scontext = matches[3].str(),
                                    .tcontext = matches[4].str(),
                                    .tclass = matches[5].str(),
                                    .permissive = permissive});
  }
  return fit::error("Failed to parse audit log string: " + line);
}

void ExpectationsToJSON(std::vector<std::string> logs, const std::string& test_name) {
  printf("\n{\n");
  PrintWithTab(1, "\"%s\": \"%s\",\n", kTestNameKey, test_name.c_str());
  PrintWithTab(1, "\"%s\": [\n", kTestAuditExpectationsKey);
  for (int i = 0; i < (int)logs.size(); i++) {
    EscapeAuditLog(logs[i]);
    PrintWithTab(2, "\"%s\"", logs[i].c_str());
    if (i != (int)logs.size() - 1) {
      printf(",");
    }
    printf("\n");
  }
  PrintWithTab(1, "]\n");
  printf("}\n");
}

void SendStartSentinel() {
  auto fork_helper = test_helper::ForkHelper();
  fork_helper.OnlyWaitForForkedChildren();
  pid_t child_pid = fork_helper.RunInForkedProcess([&] {
    fbl::unique_fd fd = OpenNetlinkAuditSocket();
    if (!fd.is_valid()) {
      exit(2);
    }
    auto send_res = SendUserAuditMessage(fd.get(), 1, "SENTINEL_START");
    if (send_res.is_error()) {
      fprintf(stderr, "Start sentinel send error %s\n", strerror(send_res.error_value()));
      exit(3);
    }
    exit(0);
  });
  if (child_pid < 0 || !fork_helper.WaitForChildren()) {
    fprintf(stderr, "Start sentinel child error\n");
  }
}

void SendEndSentinel() {
  auto fork_helper = test_helper::ForkHelper();
  fork_helper.OnlyWaitForForkedChildren();
  pid_t child_pid = fork_helper.RunInForkedProcess([&] {
    fbl::unique_fd fd = OpenNetlinkAuditSocket();
    if (!fd.is_valid()) {
      exit(2);
    }
    auto send_res = SendUserAuditMessage(fd.get(), 1, "SENTINEL_END");
    if (send_res.is_error()) {
      fprintf(stderr, "End sentinel send error %s\n", strerror(send_res.error_value()));
      exit(3);
    }
    exit(0);
  });
  if (child_pid < 0 || !fork_helper.WaitForChildren()) {
    fprintf(stderr, "End sentinel end child error\n");
  }
}

void DrainAuditLog() {
  SendEndSentinel();
  fbl::unique_fd fd = OpenNetlinkAuditSocket();
  if (!fd.is_valid() || RegisterAsAuditDaemon(fd.get(), getpid()).is_error()) {
    return;
  }

  char buf[kNetlinkBufSize];
  // Drain the audit backlog until there is no other message.
  while (true) {
    auto recv_res = ReceiveNetlinkMessage(fd.get(), buf, sizeof(buf));
    if (recv_res.is_error() && recv_res.error_value() == EINTR) {
      continue;
    }
    if (recv_res.value() < NLMSG_LENGTH(0)) {
      fprintf(stderr, "audit_drainer: message too short\n");
      continue;
    }
    struct nlmsghdr* header = reinterpret_cast<struct nlmsghdr*>(buf);
    if (header->nlmsg_type != AUDIT_USER_AVC) {
      continue;
    }
    std::string message((char*)NLMSG_DATA(header),
                        (char*)NLMSG_DATA(header) + (recv_res.value() - sizeof(*header)));
    if (header->nlmsg_type == AUDIT_USER_AVC) {
      if (!strstr(message.c_str(), "SENTINEL_END")) {
        continue;
      }
      break;
    }
  }
  if (UnregisterAuditDaemon(fd.get()).is_error()) {
    printf("Failed to unregister\n");
    return;
  }
}

}  // namespace

AuditChecker::AuditChecker() {
  // Use `ADD_FAILURE` here to trigger an environment setup failure.
  if (!ParseExpectationsFile(kExpectationsFile)) {
    ADD_FAILURE() << "Failed to parse audit expectations file.";
  }
}

AuditChecker* AuditChecker::with_json_generation() {
  auto checker = new AuditChecker;
  checker->generate_json_ = true;
  return checker;
}

bool AuditChecker::ParseExpectationsFile(const std::string& file_path) {
  json_parser::JSONParser parser;
  rapidjson::Document doc = parser.ParseFromFile(file_path);

  if (parser.HasError()) {
    return false;
  }
  if (!doc.IsObject() || !doc.HasMember(kSuccessKey) || !doc[kSuccessKey].IsArray()) {
    return false;
  }

  if (doc.HasMember(kExpectedFailureKey) && !doc[kExpectedFailureKey].IsArray()) {
    return false;
  }
  for (const auto& failing_test : doc[kExpectedFailureKey].GetArray()) {
    if (!failing_test.IsString()) {
      return false;
    }
    expected_failure_tests_.push_back(failing_test.GetString());
  }
  if (!std::is_sorted(expected_failure_tests_.begin(), expected_failure_tests_.end())) {
    fprintf(stderr, "AuditChecker Error: %s is not sorted lexicographically.\n",
            kExpectedFailureKey);
    return false;
  }

  if (doc.HasMember(kSkipKey) && !doc[kSkipKey].IsArray()) {
    return false;
  }
  for (const auto& skipped_test : doc[kSkipKey].GetArray()) {
    if (!skipped_test.IsString()) {
      return false;
    }
    skipped_tests_.push_back(skipped_test.GetString());
  }
  if (!std::is_sorted(skipped_tests_.begin(), skipped_tests_.end())) {
    fprintf(stderr, "AuditChecker Error: %s is not sorted lexicographically.\n", kSkipKey);
    return false;
  }

  std::vector<std::string> success_test_names;
  for (const auto& test_obj : doc[kSuccessKey].GetArray()) {
    if (!test_obj.IsObject() || !test_obj.HasMember(kTestNameKey) ||
        !test_obj[kTestNameKey].IsString() || !test_obj.HasMember(kTestAuditExpectationsKey) ||
        !test_obj[kTestAuditExpectationsKey].IsArray()) {
      return false;
    }

    std::string test_name = test_obj[kTestNameKey].GetString();
    success_test_names.push_back(test_name);
    std::vector<AuditLogEntry> expected_logs;

    for (const auto& log_str_val : test_obj[kTestAuditExpectationsKey].GetArray()) {
      if (!log_str_val.IsString()) {
        return false;
      }
      std::string log_str = log_str_val.GetString();
      auto parse_result = ParseAuditLogString(log_str);
      if (parse_result.is_ok()) {
        expected_logs.push_back(parse_result.value());
      } else {
        return false;
      }
    }
    expectations_map_[test_name] = std::move(expected_logs);
  }
  if (!std::is_sorted(success_test_names.begin(), success_test_names.end())) {
    fprintf(stderr, "AuditChecker Error: %s is not sorted lexicographically.\n", kSuccessKey);
    return false;
  }
  return true;
}

bool AuditChecker::AuditLogEntry::operator==(const AuditChecker::AuditLogEntry& other) const =
    default;

bool AuditChecker::AuditLogEntry::contains(const AuditLogEntry& other) const {
  // TODO: https://fxbug.dev/449714364 - Remove after denials are generated in permissive mode.
  bool granted = !denied || permissive;
  bool other_granted = !other.denied || other.permissive;

  if (other_granted != granted || other.scontext != scontext || other.tcontext != tcontext ||
      other.tclass != tclass) {
    return false;
  }
  if (other.permission == permission) {
    return true;
  }
  return std::ranges::includes(permission, other.permission);
}

std::string AuditChecker::AuditLogEntry::ToString() const {
  auto perms_str = fxl::JoinStrings(permission, " ");
  auto permissive_str = permissive ? "1" : "0";
  return fxl::StringPrintf("avc: %s { %s } scontext=%s tcontext=%s tclass=%s permissive=%s",
                           denied ? "denied" : "granted", perms_str.c_str(), scontext.c_str(),
                           tcontext.c_str(), tclass.c_str(), permissive_str);
}

fit::result<std::string, std::vector<AuditChecker::AuditLogEntry>> AuditChecker::ReadAuditLogs(
    const std::string& test_name) {
  std::vector<std::string> raw_logs;
  std::vector<AuditLogEntry> parsed_logs;
  fbl::unique_fd fd = OpenNetlinkAuditSocket();
  if (!fd.is_valid()) {
    return fit::error("Failed to open NETLINK_AUDIT socket");
  }
  auto register_result = RegisterAsAuditDaemon(fd.get(), getpid());
  if (register_result.is_error()) {
    return fit::error("Failed to register as daemon: " +
                      std::to_string(register_result.error_value()));
  }

  bool checked_start = false;
  char buf[kNetlinkBufSize];
  // Drain the audit backlog until there is no other message.
  while (true) {
    auto recv_res = ReceiveNetlinkMessage(fd.get(), buf, sizeof(buf));
    if (recv_res.is_error() && recv_res.error_value() == EINTR) {
      continue;
    }
    if (recv_res.value() < NLMSG_LENGTH(0)) {
      fprintf(stderr, "audit_listener: message too short\n");
      continue;
    }

    struct nlmsghdr* header = (struct nlmsghdr*)buf;
    if (header->nlmsg_type != AUDIT_AVC && header->nlmsg_type != AUDIT_USER_AVC) {
      continue;
    }
    std::string message((char*)NLMSG_DATA(header),
                        (char*)NLMSG_DATA(header) + (recv_res.value() - sizeof(*header)));
    if (header->nlmsg_type == AUDIT_USER_AVC) {
      if (!checked_start && strstr(message.c_str(), "SENTINEL_START")) {
        checked_start = true;
        continue;
      }
      if (!strstr(message.c_str(), "SENTINEL_END")) {
        continue;
      }
      break;
    }
    // Parse the audit string
    auto parse_result = ParseAuditLogString(message);
    if (parse_result.is_ok()) {
      const auto& entry = parse_result.value();

      // If there is a AUDIT_AVC log before the start sentinel, it means
      // there is a policy violation outside of the current test.
      // The message should be checked, it can be from another test that
      // should include an audit expectation.
      if (!checked_start) {
        fprintf(stderr, "Found AUDIT_AVC log before start sentinel: %s\n", message.c_str());
        continue;
      }
      if (generate_json_) {
        raw_logs.push_back(message);
      }
      fprintf(stderr, "%s\n", message.c_str());
      parsed_logs.push_back(entry);
    } else {
      ADD_FAILURE() << "Failed to parse AUDIT_AVC message: " << parse_result.error_value()
                    << "\nMessage: " << message;
    }
  }
  if (UnregisterAuditDaemon(fd.get()).is_error()) {
    fprintf(stderr, "Failed to unregister\n");
    return fit::ok(parsed_logs);
  }
  if (!checked_start) {
    fprintf(stderr, "Did not find start sentinel\n");
  }
  if (generate_json_) {
    current_test_raw_logs_ = std::move(raw_logs);
  }
  return fit::ok(parsed_logs);
}

bool AuditChecker::ShouldOnlyDrainAudits(const std::string& test_name) const {
  auto found = std::ranges::find(skipped_tests_, test_name);
  return found != skipped_tests_.end();
}

bool AuditChecker::IsExpectedToFail(const std::string& test_name) const {
  if (!test_helper::IsStarnix()) {
    return false;
  }

  auto found = std::ranges::find(expected_failure_tests_, test_name);
  return found != expected_failure_tests_.end();
}

void AuditChecker::OnTestSuiteEnd(const testing::TestSuite& test_suite) {
  if (generate_json_) {
    // Add a sleep to allow the Linux kernel to print syslog messages before
    // printing any audit JSON objects.
    sleep(1);
    for (auto test_logs : current_test_suite_raw_logs_) {
      const auto& logs = std::get<0>(test_logs);
      const auto& test_name = std::get<1>(test_logs);
      if (!logs.empty() || expectations_map_.count(test_name) > 0) {
        ExpectationsToJSON(logs, test_name);
      }
    }
    current_test_suite_raw_logs_.clear();
  }
}

void AuditChecker::OnTestStart(const testing::TestInfo& test_info) {
  auto test_name = fxl::StringPrintf("%s.%s", test_info.test_suite_name(), test_info.name());
  if (ShouldOnlyDrainAudits(test_name)) {
    return;
  }
  SendStartSentinel();
}

void AuditChecker::OnTestEnd(const testing::TestInfo& test_info) {
  auto test_name = fxl::StringPrintf("%s.%s", test_info.test_suite_name(), test_info.name());
  // If the audit log checking should be skipped, drain the audit logs.
  // This can be used for tests that fail on Starnix or do not produce any
  // useful logs.
  if (ShouldOnlyDrainAudits(test_name)) {
    DrainAuditLog();
    return;
  }
  // Skip the audit check if the test is skipped at runtime.
  if (::testing::Test::IsSkipped()) {
    DrainAuditLog();
    return;
  }
  SendEndSentinel();
  bool result = CheckAuditExpectations(test_name);
  if (!result && generate_json_) {
    // Only update the JSON file if the test fails: upon success, there are no meaningful audit
    // updates.
    current_test_suite_raw_logs_.emplace_back(std::move(current_test_raw_logs_), test_name);
  }
}

bool AuditChecker::CheckAuditExpectations(const std::string& test_name) {
  auto read_result = ReadAuditLogs(test_name);
  if (read_result.is_error()) {
    ADD_FAILURE() << read_result.error_value();
    return false;
  }

  // Fetch the current expectations, if any, for `test_name`.
  auto it = expectations_map_.find(test_name);
  auto expected_logs = it != expectations_map_.end() ? it->second : std::vector<AuditLogEntry>();
  auto actual_logs = read_result.value();

  if (test_helper::IsStarnix()) {
    // TODO: Introduce an explicit ignore-audit-logs scope so that permissive logs can be compared.
    std::erase_if(expected_logs, [](auto& x) { return x.permissive; });
    std::erase_if(actual_logs, [](auto& x) { return !x.denied || x.permissive; });
  }

  // Expectations are generally expected to match in-order, so iterate over the two lists to
  // emit each line with a prefix indicating the difference between the
  // observed and expected lists.
  std::string audit_diff;
  auto actual_it = actual_logs.begin();
  auto expected_it = expected_logs.begin();
  bool audit_logs_match = true;

  while (actual_it != actual_logs.end() && expected_it != expected_logs.end()) {
    if (test_helper::IsStarnix()) {
      // Account for Starnix only checking, and audit logging, one permission
      // at at time, where Linux may check several in a single operation.
      if (expected_it->contains(*actual_it)) {
        // Observed audit log matches at least one permission in the next expectation, so emit it
        // with no prefix.
        if (expected_it->permission == actual_it->permission) {
          // Move to next expectation in case of exact match.
          expected_it++;
        } else if (actual_it->denied) {
          // Move to next expectation, if any, in case of partial match denied access, since the
          // first denial will prevent the subsequent checks being made.
          expected_it++;
        } else {
          // If the expectation is granted (whether permissive or denied) then we expect the other
          // permissions to also be checked, so just remove the matched permission.
          std::string found_perm = *actual_it->permission.begin();
          if (expected_it->permission.erase(found_perm) != 1u) {
            ADD_FAILURE() << "Expected to erase exactly 1 permission, but failed.";
            return false;
          }
        }
        audit_diff += "\n " + actual_it->ToString();
        actual_it++;
        continue;
      }
    } else {
      // Linux is the baseline, so expect audit logs to match line for line.
      if (*expected_it == *actual_it) {
        audit_diff += "\n " + actual_it->ToString();
        actual_it++;
        expected_it++;
        continue;
      }
    }

    audit_logs_match = false;

    if (auto it = std::find_if(
            expected_it, expected_logs.end(),
            [actual_it](const AuditLogEntry& expected) { return expected.contains(*actual_it); });
        it != expected_logs.end()) {
      // Items don't match, but the observed audit log matches an expectation that appears later.
      // Emit all the intervening expected logs with a prefix indicating they're missing.
      for (; !expected_it->contains(*actual_it); expected_it++) {
        audit_diff += "\n-" + expected_it->ToString();
      }
    } else {
      // Item doesn't appear in the expected audit logs at all.
      audit_diff += "\n+" + actual_it->ToString();
      actual_it++;
    }
  }

  for (; expected_it != expected_logs.end(); expected_it++) {
    audit_diff += "\n-" + expected_it->ToString();
    audit_logs_match = false;
  }
  for (; actual_it != actual_logs.end(); actual_it++) {
    audit_diff += "\n+" + actual_it->ToString();
    audit_logs_match = false;
  }

  bool expect_success = !IsExpectedToFail(test_name);
  if (audit_logs_match == expect_success) {
    return true;
  }

  if (generate_json_) {
    return false;
  }

  if (!expect_success) {
    // Audit logs matched expectations, despite us expecting them to mismatch.
    ADD_FAILURE() << "Got matching audit logs, expected mismatch, for test: " << test_name;
    return false;
  }

  ADD_FAILURE() << "Audit logs mismatch. Expected " << expected_logs.size() << ", got "
                << actual_logs.size() << "." << std::endl
                << "Diff of observed from expected audit logs: " << audit_diff;
  return false;
}
