// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/performance/lib/trace_converters/chromium_exporter.h"

#include <fstream>
#include <sstream>
#include <vector>

#include <gtest/gtest.h>
#include <trace-reader/records.h>

namespace {

TEST(ChromiumExporterTest, ValidUtf8) {
  trace::EventData data(trace::EventData::Instant{trace::EventScope::kGlobal});
  std::vector<trace::Argument> arguments;
  arguments.push_back(trace::Argument("arg", trace::ArgumentValue::MakeString("foo\xb5\xb3")));
  trace::Record record(trace::Record::Event{1000, trace::ProcessThread(45, 46), "c\342\202at",
                                            "n\301a\205me", std::move(arguments), std::move(data)});

  // Enclosing the exporter in its own scope ensures that its
  // cleanup routines are called by the destructor before the
  // output stream is read. This way, we can obtain the full
  // output rather than a truncated version.
  {
    const std::filesystem::path& output = "/tmp/test-trace-valid-utf8.json";
    tracing::ChromiumExporter exporter(output);
    exporter.ExportRecord(record);
  }

  std::stringstream buffer;

  std::ifstream input("/tmp/test-trace-valid-utf8.json");

  buffer << input.rdbuf();

  EXPECT_EQ(buffer.str(),
            "{\"displayTimeUnit\":\"ns\",\"traceEvents\":[{\"cat\":\"c\uFFFDat\","
            "\"name\":\"n\uFFFDa\uFFFDme\",\"ts\":1.0,\"pid\":45,\"tid\":46,\"ph\":"
            "\"i\",\"s\":\"g\",\"args\":{\"arg\":\"foo\uFFFD\uFFFD\"}}"
            "],\"systemTraceEvents\":{\"type\":\"fuchsia\",\"events\":[]}}");
}

TEST(ChromiumExporterTest, UnknownLargeBlobEventDropped) {
  std::vector<trace::Argument> arguments;
  arguments.push_back(trace::Argument("arg", trace::ArgumentValue::MakeString("foo")));
  static const char blob[] = "some test blob data";
  trace::Record record(trace::LargeRecordData{trace::LargeRecordData::BlobEvent{
      "category",
      "no::UnknownName",
      1000,
      trace::ProcessThread(45, 46),
      std::move(arguments),
      blob,
      sizeof(blob),
  }});

  // Enclosing the exporter in its own scope ensures that its
  // cleanup routines are called by the destructor before the
  // output stream is read. This way, we can obtain the full
  // output rather than a truncated version.
  {
    std::string file_name = "/tmp/test-trace-unknow-blob.json";
    tracing::ChromiumExporter exporter(file_name);
    exporter.ExportRecord(record);
  }

  std::stringstream buffer;

  std::ifstream input("/tmp/test-trace-unknow-blob.json");

  buffer << input.rdbuf();

  EXPECT_EQ(buffer.str(),
            "{\"displayTimeUnit\":\"ns\",\"traceEvents\":["
            "],\"systemTraceEvents\":{\"type\":\"fuchsia\",\"events\":[]}}");
}

TEST(ChromiumExporterTest, UnknownLargeBlobAttachmentDropped) {
  static const char blob[] = "some test blob data";
  trace::Record record(trace::LargeRecordData{trace::LargeRecordData::BlobAttachment{
      "category",
      "no::UnknownName",
      blob,
      sizeof(blob),
  }});

  // Enclosing the exporter in its own scope ensures that its
  // cleanup routines are called by the destructor before the
  // output stream is read. This way, we can obtain the full
  // output rather than a truncated version.
  {
    const std::filesystem::path& output = "/tmp/test-trace-dropped-blob.json";
    tracing::ChromiumExporter exporter(output);
    exporter.ExportRecord(record);
  }
  std::stringstream buffer;

  std::ifstream input("/tmp/test-trace-dropped-blob.json");

  buffer << input.rdbuf();

  EXPECT_EQ(buffer.str(),
            "{\"displayTimeUnit\":\"ns\",\"traceEvents\":["
            "],\"systemTraceEvents\":{\"type\":\"fuchsia\",\"events\":[]}}");
}

TEST(ChromiumExporterTest, FidlBlobExported) {
  static const char blob[] = "some test blob data";
  trace::Record record(trace::LargeRecordData{trace::LargeRecordData::BlobEvent{
      "fidl:blob",
      "BlobName",
      1000,
      trace::ProcessThread(45, 46),
      std::vector<trace::Argument>(),
      blob,
      sizeof(blob),
  }});

  // Enclosing the exporter in its own scope ensures that its
  // cleanup routines are called by the destructor before the
  // output stream is read. This way, we can obtain the full
  // output rather than a truncated version.
  {
    const std::filesystem::path& file_name = "/tmp/test-trace-fidl-blob.json";
    tracing::ChromiumExporter exporter(file_name);
    exporter.ExportRecord(record);
  }

  std::stringstream buffer;

  std::ifstream input("/tmp/test-trace-fidl-blob.json");

  buffer << input.rdbuf();

  EXPECT_EQ(buffer.str(),
            "{\"displayTimeUnit\":\"ns\",\"traceEvents\":[{\"ph\":\"O\",\"id\":\"\",\"cat\":\"fidl:"
            "blob\",\"name\":\"BlobName\",\"ts\":1.0,\"pid\":45,\"tid\":46,\"blob\":"
            "\"c29tZSB0ZXN0IGJsb2IgZGF0YQA=\"}],\"systemTraceEvents\":{\"type\":\"fuchsia\","
            "\"events\":[]}}");
}

TEST(ChromiumExporterTest, EmptyTrace) {
  // Enclosing the exporter in its own scope ensures that its
  // cleanup routines are called by the destructor before the
  // output stream is read. This way, we can obtain the full
  // output rather than a truncated version.
  {
    const std::filesystem::path& file_name = "/tmp/test-trace-empty-trace.json";
    tracing::ChromiumExporter exporter(file_name);
  }

  std::stringstream buffer;

  std::ifstream input("/tmp/test-trace-empty-trace.json");
  buffer << input.rdbuf();

  EXPECT_EQ(buffer.str(),
            "{\"displayTimeUnit\":\"ns\",\"traceEvents\":["
            "],\"systemTraceEvents\":{\"type\":\"fuchsia\",\"events\":[]}}");
}

TEST(ChromiumExporterTest, EmptyTraceSplit) {
  const std::filesystem::path main_output = "/tmp/test-trace-empty-split.json";
  const std::filesystem::path system_output = "/tmp/test-trace-empty-split.system.json";
  // Enclosing the exporter in its own scope ensures that its
  // cleanup routines are called by the destructor before the
  // output stream is read. This way, we can obtain the full
  // output rather than a truncated version.
  {
    tracing::ChromiumExporter exporter(main_output, system_output);
  }

  std::stringstream main_buffer;
  std::ifstream main_input(main_output);
  main_buffer << main_input.rdbuf();
  EXPECT_EQ(main_buffer.str(), "{\"displayTimeUnit\":\"ns\",\"traceEvents\":[]}");

  std::stringstream system_buffer;
  std::ifstream system_input(system_output);
  system_buffer << system_input.rdbuf();
  EXPECT_EQ(system_buffer.str(), "");
}

TEST(ChromiumExporterTest, SplitOutput) {
  const std::filesystem::path main_output = "/tmp/test-trace-split.json";
  const std::filesystem::path system_output = "/tmp/test-trace-split.system.json";

  // Records to export
  trace::Record process_kobj(trace::Record::KernelObject{
      .koid = 123, .object_type = ZX_OBJ_TYPE_PROCESS, .name = "process-name", .arguments = {}});
  std::vector<trace::Argument> thread_args;
  thread_args.emplace_back("process", trace::ArgumentValue::MakeKoid(123));
  trace::Record thread_kobj(trace::Record::KernelObject{.koid = 456,
                                                        .object_type = ZX_OBJ_TYPE_THREAD,
                                                        .name = "thread-name",
                                                        .arguments = std::move(thread_args)});
  trace::EventData event_data(trace::EventData::Instant{trace::EventScope::kGlobal});
  trace::Record event_record(trace::Record::Event{.timestamp = 1000,
                                                  .process_thread = trace::ProcessThread(123, 456),
                                                  .category = "category",
                                                  .name = "name",
                                                  .arguments = {},
                                                  .data = event_data});
  trace::Record scheduler_record(
      trace::Record::SchedulerEvent(trace::Record::SchedulerEvent::LegacyContextSwitch(
          2000,                           /*timestamp*/
          0,                              /*cpu_number*/
          trace::ThreadState::kSuspended, /*outgoing_thread_state*/
          trace::ProcessThread(1, 2),     /*outgoing_thread*/
          trace::ProcessThread(3, 4),     /*incoming_thread*/
          5,                              /*outgoing_thread_priority*/
          10                              /*incoming_thread_priority*/
          )));

  // Scope for exporter
  {
    tracing::ChromiumExporter exporter(main_output, system_output);

    // Main pass
    exporter.ExportRecord(process_kobj);
    exporter.ExportRecord(thread_kobj);
    exporter.ExportRecord(event_record);
    exporter.ExportRecord(scheduler_record);  // Ignored in main pass

    // Scheduler pass
    exporter.StartSchedulerPass();
    exporter.ExportRecord(process_kobj);      // Ignored in scheduler pass
    exporter.ExportRecord(thread_kobj);       // Ignored in scheduler pass
    exporter.ExportRecord(event_record);      // Ignored in scheduler pass
    exporter.ExportRecord(scheduler_record);  // Emitted in scheduler pass
  }

  // Read and verify main output
  std::stringstream main_buffer;
  std::ifstream main_input(main_output);
  main_buffer << main_input.rdbuf();
  EXPECT_EQ(main_buffer.str(),
            "{\"displayTimeUnit\":\"ns\",\"traceEvents\":[{\"cat\":\"category\","
            "\"name\":\"name\",\"ts\":1.0,\"pid\":123,\"tid\":456,\"ph\":\"i\",\"s\":\"g\"}]}");

  // Read and verify system output
  std::ifstream system_input(system_output);

  std::string line;
  std::vector<std::string> lines;
  while (std::getline(system_input, line)) {
    lines.push_back(line);
  }
  ASSERT_EQ(lines.size(), 3UL);
  EXPECT_EQ(lines[0], "{\"ph\":\"p\",\"pid\":123,\"name\":\"process-name\"}");
  EXPECT_EQ(lines[1], "{\"ph\":\"t\",\"pid\":123,\"tid\":456,\"name\":\"thread-name\"}");
  EXPECT_EQ(lines[2],
            "{\"ph\":\"k\",\"ts\":2.0,\"cpu\":0,\"out\":{\"pid\":1,\"tid\":2,\"state\":2,"
            "\"prio\":5},\"in\":{\"pid\":3,\"tid\":4,\"prio\":10}}");
}

}  // namespace
