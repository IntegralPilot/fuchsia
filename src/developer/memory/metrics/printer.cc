// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/memory/metrics/printer.h"

#include <lib/trace/event.h>
#include <zircon/types.h>

#include <algorithm>
#include <cstdint>
#include <cstdio>
#include <string_view>
#include <unordered_map>
#include <unordered_set>

#include "lib/trace/internal/event_common.h"
#include "src/lib/fsl/socket/strings.h"
#include "third_party/rapidjson/include/rapidjson/document.h"
#include "third_party/rapidjson/include/rapidjson/rapidjson.h"
#include "third_party/rapidjson/include/rapidjson/writer.h"

namespace {

// TODO(https://fxbug.dev/42136089): replace with std::saturate_cast when available.
rapidjson::SizeType safecast(size_t v) {
  static_assert(std::numeric_limits<rapidjson::SizeType>::min() <=
                std::numeric_limits<size_t>::min());
  static_assert(std::numeric_limits<rapidjson::SizeType>::max() <=
                std::numeric_limits<size_t>::max());
  return static_cast<rapidjson::SizeType>(
      std::clamp<size_t>(v, std::numeric_limits<rapidjson::SizeType>::min(),
                         std::numeric_limits<rapidjson::SizeType>::max()));
}

class SocketWriteStream {
 public:
  using Ch = char;

  explicit SocketWriteStream(zx::socket& socket) : socket_(socket) {}

  void Put(Ch c) {
    if (buffer_len_ == sizeof(buffer_)) {
      Flush();
    }

    buffer_[buffer_len_++] = c;
  }

  void Flush() {
    fsl::BlockingCopyFromString(std::string_view(buffer_, buffer_len_), socket_);
    buffer_len_ = 0;
  }

 private:
  FXL_DISALLOW_COPY_AND_ASSIGN(SocketWriteStream);

  zx::socket& socket_;

