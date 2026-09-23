// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/memory/metrics/printer.h"

#include <gtest/gtest.h>

#include "gmock/gmock.h"
#include "src/developer/memory/metrics/capture.h"
#include "src/developer/memory/metrics/summary.h"
#include "src/developer/memory/metrics/tests/test_utils.h"
#include "src/lib/fsl/socket/strings.h"
#include "src/lib/fxl/strings/split_string.h"
#include "third_party/rapidjson/include/rapidjson/document.h"
#include "third_party/rapidjson/include/rapidjson/ostreamwrapper.h"
#include "third_party/rapidjson/include/rapidjson/prettywriter.h"

namespace {
void ConfirmLines(std::ostringstream& oss, const std::vector<std::string>& expected_lines) {
  auto lines = fxl::SplitStringCopy(oss.str(), "\n", fxl::kKeepWhitespace, fxl::kSplitWantNonEmpty);
  EXPECT_THAT(lines, testing::UnorderedElementsAreArray(expected_lines));
}

std::string ExecuteWithSocket(fit::function<void(zx::socket)> generator) {
  zx::socket endpoint0, endpoint1;
  zx::socket::create(ZX_SOCKET_STREAM, &endpoint0, &endpoint1);

  std::string output;
  std::thread receiver([&]() { fsl::BlockingCopyToString(std::move(endpoint0), &output); });
  generator(std::move(endpoint1));
  receiver.join();

  return output;
}

}  // namespace

namespace rapidjson {
// Teach the testing framework to print json doc.
extern void PrintTo(const rapidjson::Document& value, ::std::ostream* os) {
  rapidjson::OStreamWrapper osw(*os);
  rapidjson::PrettyWriter writer(osw);
  value.Accept(writer);
}
}  // namespace rapidjson

