// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/symbolizer/log_parser.h"

#include <lib/fit/defer.h>
#include <lib/syslog/cpp/macros.h>

#include <charconv>
#include <cstdint>
#include <deque>
#include <memory>
#include <string>
#include <string_view>
#include <utility>

#include "src/developer/debug/ipc/records.h"
#include "src/lib/fxl/strings/split_string.h"
#include "src/lib/fxl/strings/trim.h"
#include "tools/symbolizer/symbolizer.h"

namespace symbolizer {

namespace {

// This need to match
// https://github.com/dart-lang/sdk/blob/f424f3a4cca306513e77c7747682f1c1c99e3307/runtime/vm/object.cc#L25501
constexpr std::string_view kDartStackTraceMagic =
    "*** *** *** *** *** *** *** *** *** *** *** *** *** *** *** ***";

constexpr std::string_view kElfTypeDelimiter = ":elf:";

// Converts the string in dec or hex into an integer. Returns whether the conversion is complete.
template <typename int_t>
bool ParseInt(std::string_view string, int_t &i, int base = 10) {
  if (string.empty())
    return false;

  const char *begin = string.data();
  const char *end = begin + string.size();
  if (string.size() > 2 && string[0] == '0' && string[1] == 'x') {
    base = 16;
    begin += 2;
  }
  return std::from_chars(begin, end, i, base).ptr == end;
}

}  // namespace

std::optional<LogToken> LogTokenIterator::Next() {
  if (pos_ >= line_.size()) {
    return std::nullopt;
  }

  const size_t start = line_.find("{{{", pos_);
  if (start == std::string_view::npos) {
    const std::string_view text_segment = line_.substr(pos_);
    pos_ = line_.size();
    return LogToken{TokenType::kText, text_segment, text_segment};
  }

  if (start > pos_) {
    const std::string_view text_segment = line_.substr(pos_, start - pos_);
    pos_ = start;
    return LogToken{TokenType::kText, text_segment, text_segment};
  }

  const size_t end = line_.find("}}}", start + 3);
  const size_t next_start = line_.find("{{{", start + 3);

  if (end == std::string_view::npos || (next_start != std::string_view::npos && next_start < end)) {
    const size_t text_end = (next_start != std::string_view::npos) ? next_start : line_.size();
    const std::string_view text_segment = line_.substr(start, text_end - start);
    pos_ = text_end;
    return LogToken{TokenType::kText, text_segment, text_segment};
  }

  const std::string_view markup_text = line_.substr(start + 3, end - (start + 3));
  const std::string_view raw_text = line_.substr(start, end + 3 - start);
  pos_ = end + 3;
  return LogToken{TokenType::kMarkup, markup_text, raw_text};
}

bool LogTokenIterator::PeekNextMarkup() const {
  size_t search_pos = pos_;
  while (true) {
    const size_t start = line_.find("{{{", search_pos);
    if (start == std::string_view::npos) {
      return false;
    }
    const size_t end = line_.find("}}}", start + 3);
    const size_t next_start = line_.find("{{{", start + 3);
    if (end == std::string_view::npos) {
      return false;
    }
    if (next_start != std::string_view::npos && next_start < end) {
      search_pos = next_start;
      continue;
    }
    return true;
  }
}

bool LogParser::ProcessNextLine() {
  std::string line;

  std::getline(input_, line);
  if (input_.eof() && line.empty()) {
    return false;
  }

  LogTokenIterator it(line);
  if (it.PeekNextMarkup()) {
    enum class LineDisposition {
      kUnset,
      kSuppressPrefix,
      kEmitSurroundingText,
    };

    LineDisposition disposition = LineDisposition::kUnset;
    bool last_tag_dropped = false;
    std::string pending_prefix;

    while (auto token = it.Next()) {
      if (token->type == TokenType::kText) {
        pending_prefix += std::string(token->text);
      } else {
        const bool is_last_markup = !it.PeekNextMarkup();

        std::string current_prefix = std::move(pending_prefix);
        pending_prefix = "";

        std::string suffix_segment;
        if (is_last_markup) {
          suffix_segment = std::string(it.RemainingText());
        }

        auto [output, entry] = CreateOutputFn(current_prefix, suffix_segment);
        const bool ok = ProcessMarkup(token->text, std::move(output));
        if (ok) {
          if (disposition == LineDisposition::kUnset) {
            disposition = LineDisposition::kSuppressPrefix;
          }
        } else {
          disposition = LineDisposition::kEmitSurroundingText;
        }

        if (entry->state != OutputEntry::State::kDropped) {
          disposition = LineDisposition::kEmitSurroundingText;
          last_tag_dropped = false;
        } else {
          last_tag_dropped = true;
          // If the tag emitted no output, defer outputting current_prefix.
          // For invalid markup (!ok), retain the raw tag text in pending_prefix
          // so it can be printed verbatim. For valid silent markup (e.g. module,
          // reset), omit the tag text but keep current_prefix in case subsequent
          // tokens on the line require emitting surrounding text.
          if (!ok) {
            pending_prefix = current_prefix + std::string(token->raw);
          } else {
            pending_prefix = current_prefix;
          }
        }

        if (is_last_markup) {
          if (last_tag_dropped) {
            pending_prefix += std::string(it.RemainingText());
          }
          break;
        }
      }
    }

    if (last_tag_dropped) {
      // Discard standalone valid silent tags (e.g. "[klog] INFO: {{{module:...}}}")
      // along with their surrounding text. If the line contained any invalid or
      // output-producing tags, emit the accumulated surrounding text.
      if (!pending_prefix.empty() && disposition == LineDisposition::kEmitSurroundingText) {
        OutputRaw(pending_prefix);
        return true;
      }
    }

    if (disposition != LineDisposition::kUnset) {
      return true;
    }
  }

  // Handle Dart symbolization.
  if (line == kDartStackTraceMagic) {
    symbolizing_dart_ = true;
  } else if (symbolizing_dart_) {
    auto [output, entry] = CreateOutputFn("", "");
    if (ProcessDart(line, std::move(output))) {
      return true;
    }
    symbolizing_dart_ = false;
  }

  OutputRaw(line);
  return true;
}

bool LogParser::ProcessMarkup(std::string_view markup, Symbolizer::StringOutputFn output) {
  auto splitted = fxl::SplitString(markup, ":", fxl::kKeepWhitespace, fxl::kSplitWantAll);
  if (splitted.empty()) {
    return false;
  }

  auto tag = splitted[0];

  if (tag == "reset") {
    auto type = Symbolizer::ResetType::kUnknown;
    if (splitted.size() >= 2) {
      if (splitted[1] == "begin") {
        type = Symbolizer::ResetType::kBegin;
      } else if (splitted[1] == "end") {
        type = Symbolizer::ResetType::kEnd;
      }
    }
    symbolizer_->Reset(false, type);
    return true;
  }

  if (tag == "module") {
    // module:0x{id}:{name}:elf:{build_id}(:extra)*
    size_t first_colon = markup.find(':');
    if (first_colon == std::string_view::npos)
      return false;
    size_t second_colon = markup.find(':', first_colon + 1);
    if (second_colon == std::string_view::npos)
      return false;

    std::string_view id_str = markup.substr(first_colon + 1, second_colon - first_colon - 1);
    uint64_t id;
    if (!ParseInt(id_str, id))
      return false;

    std::string_view rest = markup.substr(second_colon + 1);
    size_t elf_pos = rest.rfind(kElfTypeDelimiter);
    if (elf_pos == std::string_view::npos)
      return false;

    std::string_view name = rest.substr(0, elf_pos);
    std::string_view after_elf = rest.substr(elf_pos + kElfTypeDelimiter.size());

    size_t next_colon = after_elf.find(':');
    std::string_view build_id =
        (next_colon == std::string_view::npos) ? after_elf : after_elf.substr(0, next_colon);

    symbolizer_->Module(id, name, build_id);
    return true;
  }

  if (tag == "mmap") {
    // mmap:0x{address}:0x{size}:load:0x{module_id}:r?w?x?:0x{module_offset}
    if (splitted.size() < 7)
      return false;

    uint64_t address;
    uint64_t size;
    uint64_t module_id;
    uint64_t module_offset;

    if (!ParseInt(splitted[1], address) || !ParseInt(splitted[2], size) ||
        !ParseInt(splitted[4], module_id) || !ParseInt(splitted[6], module_offset) ||
        splitted[3] != "load")
      return false;

    symbolizer_->MMap(address, size, module_id, splitted[5], module_offset, std::move(output));
    return true;
  }

  if (tag == "bt") {
    // bt:{frame_id}:{address}(:ra|:pc)?(:msg)?
    if (splitted.size() < 3)
      return false;

    int frame_id;
    uint64_t address;
    debug_ipc::StackFrame::AddressType type = debug_ipc::StackFrame::AddressType::kUnknown;
    std::string_view message;

    if (!ParseInt(splitted[1], frame_id) || !ParseInt(splitted[2], address))
      return false;

    // Optional suffix(es).
    if (splitted.size() >= 4) {
      if (splitted[3] == "ra") {
        type = debug_ipc::StackFrame::AddressType::kReturn;
      } else if (splitted[3] == "pc") {
        type = debug_ipc::StackFrame::AddressType::kExact;
      } else {
        message = splitted[3];
      }
      if (splitted.size() >= 5) {
        message = splitted[4];
      }
    }
    symbolizer_->Backtrace(frame_id, address, type, message, std::move(output));
    return true;
  }

  if (tag == "dumpfile") {
    // dumpfile:{type}:{name}
    if (splitted.size() < 3)
      return false;

    symbolizer_->DumpFile(splitted[1], splitted[2]);
    return true;
  }

  return false;
}

// If returning true, we're responsible to output the line.
bool LogParser::ProcessDart(std::string_view line, Symbolizer::StringOutputFn output) {
  constexpr uint64_t kModuleId = 0;
  constexpr uint64_t kModuleSize = 0x800000000;  // 32 GB should be big enough.

  auto splitted = fxl::SplitString(line, " ", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);

  if (splitted.size() == 6 && splitted[0] == "pid:") {
    // pid: 12, tid: 30221, name some.ui
    dart_process_name_ = splitted[5];
    symbolizer_->Reset(true, Symbolizer::ResetType::kUnknown);
  } else if (splitted.size() == 2 && splitted[0] == "build_id:") {
    // build_id: '0123456789abcdef'
    symbolizer_->Module(kModuleId, dart_process_name_, fxl::TrimString(splitted[1], "'"));
  } else if (splitted.size() == 4 && splitted[0] == "isolate_dso_base:") {
    // isolate_dso_base: f2e4c8000, vm_dso_base: f2e4c8000
    uint64_t address;
    if (!ParseInt(splitted[3], address, 16)) {
      return false;
    }
    symbolizer_->MMap(address, kModuleSize, kModuleId, "", 0, std::move(output));
  } else if (!splitted.empty() &&
             (splitted[0] == "os:" || splitted[0] == "isolate_instructions:")) {
    // os: fuchsia arch: arm64 comp: no sim: no
    // isolate_instructions: f2f9f8e60, vm_instructions: f2f9f4000
  } else if (splitted.size() >= 6 && splitted[0][0] == '#' && splitted[1] == "abs") {
    // #00 abs 0000000f2fbb51c7 virt 00000000016ed1c7 _kDartIsolateSnapshotInstructions+0x1bc367
    uint64_t frame_id;
    uint64_t address;
    if (!ParseInt(splitted[0].substr(1), frame_id)) {
      return false;
    }
    if (!ParseInt(splitted[2], address, 16)) {
      return false;
    }
    symbolizer_->Backtrace(frame_id, address, debug_ipc::StackFrame::AddressType::kUnknown, "",
                           std::move(output));
    return true;
  } else {
    return false;
  }

  // Don't forget to output the context as is.
  OutputRaw(line);
  return true;
}

std::pair<Symbolizer::StringOutputFn, fxl::RefPtr<LogParser::OutputEntry>>
LogParser::CreateOutputFn(std::string_view prefix, std::string_view suffix) {
  auto entry = fxl::MakeRefCounted<OutputEntry>();
  output_buffers_.push_back(entry);

  auto on_drop = fit::defer([this, entry]() {
    if (entry->state == OutputEntry::State::kPending) {
      entry->state = OutputEntry::State::kDropped;
    }
    entry->ready = true;
    FlushOutputBuffers();
  });

  auto output = [prefix = std::string(prefix), suffix = std::string(suffix), entry,
                 on_drop = std::move(on_drop)](std::string_view content) {
    entry->state = OutputEntry::State::kInvoked;
    entry->text += prefix;
    entry->text += content;
    entry->text += suffix;
    entry->text += '\n';
  };

  return {std::move(output), entry};
}

void LogParser::FlushOutputBuffers() {
  while (!output_buffers_.empty() && output_buffers_.front()->ready) {
    output_ << output_buffers_.front()->text;
    output_buffers_.pop_front();
  }
}

void LogParser::OutputRaw(std::string_view message) {
  if (!output_buffers_.empty()) {
    auto entry = fxl::MakeRefCounted<OutputEntry>();
    entry->text = std::string(message) + '\n';
    entry->ready = true;
    output_buffers_.push_back(std::move(entry));
    FlushOutputBuffers();
  } else {
    output_ << message << '\n';
  }
}

}  // namespace symbolizer