  char buffer_[16 * 1024];
  size_t buffer_len_ = 0;
};

rapidjson::Document DocumentFromCapture(const memory::Capture& capture) {
  TRACE_DURATION("memory_metrics", "JsonPrinter::DocumentFromCapture");
  rapidjson::Document j(rapidjson::kObjectType);
  auto& a = j.GetAllocator();
  j.AddMember("Time", capture.time(), a);

  rapidjson::Value kernel(rapidjson::kObjectType);
  const auto& k = capture.kmem();
  kernel.AddMember("total", k.total_bytes, a)
      .AddMember("free", k.free_bytes, a)
      .AddMember("wired", k.wired_bytes, a)
      .AddMember("total_heap", k.total_heap_bytes, a)
      .AddMember("free_heap", k.free_heap_bytes, a)
      .AddMember("vmo", k.vmo_bytes, a)
      .AddMember("mmu", k.mmu_overhead_bytes, a)
      .AddMember("ipc", k.ipc_bytes, a)
      .AddMember("other", k.other_bytes, a);

  // Add additional kernel fields if kmem_extended is populated.
  // `kmem()` and `kmem_extended()` consistency is guaranteed.
  if (capture.kmem_extended()) {
    const auto& k_ext = capture.kmem_extended().value();
    kernel.AddMember("vmo_pager_total", k_ext.vmo_pager_total_bytes, a)
        .AddMember("vmo_pager_newest", k_ext.vmo_pager_newest_bytes, a)
        .AddMember("vmo_pager_oldest", k_ext.vmo_pager_oldest_bytes, a)
        .AddMember("vmo_discardable_locked", k_ext.vmo_discardable_locked_bytes, a)
        .AddMember("vmo_discardable_unlocked", k_ext.vmo_discardable_unlocked_bytes, a)
        .AddMember("vmo_reclaim_disabled", k_ext.vmo_reclaim_disabled_bytes, a);
  }
  j.AddMember("Kernel", kernel, a);

  if (capture.kmem_compression()) {
    rapidjson::Value kmem_stats_compression(rapidjson::kObjectType);

    const auto& k_zram = capture.kmem_compression().value();
    constexpr size_t log_time_size =
        sizeof(zx_info_kmem_stats_compression_t::pages_decompressed_within_log_time) /
        sizeof(zx_info_kmem_stats_compression_t::pages_decompressed_within_log_time[0]);
    rapidjson::Value log_time(rapidjson::kArrayType);
    log_time.Reserve(log_time_size, a);
    for (auto v : k_zram.pages_decompressed_within_log_time) {
      log_time.PushBack(v, a);
    }

    kmem_stats_compression
        .AddMember("uncompressed_storage_bytes", k_zram.uncompressed_storage_bytes, a)
        .AddMember("compressed_storage_bytes", k_zram.compressed_storage_bytes, a)
        .AddMember("compressed_fragmentation_bytes", k_zram.compressed_fragmentation_bytes, a)
        .AddMember("compression_time", k_zram.compression_time, a)
        .AddMember("decompression_time", k_zram.decompression_time, a)
        .AddMember("total_page_compression_attempts", k_zram.total_page_compression_attempts, a)
        .AddMember("failed_page_compression_attempts", k_zram.failed_page_compression_attempts, a)
        .AddMember("total_page_decompressions", k_zram.total_page_decompressions, a)
        .AddMember("compressed_page_evictions", k_zram.compressed_page_evictions, a)
        .AddMember("eager_page_compressions", k_zram.eager_page_compressions, a)
        .AddMember("memory_pressure_page_compressions", k_zram.memory_pressure_page_compressions, a)
        .AddMember("critical_memory_page_compressions", k_zram.critical_memory_page_compressions, a)
        .AddMember("pages_decompressed_unit_ns", k_zram.pages_decompressed_unit_ns, a)
        .AddMember("pages_decompressed_within_log_time", log_time, a);

    j.AddMember("kmem_stats_compression", kmem_stats_compression, a);
  }

  struct NameCount {
    std::string_view name_;
    mutable size_t count = 1;
    explicit NameCount(const char* n) : name_(n, strlen(n)) {}

    bool operator==(const NameCount& kc) const { return name_ == kc.name_; }
  };

  class NameCountHash {
   public:
    size_t operator()(const NameCount& kc) const { return std::hash<std::string_view>()(kc.name_); }
  };

  std::unordered_set<NameCount, NameCountHash> name_count;
  rapidjson::Value processes(rapidjson::kArrayType);
  {
    TRACE_DURATION("memory_metrics", "JsonPrinter::DocumentFromCapture::Processes");
    processes.Reserve(safecast(capture.koid_to_process().size()), a);
    rapidjson::Value process_header(rapidjson::kArrayType);
    processes.PushBack(process_header.PushBack("koid", a).PushBack("name", a).PushBack("vmos", a),
                       a);
    for (const auto& [_, p] : capture.koid_to_process()) {
      rapidjson::Value vmos(rapidjson::kArrayType);
      vmos.Reserve(safecast(p.vmos.size()), a);
      for (const auto& v : p.vmos) {
        vmos.PushBack(v, a);
        auto [it, inserted] = name_count.emplace(capture.koid_to_vmo().find(v)->second.name);
        if (!inserted) {
          (*it).count++;
        }
      }
      rapidjson::Value process(rapidjson::kArrayType);
      process.PushBack(p.koid, a).PushBack(rapidjson::StringRef(p.name), a).PushBack(vmos, a);
      processes.PushBack(process, a);
    }
  }
  std::unordered_map<std::string_view, size_t> name_to_index;
  rapidjson::Value vmo_names(rapidjson::kArrayType);
  rapidjson::Value vmos(rapidjson::kArrayType);
  {
    TRACE_DURATION("memory_metrics", "JsonPrinter::DocumentFromCapture::Names");
    std::vector<NameCount> sorted_counts(name_count.begin(), name_count.end());
    std::ranges::sort(sorted_counts, std::ranges::greater(), &NameCount::count);
    size_t index = 0;
    name_to_index.reserve(sorted_counts.size());
    for (const auto& kc : sorted_counts) {
      name_to_index[kc.name_] = index++;
    }

    for (const auto& nc : sorted_counts) {
      vmo_names.PushBack(rapidjson::StringRef(nc.name_.data(), nc.name_.length()), a);
    }
  }
  {
    TRACE_DURATION("memory_metrics", "JsonPrinter::DocumentFromCapture::Vmos");
    rapidjson::Value vmo_header(rapidjson::kArrayType);
    vmo_header.PushBack("koid", a)
        .PushBack("name", a)
        .PushBack("parent_koid", a)
        .PushBack("committed_bytes", a)
        .PushBack("allocated_bytes", a);
    if (capture.kmem_compression()) {
      vmo_header.PushBack("populated_bytes", a);
    }
    vmos.PushBack(vmo_header, a);
    for (const auto& [k, v] : capture.koid_to_vmo()) {
      rapidjson::Value vmo_value(rapidjson::kArrayType);
      // TODO(b/377993710): Should also pass PSS RSS and USS for proper accounting.
      vmo_value.PushBack(v.koid, a)
          .PushBack(name_to_index[v.name], a)
          .PushBack(v.parent_koid, a)
          .PushBack(v.committed_scaled_bytes.integral, a)
          .PushBack(v.allocated_bytes, a);
      if (capture.kmem_compression()) {
        vmo_value.PushBack(v.populated_scaled_bytes.integral, a);
      }
      vmos.PushBack(vmo_value, a);
    }
  }
  j.AddMember("Processes", processes, a)
      .AddMember("VmoNames", vmo_names, a)
      .AddMember("Vmos", vmos, a);

  return j;
}

std::string EscapeString(std::string_view s) {
  std::string escaped;
  escaped.reserve(s.size());
  for (char c : s) {
    switch (c) {
      case '\n':
        escaped += "\\n";
        break;
      case '\r':
        escaped += "\\r";
        break;
      case '\t':
        escaped += "\\t";
        break;
      case '\\':
        escaped += "\\\\";
        break;
      case '"':
        escaped += "\\\"";
        break;
      default:
        // Escape ASCII control characters (< 32) and DEL (127).
        if (static_cast<unsigned char>(c) < 32 || static_cast<unsigned char>(c) == 127) {
          char buf[5];
          snprintf(buf, sizeof(buf), "\\x%02x", static_cast<unsigned char>(c));
          escaped += buf;
        } else {
          escaped += c;
        }
        break;
    }
  }
  return escaped;
}

}  // namespace