namespace memory::test {

using PrinterUnitTest = testing::Test;

TEST_F(PrinterUnitTest, PrintCapture) {
  Capture c;
  TestUtils::CreateCapture(&c, {
                                   .time = 1234,
                                   .kmem =
                                       {
                                           .total_bytes = 300,
                                           .free_bytes = 100,
                                           .wired_bytes = 10,
                                           .total_heap_bytes = 20,
                                           .free_heap_bytes = 30,
                                           .vmo_bytes = 40,
                                           .mmu_overhead_bytes = 50,
                                           .ipc_bytes = 60,
                                           .other_bytes = 70,
                                       },
                                   .kmem_extended =
                                       {
                                           .total_bytes = 300,
                                           .free_bytes = 100,
                                           .wired_bytes = 10,
                                           .total_heap_bytes = 20,
                                           .free_heap_bytes = 30,
                                           .vmo_bytes = 40,
                                           .vmo_pager_total_bytes = 15,
                                           .vmo_pager_newest_bytes = 4,
                                           .vmo_pager_oldest_bytes = 8,
                                           .vmo_discardable_locked_bytes = 3,
                                           .vmo_discardable_unlocked_bytes = 7,
                                           .mmu_overhead_bytes = 50,
                                           .ipc_bytes = 60,
                                           .other_bytes = 70,
                                       },
                                   .vmos =
                                       {
                                           {
                                               .koid = 1,
                                               .name = "v1",
                                               .size_bytes = 300,
                                               .parent_koid = 100,
                                               .committed_bytes = 200,
                                               .committed_scaled_bytes = 200,
                                               .committed_fractional_scaled_bytes = 0,
                                           },
                                       },
                                   .processes =
                                       {
                                           {.koid = 100, .name = "p1", .vmos = {1}},
                                       },
                               });
  std::string output =
      ExecuteWithSocket([&](zx::socket socket) { JsonPrinter(socket).PrintCapture(c); });
  rapidjson::Document doc;
  doc.Parse(output.c_str());

  rapidjson::Document expected;
  expected.Parse(R"json(
    {
        "Time": 1234,
        "Kernel": {
            "total": 300,
            "free": 100,
            "wired": 10,
            "total_heap": 20,
            "free_heap": 30,
            "vmo": 40,
            "mmu": 50,
            "ipc": 60,
            "other": 70,
            "vmo_pager_total": 15,
            "vmo_pager_newest": 4,
            "vmo_pager_oldest": 8,
            "vmo_discardable_locked": 3,
            "vmo_discardable_unlocked": 7,
            "vmo_reclaim_disabled": 0
        },
        "Processes": [
            [
                "koid",
                "name",
                "vmos"
            ],
            [
                100,
                "p1",
                [
                    1
                ]
            ]
        ],
        "VmoNames": [
            "v1"
        ],
        "Vmos": [
            [
                "koid",
                "name",
                "parent_koid",
                "committed_bytes",
                "allocated_bytes"
            ],
            [
                1,
                0,
                100,
                200,
                300
            ]
        ]
    }
  )json");
  EXPECT_EQ(doc, expected);
}

TEST_F(PrinterUnitTest, PrintCaptureAndBucketConfig) {
  Capture c;
  TestUtils::CreateCapture(&c, {
                                   .time = 1234,
                                   .kmem =
                                       {
                                           .total_bytes = 300,
                                           .free_bytes = 100,
                                           .wired_bytes = 10,
                                           .total_heap_bytes = 20,
                                           .free_heap_bytes = 30,
                                           .vmo_bytes = 40,
                                           .mmu_overhead_bytes = 50,
                                           .ipc_bytes = 60,
                                           .other_bytes = 70,
                                       },
                                   .kmem_extended =
                                       {
                                           .total_bytes = 300,
                                           .free_bytes = 100,
                                           .wired_bytes = 10,
                                           .total_heap_bytes = 20,
                                           .free_heap_bytes = 30,
                                           .vmo_bytes = 40,
                                           .vmo_pager_total_bytes = 15,
                                           .vmo_pager_newest_bytes = 4,
                                           .vmo_pager_oldest_bytes = 8,
                                           .vmo_discardable_locked_bytes = 3,
                                           .vmo_discardable_unlocked_bytes = 7,
                                           .mmu_overhead_bytes = 50,
                                           .ipc_bytes = 60,
                                           .other_bytes = 70,
                                       },
                                   .vmos =
                                       {
                                           {
                                               .koid = 1,
                                               .name = "v1",
                                               .size_bytes = 300,
                                               .parent_koid = 100,
                                               .committed_bytes = 200,
                                               .committed_scaled_bytes = 200,
                                               .committed_fractional_scaled_bytes = 0,
                                           },
                                       },
                                   .processes =
                                       {
                                           {.koid = 100, .name = "p1", .vmos = {1}},
                                       },
                               });

  const std::string bucket_config = R"(
    [
        {
            "event_code" : 29,
            "name" : "BlobfsInactive",
            "process" : "blobfs\\.cm",
            "vmo": "inactive-blob-.*"
        }
    ]
  )";
  std::string output = ExecuteWithSocket([&](zx::socket socket) {
    JsonPrinter(socket).PrintCaptureAndBucketConfig(c, bucket_config);
  });
  rapidjson::Document doc;
  doc.Parse(output.c_str());
  ASSERT_TRUE(doc.IsObject());

  rapidjson::Value const& capture = doc["Capture"];
  rapidjson::Value const& buckets = doc["Buckets"];

  // Test the content of `capture`.
  ASSERT_TRUE(capture.IsObject());

  EXPECT_EQ(1234, capture["Time"].GetInt64());

  auto kernel = capture["Kernel"].GetObject();
  EXPECT_EQ(300U, kernel["total"].GetUint64());
  EXPECT_EQ(100U, kernel["free"].GetUint64());
  EXPECT_EQ(10U, kernel["wired"].GetUint64());
  EXPECT_EQ(20U, kernel["total_heap"].GetUint64());
  EXPECT_EQ(30U, kernel["free_heap"].GetUint64());
  EXPECT_EQ(40U, kernel["vmo"].GetUint64());
  EXPECT_EQ(50U, kernel["mmu"].GetUint64());
  EXPECT_EQ(60U, kernel["ipc"].GetUint64());
  EXPECT_EQ(70U, kernel["other"].GetUint64());
  EXPECT_EQ(15U, kernel["vmo_pager_total"].GetUint64());
  EXPECT_EQ(4U, kernel["vmo_pager_newest"].GetUint64());
  EXPECT_EQ(8U, kernel["vmo_pager_oldest"].GetUint64());
  EXPECT_EQ(3U, kernel["vmo_discardable_locked"].GetUint64());
  EXPECT_EQ(7U, kernel["vmo_discardable_unlocked"].GetUint64());

