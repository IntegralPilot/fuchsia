// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cinttypes>
#include <sstream>

#include <fbl/string_buffer.h>
#include <fbl/string_printf.h>
#include <rapidjson/document.h>
#include <rapidjson/prettywriter.h>
#include <zxtest/base/assertion.h>
#include <zxtest/base/json-reporter.h>
#include <zxtest/base/log-sink.h>
#include <zxtest/base/test-case.h>

namespace zxtest::internal {

namespace {

fbl::String FormatTimeMs(uint64_t time_ms) {
  return fbl::StringPrintf("%" PRIu64 ".%03" PRIu64 "s", time_ms / 1000, time_ms % 1000);
}

class CustomFileSinkWriter {
 public:
  using Ch = char;

  explicit CustomFileSinkWriter(FileLogSink* sink) : sink_(sink) {}

  Ch* PutBegin() { return 0; }

  void Put(Ch c) { sink_->Write("%c", c); }
  void Flush() { sink_->Flush(); }

  size_t PutEnd(Ch*) { return 0; }

 private:
  FileLogSink* sink_;
};

void AddAssertion(rapidjson::PrettyWriter<CustomFileSinkWriter>& json, const Assertion& assertion) {
  json.StartObject();

  json.Key("failure");

  std::ostringstream message;
  message << assertion.location().filename << ":" << assertion.location().line_number
          << ": Failure: " << assertion.description();
  json.String(message.str().c_str());

  json.Key("type");
  json.String("");

  json.EndObject();
}

}  // namespace

class JsonWriter {
 public:
  explicit JsonWriter(std::unique_ptr<FileLogSink> sink)
      : sink_(std::move(sink)), file_writer_(sink_.get()), writer_(file_writer_) {}

  rapidjson::PrettyWriter<CustomFileSinkWriter>& out() { return writer_; }

