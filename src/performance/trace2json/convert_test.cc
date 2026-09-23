// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/trace2json/convert.h"

#include <gtest/gtest.h>
#include <rapidjson/document.h>
#include <rapidjson/istreamwrapper.h>
#include <src/lib/files/file.h>

#if defined(__APPLE__)
#include <mach-o/dyld.h>
#endif

#include <filesystem>

namespace {

using ::testing::Test;

std::string GetSelfPath() {
  std::string result;
#if defined(__APPLE__)
  // Executable path can have relative references ("..") depending on how the
  // app was launched.
  uint32_t length = 0;
  _NSGetExecutablePath(nullptr, &length);
  result.resize(length);
  _NSGetExecutablePath(&result[0], &length);
  result.resize(length - 1);  // Length included terminator.
#elif defined(__linux__)
  // The realpath() call below will resolve the symbolic link.
  result.assign("/proc/self/exe");
#else
#error Write this for your platform.
#endif

  char fullpath[PATH_MAX];
  return std::string(realpath(result.c_str(), fullpath));
}

std::string GetTestDataPath() {
  std::string path = GetSelfPath();
  size_t last_slash = path.rfind('/');
  if (last_slash == std::string::npos) {
    path = "./";
  } else {
    path.resize(last_slash + 1);
  }
  return path + "test_data/trace2json/";
}

void ConvertAndCompare(const ConvertSettings& settings, const std::string& expected_output_file) {
  ASSERT_TRUE(ConvertTrace(settings));
  std::string actual_out, expected_out;
  EXPECT_TRUE(files::ReadFileToString(settings.output_file_name, &actual_out));
  EXPECT_TRUE(files::ReadFileToString(expected_output_file, &expected_out));

  // Not using EXPECT_EQ here as the trace files can be large, so failures create an unreasonable
  // amount of error output.
  EXPECT_TRUE(actual_out == expected_out)
      << "Files " << settings.output_file_name << " and " << expected_output_file << " differ.";
}

void ConvertAndCompareSplit(const ConvertSettings& settings,
                            const std::string& expected_output_file,
                            const std::string& expected_system_output_file) {
  ASSERT_TRUE(ConvertTrace(settings));
  std::string actual_out, expected_out;
  EXPECT_TRUE(files::ReadFileToString(settings.output_file_name, &actual_out));
  EXPECT_TRUE(files::ReadFileToString(expected_output_file, &expected_out));

  // Not using EXPECT_EQ here as the trace files can be large, so failures create an unreasonable
  // amount of error output.
  EXPECT_TRUE(actual_out == expected_out)
      << "Files " << settings.output_file_name << " and " << expected_output_file << " differ.";

  std::string actual_system_out, expected_system_out;
  EXPECT_TRUE(files::ReadFileToString(settings.system_event_output_file_name, &actual_system_out));
  EXPECT_TRUE(files::ReadFileToString(expected_system_output_file, &expected_system_out));

  EXPECT_TRUE(actual_system_out == expected_system_out)
      << "Files " << settings.system_event_output_file_name << " and "
      << expected_system_output_file << " differ.";
}

TEST(ConvertTest, SimpleTrace) {
  // simple_trace.fxt is a small hand-written trace file that exercises a few
  // basic event types (currently slice begin, slice end, slice complete, async
  // begin, and async end), and includes both inline and table referenced
  // strings. It only contains one provider.
  std::string test_data_path = GetTestDataPath();
  ConvertSettings settings;
  settings.input_file_name = test_data_path + "simple_trace.fxt";
  settings.output_file_name = test_data_path + "simple_trace_actual.json";
  ConvertAndCompare(settings, test_data_path + "simple_trace_expected.json");
}

TEST(ConvertTest, SplitOutputTrace) {
  std::string test_data_path = GetTestDataPath();
  ConvertSettings settings;
  settings.input_file_name = test_data_path + "simple_trace.fxt";
  settings.output_file_name = test_data_path + "simple_trace_split_main_actual.json";
  settings.system_event_output_file_name =
      test_data_path + "simple_trace_split_system_actual.jsonlines";
  ConvertAndCompareSplit(settings, test_data_path + "simple_trace_split_main_expected.json",
                         test_data_path + "simple_trace_split_system_expected.jsonlines");
}

TEST(ConvertTest, ExampleBenchmark) {
  // example_benchmark.fxt is the trace written by the program in
  // garnet/examples/benchmark, in this case run on qemu. To collect the trace,
  // include //src/examples/benchmark in your build and then run:
  // ffx trace start --duration 10 --categories "benchmark"
  // ffx component run /core/ffx-laboratory:benchmark
  // "fuchsia-pkg://fuchsia.com/benchmark#meta/benchmark.cm"

  std::string test_data_path = GetTestDataPath();
  ConvertSettings settings;
  settings.input_file_name = test_data_path + "example_benchmark.fxt";
  settings.output_file_name = test_data_path + "example_benchmark_actual.json";
  ConvertAndCompare(settings, test_data_path + "example_benchmark_expected.json");
}

TEST(ConvertTest, DotStarPatternFiltering) {
  std::string test_data_path = GetTestDataPath();
  ConvertSettings settings;
  settings.input_file_name = test_data_path + "simple_trace.fxt";
  settings.output_file_name = test_data_path + "simple_trace_multi_filtered_actual.json";
  settings.patterns.push_back(".*");
  ConvertAndCompare(settings, test_data_path + "simple_trace_expected.json");
}

TEST(ConvertTest, PatternFiltering) {
  std::string test_data_path = GetTestDataPath();
  ConvertSettings settings;
  settings.input_file_name = test_data_path + "simple_trace.fxt";
  settings.output_file_name = test_data_path + "simple_trace_filtered_actual.json";
  settings.patterns.push_back(".*_ref");
  ConvertAndCompare(settings, test_data_path + "simple_trace_filtered_expected.json");
}

TEST(ConvertTest, ExactPatternFiltering) {
  std::string test_data_path = GetTestDataPath();
  ConvertSettings settings;
  settings.input_file_name = test_data_path + "simple_trace.fxt";
  settings.output_file_name = test_data_path + "simple_trace_exact_filtered_actual.json";
  settings.patterns.push_back("begin_end_ref");
  ConvertAndCompare(settings, test_data_path + "simple_trace_exact_filtered_expected.json");
}

TEST(ConvertTest, MultiPatternFiltering) {
  std::string test_data_path = GetTestDataPath();
  ConvertSettings settings;
  settings.input_file_name = test_data_path + "simple_trace.fxt";
  settings.output_file_name = test_data_path + "simple_trace_multi_filtered_actual.json";
  settings.patterns.push_back("complete.*");
  settings.patterns.push_back("async");
  ConvertAndCompare(settings, test_data_path + "simple_trace_multi_filtered_expected.json");
}

TEST(ConvertTest, MissingMagicNumber) {
  std::string test_data_path = GetTestDataPath();
  ConvertSettings settings;
  settings.input_file_name = test_data_path + "no_magic.fxt";
  settings.output_file_name = test_data_path + "no_magic.json";
  EXPECT_FALSE(ConvertTrace(settings));
  EXPECT_FALSE(std::filesystem::exists(settings.output_file_name));
}

TEST(ConvertTest, NoMatchCategoryFiltering) {
  std::string test_data_path = GetTestDataPath();
  ConvertSettings settings;
  settings.input_file_name = test_data_path + "simple_trace.fxt";
  settings.output_file_name = test_data_path + "simple_trace_no_events_actual.json";
  settings.categories.push_back("non_existent_category");
  ConvertAndCompare(settings, test_data_path + "simple_trace_no_events_expected.json");
}

TEST(ConvertTest, CategoryFiltering) {
  std::string test_data_path = GetTestDataPath();
  ConvertSettings settings;
  settings.input_file_name = test_data_path + "multi_category.fxt";
  settings.output_file_name = test_data_path + "multi_category_filtered_actual.json";
  settings.categories.push_back("test");
  settings.categories.push_back("test_2");
  ConvertAndCompare(settings, test_data_path + "multi_category_filtered_expected.json");
}

TEST(ConvertTest, CategoryOrPatternFiltering) {
  std::string test_data_path = GetTestDataPath();
  ConvertSettings settings;
  settings.input_file_name = test_data_path + "multi_category.fxt";
  settings.output_file_name = test_data_path + "multi_category_cat_or_pattern_actual.json";
  settings.categories.push_back("test");
  settings.categories.push_back("test_2");
  settings.patterns.push_back("async");
  ConvertAndCompare(settings, test_data_path + "multi_category_expected.json");
}

TEST(ConvertTest, NegativeMatchPatternNoExclusionFiltering) {
  std::string test_data_path = GetTestDataPath();
  ConvertSettings settings;
  settings.input_file_name = test_data_path + "multi_category.fxt";
  settings.output_file_name = test_data_path + "multi_category_no_exclusion_actual.json";
  settings.categories.push_back("test");
  settings.categories.push_back("test_2");
  settings.patterns.push_back(".*_ref");
  ConvertAndCompare(settings, test_data_path + "multi_category_filtered_expected.json");
}

}  // namespace