  auto processes = capture["Processes"].GetArray();
  ASSERT_EQ(2U, processes.Size());
  auto process_header = processes[0].GetArray();
  EXPECT_STREQ("koid", process_header[0].GetString());
  EXPECT_STREQ("name", process_header[1].GetString());
  EXPECT_STREQ("vmos", process_header[2].GetString());
  auto process = processes[1].GetArray();
  EXPECT_EQ(100U, process[0].GetUint64());
  EXPECT_STREQ("p1", process[1].GetString());
  auto process_vmos = process[2].GetArray();
  ASSERT_EQ(1U, process_vmos.Size());
  EXPECT_EQ(1, process_vmos[0]);

  auto vmo_names = capture["VmoNames"].GetArray();
  ASSERT_EQ(1U, vmo_names.Size());
  EXPECT_STREQ("v1", vmo_names[0].GetString());

  auto vmos = capture["Vmos"].GetArray();
  ASSERT_EQ(2U, vmos.Size());
  auto vmo_header = vmos[0].GetArray();
  EXPECT_STREQ("koid", vmo_header[0].GetString());
  EXPECT_STREQ("name", vmo_header[1].GetString());
  EXPECT_STREQ("parent_koid", vmo_header[2].GetString());
  EXPECT_STREQ("committed_bytes", vmo_header[3].GetString());
  EXPECT_STREQ("allocated_bytes", vmo_header[4].GetString());
  auto vmo = vmos[1].GetArray();
  EXPECT_EQ(1U, vmo[0].GetUint64());
  EXPECT_EQ(0U, vmo[1].GetUint64());
  EXPECT_EQ(100U, vmo[2].GetUint64());
  EXPECT_EQ(200U, vmo[3].GetUint64());
  EXPECT_EQ(300U, vmo[4].GetUint64());

  // Test the content of `buckets`.
  ASSERT_TRUE(buckets.IsArray());
  EXPECT_EQ(1U, buckets.Size());
  rapidjson::Value const& bucket = buckets[0];
  EXPECT_EQ(29U, bucket["event_code"].GetUint64());
  EXPECT_STREQ("BlobfsInactive", bucket["name"].GetString());
  EXPECT_STREQ("blobfs\\.cm", bucket["process"].GetString());
  EXPECT_STREQ("inactive-blob-.*", bucket["vmo"].GetString());
}

TEST_F(PrinterUnitTest, PrintSummaryKMEM) {
  Capture c;
  TestUtils::CreateCapture(&c, {
                                   .time = 1234,
                                   .kmem =
                                       {
                                           .total_bytes = 1024ul * 1024,
                                           .free_bytes = 1024,
                                           .wired_bytes = 2ul * 1024,
                                           .total_heap_bytes = 3ul * 1024,
                                           .free_heap_bytes = 2ul * 1024,
                                           .vmo_bytes = 5ul * 1024,
                                           .mmu_overhead_bytes = 6ul * 1024,
                                           .ipc_bytes = 7ul * 1024,
                                           .other_bytes = 8ul * 1024,
                                       },
                               });

  std::ostringstream oss;
  TextPrinter p(oss);
  Summary s(c);
  p.PrintSummary(s, CaptureLevel::KMEM, SORTED);

  ConfirmLines(oss, {
                        "Time: 1234 VMO: 5K Free: 1K",
                    });
}