 private:
  std::unique_ptr<FileLogSink> sink_;
  CustomFileSinkWriter file_writer_;
  rapidjson::PrettyWriter<CustomFileSinkWriter> writer_;
};

JsonReporter::JsonReporter(std::unique_ptr<FileLogSink> sink)
    : json_writer_(new JsonWriter(std::move(sink))) {}

JsonReporter::~JsonReporter() { delete json_writer_; }

void JsonReporter::OnProgramStart(const Runner& runner) {
  // Start a new output file.
  // All iterations would be placed under a single set of test
  // suites, but when run using the zxtest_runner we will always execute
  // a single test one time. Passing --gtest_repeat is also restricted
  // so users will not be able to trigger multiple iterations when
  // using fx test. Manual execution is not restricted.
  timers_.program.Reset();
  auto& json = json_writer_->out();
  json.StartObject();
  json.Key("name");
  json.String("AllTests");
  json.Key("testsuites");
  json.StartArray();
}

void JsonReporter::OnTestCaseStart(const TestCase& test_case) {
  ResetSuite();
  suite_entered = true;
  auto& json = json_writer_->out();
  json.StartObject();
  json.Key("name");
  json.String(test_case.name().c_str());
  json.Key("testsuite");
  json.StartArray();
}

void JsonReporter::OnTestStart(const TestCase& test_case, const TestInfo& test) {
  ResetCase();
  auto& json = json_writer_->out();
  test_entered = true;
  run_test_count++;
  suite_test_count++;

  json.StartObject();
  json.Key("name");
  json.String(test.name().c_str());
  json.Key("file");
  json.String(test.location().filename);
  json.Key("line");
  json.Uint64(test.location().line_number);
}

void JsonReporter::OnAssertion(const Assertion& assertion) {
  auto& json = json_writer_->out();
  if (!test_entered) {
    if (!suite_entered) {
      // Create a synthetic suite
      json.StartObject();
      json.Key("name");
      json.String("");
      json.Key("tests");
      json.Uint64(0);
      json.Key("failures");
      json.Uint64(0);
      json.Key("disabled");
      json.Uint64(0);
      json.Key("time");
      json.String("0.000s");
      json.Key("testsuite");
      json.StartArray();
    }
    // Create a synthetic case with the failure.
    json.StartObject();
    json.Key("name");
    json.String("");
    json.Key("file");
    json.String(assertion.location().filename);
    json.Key("line");
    json.Uint64(assertion.location().line_number);

    json.Key("status");
    json.String("RUN");
    json.Key("result");
    json.String("COMPLETED");
    json.Key("time");
    json.String("0.000s");

    json.Key("failures");
    json.StartArray();
    AddAssertion(json, assertion);
    json.EndArray();
    json.EndObject();

    if (!suite_entered) {
      json.EndArray();
      json.EndObject();
    }
    return;
  }
  if (!test_has_assertion) {
    test_has_assertion = true;
    json.Key("failures");
    json.StartArray();
  }

  AddAssertion(json, assertion);
}

void JsonReporter::OnMessage(const Message& message) {
  std::ostringstream formatted_message;
  formatted_message << message.location().filename << ":" << message.location().line_number << ": "
                    << message.text();
  test_messages_.push_back(formatted_message.str());
}

void JsonReporter::OnTestSkip(const TestCase& test_case, const TestInfo& test) {
  auto& json = json_writer_->out();

  // No need to check for list mode, since we will never call that method when listing.

  if (test_has_assertion) {
    json.EndArray();
  }

  json.Key("status");
  json.String("NOTRUN");

  json.Key("result");
  json.String("SKIPPED");

  auto format_time = FormatTimeMs(timers_.test.GetElapsedTime());
  json.Key("time");
  json.String(format_time.c_str());

  json.Key("skipped");
  json.StartArray();
  // test_messages_ are cleared in ResetCase()
  for (const auto& message : test_messages_) {
    json.StartObject();
    json.Key("message");
    json.String(message.c_str());
    json.EndObject();
  }
  json.EndArray();
  json.EndObject();

  run_skipped_count++;
  suite_skipped_count++;
  test_entered = false;
}

void JsonReporter::OnTestFailure(const TestCase& test_case, const TestInfo& test) {
  auto& json = json_writer_->out();
  if (!list_mode_) {
    if (test_has_assertion) {
      json.EndArray();
    }

    json.Key("status");
    json.String("RUN");

    json.Key("result");
    json.String("COMPLETED");

    auto format_time = FormatTimeMs(timers_.test.GetElapsedTime());
    json.Key("time");
    json.String(format_time.c_str());
  }
  json.EndObject();

  run_failure_count++;
  suite_failure_count++;
  test_entered = false;
}

void JsonReporter::OnTestSuccess(const TestCase& test_case, const TestInfo& test) {
  auto& json = json_writer_->out();
  if (!list_mode_) {
    if (test_has_assertion) {
      json.EndArray();
    }

    json.Key("status");
    json.String("RUN");

    json.Key("result");
    json.String("COMPLETED");

    auto format_time = FormatTimeMs(timers_.test.GetElapsedTime());
    json.Key("time");
    json.String(format_time.c_str());
  }
  json.EndObject();
  test_entered = false;
}

void JsonReporter::OnTestCaseEnd(const TestCase& test_case) {
  auto& json = json_writer_->out();
  json.EndArray();

  json.Key("tests");
  json.Uint64(suite_test_count);

  if (!list_mode_) {
    json.Key("failures");
    json.Uint64(suite_failure_count);

    json.Key("disabled");
    json.Uint64(suite_skipped_count);

    json.Key("time");
    auto format_time = FormatTimeMs(timers_.test_suite.GetElapsedTime());
    json.String(format_time.c_str());
  }

  json.EndObject();
  suite_entered = false;
}

void JsonReporter::OnProgramEnd(const Runner& runner) {
  auto& json = json_writer_->out();
  json.EndArray();

  json.Key("tests");
  json.Uint64(run_test_count);

  if (!list_mode_) {
    json.Key("failures");
    json.Uint64(run_failure_count);

    json.Key("disabled");
    json.Uint64(run_skipped_count);

    json.Key("time");
    auto format_time = FormatTimeMs(timers_.program.GetElapsedTime());
    json.String(format_time.c_str());
  }

  json.EndObject();
  json.Flush();
}

}  // namespace zxtest::internal
