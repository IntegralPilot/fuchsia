// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/sys/fuzzing/libfuzzer/stats.h"

#include <lib/syslog/cpp/macros.h>

#include <sstream>

#include <re2/re2.h>

#include "absl/strings/string_view.h"

namespace fuzzing {

using fuchsia::fuzzer::ProcessStats;

bool ParseLibFuzzerStats(std::string_view line, UpdateReason* reason, Status* status) {
  absl::string_view input(line);
  uint32_t runs;
  if (!re2::RE2::Consume(&input, "#(\\d+)", &runs)) {
    return false;
  }

  // Parse reason.
  std::string reason_str;
  if (!re2::RE2::Consume(&input, "\\t(\\S+)", &reason_str)) {
    return false;
  }

  // Is `runs` valid?
  uint64_t scaled_runs;
  if (runs == 0 || mul_overflow(runs, kOneSecond, &scaled_runs)) {
    return false;
  }
  status->set_runs(runs);

  *reason = UpdateReason::PULSE;  // By default, assume it's just a status update.
  if (reason_str == "INITED") {
    *reason = UpdateReason::INIT;
  } else if (reason_str == "NEW") {
    *reason = UpdateReason::NEW;
  } else if (reason_str == "REDUCE") {
    *reason = UpdateReason::REDUCE;
  } else if (reason_str == "DONE") {
    *reason = UpdateReason::DONE;
    status->set_running(false);
  }

  // Parse covered PCs.
  size_t covered_pcs;
  if (re2::RE2::FindAndConsume(&input, "cov: (\\d+)", &covered_pcs)) {
    status->set_covered_pcs(covered_pcs);
  }

  // Parse covered features.
  size_t covered_features;
  if (re2::RE2::FindAndConsume(&input, "ft: (\\d+)", &covered_features)) {
    status->set_covered_features(covered_features);
  }

  // Parse corpus stats.
  size_t corpus_num_inputs;
  if (re2::RE2::FindAndConsume(&input, "corp: (\\d+)", &corpus_num_inputs)) {
    size_t corpus_total_size;
    status->set_corpus_num_inputs(corpus_num_inputs);
    if (re2::RE2::Consume(&input, "/(\\d+)b", &corpus_total_size)) {
      status->set_corpus_total_size(corpus_total_size);
    } else if (re2::RE2::Consume(&input, "/(\\d+)Kb", &corpus_total_size)) {
      status->set_corpus_total_size(corpus_total_size * kOneKb);
    } else if (re2::RE2::Consume(&input, "/(\\d+)Mb", &corpus_total_size)) {
      status->set_corpus_total_size(corpus_total_size * kOneMb);
    }
  }

  // Best effort: libFuzzer does not track total elapsed time, but it does have number of runs and
  // runs per second.
  int64_t execs_per_second;
  if (re2::RE2::FindAndConsume(&input, "exec/s: (\\d+)", &execs_per_second)) {
    if (execs_per_second != 0 && scaled_runs < std::numeric_limits<int64_t>::max()) {
      int64_t elapsed = static_cast<int64_t>(scaled_runs) / execs_per_second;
      if (!status->has_elapsed() || status->elapsed() < elapsed) {
        status->set_elapsed(elapsed);
      }
    }
  }

  // Best effort: the libFuzzer output obviously does not have `ZX_INFO_TASK_*` details. Instead,
  // this will add a minimal `ProcessStats` with a rough estimate of memory consumed if the status
  // does not have more detailed process info. These `ProcessStats` are easily identifiable by
  // having an invalid KOID.
  if (!status->has_process_stats() ||
      (status->process_stats().size() == 1 && status->process_stats()[0].koid == ZX_KOID_INVALID)) {
    size_t rss_mb;
    if (re2::RE2::FindAndConsume(&input, "rss: (\\d+)Mb", &rss_mb)) {
      std::vector<ProcessStats> process_stats(1);
      process_stats[0].koid = ZX_KOID_INVALID;
      process_stats[0].mem_private_bytes = rss_mb * kOneMb;
      status->set_process_stats(std::move(process_stats));
    }
  }

  return true;
}

std::string FormatLibFuzzerStats(UpdateReason reason, const Status& status) {
  FX_CHECK(status.has_runs());
  int64_t runs = status.runs();

  // Since `status.runs` is a `u32`, this scaling should always succeed.
  int64_t scaled_runs;
  FX_CHECK(!mul_overflow(runs, kOneSecond, &scaled_runs));

  std::ostringstream oss;
  oss << "#" << runs << "\t";

  switch (reason) {
    case UpdateReason::INIT:
      oss << "INITED";
      break;
    case UpdateReason::NEW:
      oss << "NEW   ";
      break;
    case UpdateReason::PULSE:
      oss << "pulse ";
      break;
    case UpdateReason::REDUCE:
      oss << "REDUCE";
      break;
    case UpdateReason::DONE:
      oss << "DONE  ";
      break;
  }

  if (status.has_covered_pcs()) {
    if (auto covered_pcs = status.covered_pcs()) {
      oss << " cov: " << covered_pcs;
    }
  }

  if (status.has_covered_features()) {
    if (auto covered_features = status.covered_features()) {
      oss << " ft: " << covered_features;
    }
  }

  if (status.has_corpus_num_inputs()) {
    if (auto corpus_num_inputs = status.corpus_num_inputs()) {
      oss << " corp: " << corpus_num_inputs;
      if (status.has_corpus_total_size()) {
        auto corpus_total_size = status.corpus_total_size();
        oss << "/";
        if (corpus_total_size >= kOneMb) {
          oss << (corpus_total_size / kOneMb) << "Mb";
        } else if (corpus_total_size >= kOneKb) {
          oss << (corpus_total_size / kOneKb) << "Kb";
        } else if (corpus_total_size) {
          oss << corpus_total_size << "b";
        }
      }
    }
  }

  oss << " exec/s: ";
  if (status.has_elapsed()) {
    auto elapsed = status.elapsed();
    oss << scaled_runs / elapsed;
  } else {
    oss << 0;
  }

  oss << " rss: ";
  if (status.has_process_stats()) {
    size_t mem_bytes = 0;
    for (const auto& process_stats : status.process_stats()) {
      mem_bytes += process_stats.mem_private_bytes + process_stats.mem_scaled_shared_bytes;
    }
    oss << ((mem_bytes + kOneMb - 1) / kOneMb);
  } else {
    oss << 0;
  }
  oss << "Mb";

  return oss.str();
}

}  // namespace fuzzing