TEST_F(PrinterUnitTest, PrintSummaryPROCESS) {
  Capture c;
  TestUtils::CreateCapture(&c, {
                                   .time = 1234,
                                   .kmem =
                                       {
                                           .total_bytes = 1024ul * 1024,
                                           .free_bytes = 1024,
                                           .wired_bytes = 2ul * 1024,
                                           .total_heap_bytes = 3ul * 1024,
                                           .free_heap_bytes = 2ul * 1024,
                                           .vmo_bytes = 5ul * 1024,
                                           .mmu_overhead_bytes = 6ul * 1024,
                                           .ipc_bytes = 7ul * 1024,
                                           .other_bytes = 8ul * 1024,
                                       },
                                   .vmos = {{.koid = 1,
                                             .name = "v1",
                                             .committed_bytes = 1024,
                                             .committed_scaled_bytes = 1024}},
                                   .processes = {{.koid = 100, .name = "p1", .vmos = {1}}},
                               });

  std::ostringstream oss;
  TextPrinter p(oss);
  Summary s(c);
  p.PrintSummary(s, CaptureLevel::PROCESS, SORTED);

  ConfirmLines(oss, {
                        "Time: 1234 VMO: 5K Free: 1K",
                        "kernel<1> 30K",
                        "p1<100> 1K",
                    });
}

TEST_F(PrinterUnitTest, PrintSummaryVMO) {
  Capture c;
  TestUtils::CreateCapture(&c, {
                                   .time = 1234,
                                   .kmem =
                                       {
                                           .total_bytes = 1024ul * 1024,
                                           .free_bytes = 1024,
                                           .wired_bytes = 2ul * 1024,
                                           .total_heap_bytes = 3ul * 1024,
                                           .free_heap_bytes = 2ul * 1024,
                                           .vmo_bytes = 5ul * 1024,
                                           .mmu_overhead_bytes = 6ul * 1024,
                                           .ipc_bytes = 7ul * 1024,
                                           .other_bytes = 8ul * 1024,
                                       },
                                   .vmos = {{.koid = 1,
                                             .name = "v1",
                                             .committed_bytes = 1024,
                                             .committed_scaled_bytes = 1024}},
                                   .processes = {{.koid = 100, .name = "p1", .vmos = {1}}},
                               });

  std::ostringstream oss;
  TextPrinter p(oss);
  Summary s(c);
  p.PrintSummary(s, CaptureLevel::VMO, SORTED);

  ConfirmLines(oss, {
                        "Time: 1234 VMO: 5K Free: 1K",
                        "kernel<1> 30K",
                        " other 8K",
                        " ipc 7K",
                        " mmu 6K",
                        " vmo 4K",
                        " heap 3K",
                        " wired 2K",
                        "p1<100> 1K",
                        " v1 1K",
                    });
}

TEST_F(PrinterUnitTest, PrintSummaryVMOShared) {
  Capture c;
  TestUtils::CreateCapture(&c, {
                                   .time = 1234,
                                   .kmem = {.vmo_bytes = 6ul * 1024},
                                   .vmos =
                                       {
                                           {.koid = 1,
                                            .name = "v1",
                                            .committed_bytes = 1024,
                                            .committed_scaled_bytes = 1024},
                                           {.koid = 2,
                                            .name = "v2",
                                            .committed_bytes = 2ul * 1024,
                                            .committed_scaled_bytes = 2ul * 1024},
                                           {.koid = 3,
                                            .name = "v3",
                                            .committed_bytes = 3ul * 1024,
                                            .committed_scaled_bytes = 3ul * 1024},
                                       },
                                   .processes =
                                       {
                                           {.koid = 100, .name = "p1", .vmos = {1, 2}},
                                           {.koid = 200, .name = "p2", .vmos = {2, 3}},
                                       },
                               });

  std::ostringstream oss;
  TextPrinter p(oss);
  Summary s(c);
  p.PrintSummary(s, CaptureLevel::VMO, SORTED);

  ConfirmLines(oss, {
                        "Time: 1234 VMO: 6K Free: 0B",
                        "p2<200> 3K 4K 5K",
                        " v3 3K",
                        " v2 0B 1K 2K",
                        "p1<100> 1K 2K 3K",
                        " v1 1K",
                        " v2 0B 1K 2K",
                        "kernel<1> 0B",
                    });
}