namespace memory {

const char* FormatSize(uint64_t bytes, char* buf) {
  const char units[] = "BKMGTPE";
  uint16_t r = 0;
  int ui = 0;
  while (bytes > 1023) {
    r = bytes % 1024;
    bytes /= 1024;
    ui++;
  }
  uint16_t round_up = ((r % 102) >= 51);
  r = (r / 102) + round_up;
  if (r == 10) {
    bytes++;
    r = 0;
  }
  if (r == 0) {
    snprintf(buf, kMaxFormattedStringSize, "%zu%c", bytes, units[ui]);
  } else {
    snprintf(buf, kMaxFormattedStringSize, "%zu.%1u%c", bytes, r, units[ui]);
  }
  return buf;
}

void JsonPrinter::PrintCapture(const Capture& capture) {
  TRACE_DURATION("memory_metrics", "JsonPrinter::PrintCaptureJson");
  rapidjson::Document doc = DocumentFromCapture(capture);
  {
    TRACE_DURATION("memory_metrics", "JsonPrinter::PrintCaptureJson::Write");
    SocketWriteStream sw(output_socket);
    rapidjson::Writer<SocketWriteStream> writer(sw);
    doc.Accept(writer);
  }
}

void JsonPrinter::PrintCaptureAndBucketConfig(const Capture& capture,
                                              const std::string& bucket_config) {
  TRACE_DURATION("memory_metrics", "JsonPrinter::PrintCaptureAndBucketConfig");

  rapidjson::Document d(rapidjson::kObjectType);
  auto& a = d.GetAllocator();

  rapidjson::Document capture_doc = DocumentFromCapture(capture);
  d.AddMember("Capture", capture_doc, a);

  rapidjson::Document bucket_val;
  bucket_val.Parse(bucket_config.c_str());
  d.AddMember("Buckets", bucket_val, a);

  {
    TRACE_DURATION("memory_metrics", "JsonPrinter::PrintCaptureAndBucketConfig::Write");
    SocketWriteStream sw(output_socket);
    rapidjson::Writer<SocketWriteStream> writer(sw);
    d.Accept(writer);
  }
}

void TextPrinter::OutputSizes(const Sizes& sizes) {
  if (sizes.total_bytes == sizes.private_bytes) {
    char private_buf[kMaxFormattedStringSize];
    os_ << FormatSize(sizes.private_bytes.integral, private_buf) << "\n";
    return;
  }
  char private_buf[kMaxFormattedStringSize], scaled_buf[kMaxFormattedStringSize],
      total_buf[kMaxFormattedStringSize];
  os_ << FormatSize(sizes.private_bytes.integral, private_buf) << " "
      << FormatSize(sizes.scaled_bytes.integral, scaled_buf) << " "
      << FormatSize(sizes.total_bytes.integral, total_buf) << "\n";
}

void TextPrinter::PrintSummary(const Summary& summary, CaptureLevel level, Sorted sorted) {
  TRACE_DURATION("memory_metrics", "TextPrinter::PrintSummary");
  char vmo_buf[kMaxFormattedStringSize], free_buf[kMaxFormattedStringSize];
  const auto& kstats = summary.kstats();
  os_ << "Time: " << summary.time() << " VMO: " << FormatSize(kstats.vmo_bytes, vmo_buf)
      << " Free: " << FormatSize(kstats.free_bytes, free_buf) << "\n";

  if (level == CaptureLevel::KMEM) {
    return;
  }

  const auto& summaries = summary.process_summaries();
  std::vector<uint32_t> summary_order;
  summary_order.reserve(summaries.size());
  for (uint32_t i = 0; i < summaries.size(); i++) {
    summary_order.push_back(i);
  }

  if (sorted == SORTED) {
    std::ranges::sort(summary_order, std::ranges::greater(),
                      [&summaries](uint32_t i) { return summaries[i].sizes().private_bytes; });
  }
  for (auto i : summary_order) {
    const auto& s = summaries[i];
    os_ << EscapeString(s.name()) << "<" << s.koid() << "> ";
    OutputSizes(s.sizes());
    if (level == CaptureLevel::PROCESS) {
      continue;
    }

    const auto& name_to_sizes = s.name_to_sizes();
    std::vector<std::string> names;
    names.reserve(name_to_sizes.size());
    for (const auto& [name, sizes] : name_to_sizes) {
      names.push_back(name);
    }
    if (sorted == SORTED) {
      std::ranges::sort(names, std::ranges::greater(), [&name_to_sizes](const std::string& name) {
        auto& sizes = name_to_sizes.at(name);
        return std::tie(sizes.private_bytes, sizes.scaled_bytes);
      });
    }
    for (const auto& name : names) {
      const auto& n_sizes = name_to_sizes.at(name);
      if (n_sizes.total_bytes == 0) {
        continue;
      }
      os_ << " " << EscapeString(name) << " ";
      OutputSizes(n_sizes);
    }
  }
  os_ << std::flush;
}

void TextPrinter::PrintDigest(const Digest& digest) {
  TRACE_DURATION("memory_metrics", "TextPrinter::PrintDigest");
  std::vector<Bucket> buckets = digest.buckets();
  std::ranges::sort(buckets, std::ranges::greater(), &Bucket::size);

  for (auto const& bucket : buckets) {
    char size_buf[kMaxFormattedStringSize];
    FormatSize(bucket.size(), size_buf);
    os_ << bucket.name() << ": " << size_buf << "\n";
  }
}

}  // namespace memory
