// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/libfuzzer/stats.h"

#include <gtest/gtest.h>

#include "src/lib/files/file.h"
#include "src/lib/fxl/strings/split_string.h"

namespace fuzzing {

using fuchsia::fuzzer::ProcessStats;

TEST(LibFuzzerStatsTest, ParseTestFuzzerStats) {
  std::string contents;
  ASSERT_TRUE(files::ReadFileToString("/pkg/data/test_fuzzer.stderr", &contents));
  auto lines = SplitString(contents, "\n", fxl::kTrimWhitespace, fxl::kSplitWantAll);

  Status status;
  size_t num_inits = 0;
  size_t num_news = 0;
  size_t num_reduces = 0;
  for (auto line : lines) {
    UpdateReason reason;
    if (!ParseLibFuzzerStats(line, &reason, &status)) {
      continue;
    }
    // Check lines with unique reasons. Count lines with repeated reasons.
    // This log represents an almost immediate crash, and has no non-zero exec/s.
    switch (reason) {
      case UpdateReason::INIT:
        ++num_inits;
        EXPECT_EQ(status.runs(), 2U);
        EXPECT_EQ(status.covered_pcs(), 2U);
        EXPECT_EQ(status.covered_features(), 2U);
        EXPECT_EQ(status.corpus_num_inputs(), 1U);
        EXPECT_EQ(status.corpus_total_size(), 1U);
        EXPECT_FALSE(status.has_elapsed());
        EXPECT_EQ(status.process_stats()[0].koid, ZX_KOID_INVALID);
        EXPECT_EQ(status.process_stats()[0].mem_private_bytes, 35U * kOneMb);
        break;
      case UpdateReason::NEW:
        ++num_news;
        EXPECT_FALSE(status.has_elapsed());
        break;
      case UpdateReason::REDUCE:
        ++num_reduces;
        EXPECT_FALSE(status.has_elapsed());
        break;
      default:
        break;
    }
  }
  EXPECT_EQ(num_inits, 1U);
  EXPECT_EQ(num_news, 5U);
  EXPECT_EQ(num_reduces, 2U);
}

TEST(LibFuzzerStatsTest, ParseCertFuzzerStats) {
  std::string contents;
  ASSERT_TRUE(files::ReadFileToString("/pkg/data/cert_fuzzer.stderr", &contents));
  auto lines = SplitString(contents, "\n", fxl::kTrimWhitespace, fxl::kSplitWantAll);

  Status status;
  size_t num_inits = 0;
  size_t num_news = 0;
  size_t num_reduces = 0;
  zx_duration_t previous_elapsed = 0;
  for (auto line : lines) {
    UpdateReason reason;
    if (!ParseLibFuzzerStats(line, &reason, &status)) {
      continue;
    }
    // Check lines with unique reasons. Count lines with repeated reasons.
    switch (reason) {
      case UpdateReason::INIT:
        ++num_inits;
        EXPECT_EQ(status.runs(), 1306U);
        EXPECT_EQ(status.covered_pcs(), 2560U);
        EXPECT_EQ(status.covered_features(), 9711U);
        EXPECT_EQ(status.corpus_num_inputs(), 418U);
        EXPECT_EQ(status.corpus_total_size(), 490U * kOneKb);
        EXPECT_FALSE(status.has_elapsed());
        EXPECT_EQ(status.process_stats()[0].koid, ZX_KOID_INVALID);
        EXPECT_EQ(status.process_stats()[0].mem_private_bytes, 83U * kOneMb);
        break;
      case UpdateReason::NEW:
        ++num_news;
        break;
      case UpdateReason::PULSE:
        EXPECT_EQ(status.runs(), 8192U);
        EXPECT_EQ(status.covered_pcs(), 2564U);
        EXPECT_EQ(status.covered_features(), 9760U);
        EXPECT_EQ(status.corpus_num_inputs(), 443U);
        EXPECT_EQ(status.corpus_total_size(), 533U * kOneKb);
        EXPECT_GE(status.elapsed(), 3000732600LL);  // (8192 runs * 1e9 s/ns) / (2730 exec/s)
        EXPECT_EQ(status.process_stats()[0].koid, ZX_KOID_INVALID);
        EXPECT_EQ(status.process_stats()[0].mem_private_bytes, 187U * kOneMb);
        break;
      case UpdateReason::REDUCE:
        ++num_reduces;
        break;
      case UpdateReason::DONE:
        EXPECT_EQ(status.runs(), 10000U);
        EXPECT_EQ(status.covered_pcs(), 2564U);
        EXPECT_EQ(status.covered_features(), 9761U);
        EXPECT_EQ(status.corpus_num_inputs(), 444U);
        EXPECT_EQ(status.corpus_total_size(), 533 * kOneKb);
        EXPECT_GE(status.elapsed(), 3000300030LL);  // (10000 runs * 1e9 ns/s) / (3333 exec/ns)
        EXPECT_EQ(status.process_stats()[0].koid, ZX_KOID_INVALID);
        EXPECT_EQ(status.process_stats()[0].mem_private_bytes, 212U * kOneMb);
        break;
      default:
        ADD_FAILURE() << "Unexpected update reason: " << reason;
        break;
    }
    if (status.has_elapsed()) {
      // This log includes non-zero exec/s. `elapsed` should monotonically increase.
      EXPECT_GE(status.elapsed(), previous_elapsed);
      previous_elapsed = status.elapsed();
    }
  }
  EXPECT_EQ(num_inits, 1U);
  EXPECT_EQ(num_news, 26U);
  EXPECT_EQ(num_reduces, 12U);
  EXPECT_NE(previous_elapsed, 0U);
}

TEST(LibFuzzerStatsTest, Format) {
  Status status;
  status.set_runs(123);  // 41 * 3
  status.set_covered_pcs(456);
  status.set_covered_features(789);
  status.set_corpus_num_inputs(1111);
  status.set_corpus_total_size(222 * kOneKb);
  status.set_elapsed(41 * kOneSecond);

  auto line = FormatLibFuzzerStats(UpdateReason::INIT, status);
  EXPECT_EQ(line, "#123\tINITED cov: 456 ft: 789 corp: 1111/222Kb exec/s: 3 rss: 0Mb");

  status.set_runs(168);  // 42 * 4
  status.set_elapsed(42 * kOneSecond);
  auto* process_stats = status.mutable_process_stats();
  process_stats->resize(10);
  line = FormatLibFuzzerStats(UpdateReason::NEW, status);
  EXPECT_EQ(line, "#168\tNEW    cov: 456 ft: 789 corp: 1111/222Kb exec/s: 4 rss: 0Mb");

  status.set_runs(215);  // 43 * 5
  status.set_elapsed(43 * kOneSecond);
  for (size_t i = 0; i < 10; ++i) {
    auto& stats = (*process_stats)[i];
    stats.mem_mapped_bytes = (i + 10) * kOneMb;
    stats.mem_private_bytes = i * kOneMb;
    stats.mem_shared_bytes = 10 * kOneMb;
    stats.mem_scaled_shared_bytes = kOneMb;
  }
  line = FormatLibFuzzerStats(UpdateReason::PULSE, status);
  EXPECT_EQ(line, "#215\tpulse  cov: 456 ft: 789 corp: 1111/222Kb exec/s: 5 rss: 55Mb");

  status.set_runs(264);  // 44 * 6
  status.set_covered_pcs(500);
  status.set_covered_features(1000);
  status.set_corpus_num_inputs(2048);
  status.set_corpus_total_size(1 * kOneMb);
  status.set_elapsed(44 * kOneSecond);
  line = FormatLibFuzzerStats(UpdateReason::REDUCE, status);
  EXPECT_EQ(line, "#264\tREDUCE cov: 500 ft: 1000 corp: 2048/1Mb exec/s: 6 rss: 55Mb");

  status.set_runs(315);  // 45 * 5
  status.set_elapsed(45 * kOneSecond);
  (*process_stats)[0].mem_private_bytes += 20 * kOneMb;
  line = FormatLibFuzzerStats(UpdateReason::DONE, status);
  EXPECT_EQ(line, "#315\tDONE   cov: 500 ft: 1000 corp: 2048/1Mb exec/s: 7 rss: 75Mb");
}

TEST(LibFuzzerStatsTest, RoundTrip) {
  Status in_status;
  in_status.set_runs(11111);
  in_status.set_covered_pcs(22);
  in_status.set_covered_features(333);
  in_status.set_corpus_num_inputs(44);
  in_status.set_corpus_total_size(555 * kOneKb);
  in_status.set_elapsed(168348484848);  // (11111 runs * 1e9 ns/s) / (66 exec/s)
  std::vector<ProcessStats> process_stats(1);
  process_stats[0].mem_mapped_bytes = 77 * kOneMb;
  in_status.set_process_stats(std::move(process_stats));

  UpdateReason reasons[] = {
      UpdateReason::INIT,   UpdateReason::NEW,  UpdateReason::PULSE,
      UpdateReason::REDUCE, UpdateReason::DONE,
  };
  for (auto in_reason : reasons) {
    auto line = FormatLibFuzzerStats(in_reason, in_status);
    Status out_status;
    UpdateReason out_reason;
    EXPECT_TRUE(ParseLibFuzzerStats(line, &out_reason, &out_status));
    EXPECT_EQ(in_reason, out_reason);
    EXPECT_EQ(in_status.runs(), out_status.runs());
    EXPECT_EQ(in_status.covered_pcs(), out_status.covered_pcs());
    EXPECT_EQ(in_status.covered_features(), out_status.covered_features());
    EXPECT_EQ(in_status.corpus_num_inputs(), out_status.corpus_num_inputs());
    EXPECT_EQ(in_status.corpus_total_size(), out_status.corpus_total_size());
    EXPECT_EQ(in_status.elapsed(), out_status.elapsed());
    EXPECT_EQ(in_status.process_stats()[0].mem_private_bytes,
              out_status.process_stats()[0].mem_private_bytes);
  }
}

}  // namespace fuzzing