TEST_F(PrinterUnitTest, PrintDigest) {
  // Test kernel stats.
  Capture c;
  TestUtils::CreateCapture(
      &c,
      {
          .kmem =
              {
                  .total_bytes = 1000,
                  .free_bytes = 100,
                  .wired_bytes = 10,
                  .vmo_bytes = 700,
              },
          .vmos =
              {
                  {.koid = 1, .name = "a1", .committed_bytes = 100, .committed_scaled_bytes = 100},
                  {.koid = 2, .name = "b1", .committed_bytes = 200, .committed_scaled_bytes = 200},
                  {.koid = 3, .name = "c1", .committed_bytes = 300, .committed_scaled_bytes = 300},
              },
          .processes =
              {
                  {.koid = 1, .name = "p1", .vmos = {1}},
                  {.koid = 2, .name = "q1", .vmos = {2}},
              },
      });
  Digester digester({{"A", ".*", "a.*"}, {"B", ".*", "b.*"}});
  Digest d(c, &digester);
  std::ostringstream oss;
  TextPrinter p(oss);
  p.PrintDigest(d);
  ConfirmLines(oss, {"B: 200B", "A: 100B", "Undigested: 300B", "Orphaned: 100B", "Kernel: 10B",
                     "Free: 100B", "[Addl]PagerTotal: 0B", "[Addl]PagerNewest: 0B",
                     "[Addl]PagerOldest: 0B", "[Addl]DiscardableLocked: 0B",
                     "[Addl]DiscardableUnlocked: 0B", "[Addl]ZramCompressedBytes: 0B"});
}

TEST_F(PrinterUnitTest, PrintSummaryWithControlCharacters) {
  Capture c;
  TestUtils::CreateCapture(
      &c, {
              .time = 1234,
              .kmem =
                  {
                      .total_bytes = 1024ul * 1024,
                      .free_bytes = 1024,
                      .wired_bytes = 2ul * 1024,
                      .total_heap_bytes = 3ul * 1024,
                      .free_heap_bytes = 2ul * 1024,
                      .vmo_bytes = 5ul * 1024,
                      .mmu_overhead_bytes = 6ul * 1024,
                      .ipc_bytes = 7ul * 1024,
                      .other_bytes = 8ul * 1024,
                  },
              .vmos = {{.koid = 1,
                        .name = "v1\n\r\t\\\"\x01\x7f",
                        .committed_bytes = 1024,
                        .committed_scaled_bytes = 1024}},
              .processes = {{.koid = 100, .name = "p1\n\r\t\\\"\x01\x7f", .vmos = {1}}},
          });

  std::ostringstream oss;
  TextPrinter p(oss);
  Summary s(c);
  p.PrintSummary(s, CaptureLevel::VMO, SORTED);

  ConfirmLines(oss, {
                        "Time: 1234 VMO: 5K Free: 1K",
                        "kernel<1> 30K",
                        " other 8K",
                        " ipc 7K",
                        " mmu 6K",
                        " vmo 4K",
                        " heap 3K",
                        " wired 2K",
                        "p1\\n\\r\\t\\\\\\\"\\x01\\x7f<100> 1K",
                        " v1\\n\\r\\t\\\\\\\"\\x01\\x7f 1K",
                    });
}

TEST_F(PrinterUnitTest, FormatSize) {
  struct TestCase {
    uint64_t bytes;
    const char* val;
  };
  std::vector<TestCase> tests = {
      {.bytes = 0, .val = "0B"},
      {.bytes = 1, .val = "1B"},
      {.bytes = 1023, .val = "1023B"},
      {.bytes = 1024, .val = "1K"},
      {.bytes = 1025, .val = "1K"},
      {.bytes = 1029, .val = "1K"},
      {.bytes = 1124, .val = "1.1K"},
      {.bytes = 1536, .val = "1.5K"},
      {.bytes = 2047, .val = "2K"},
      {.bytes = 1024ul * 1024, .val = "1M"},
      {.bytes = 1024ul * 1024 * 1024, .val = "1G"},
      {.bytes = 1024UL * 1024 * 1024 * 1024, .val = "1T"},
      {.bytes = 1024UL * 1024 * 1024 * 1024 * 1024, .val = "1P"},
      {.bytes = 1024UL * 1024 * 1024 * 1024 * 1024 * 1024, .val = "1E"},
      {.bytes = 1024UL * 1024 * 1024 * 1024 * 1024 * 1024 * 1024, .val = "0B"},
  };
  for (const auto& test : tests) {
    char buf[kMaxFormattedStringSize];
    EXPECT_STREQ(test.val, FormatSize(test.bytes, buf));
  }
}

}  // namespace memory::test
