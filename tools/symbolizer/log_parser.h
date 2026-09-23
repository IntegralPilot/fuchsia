// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef TOOLS_SYMBOLIZER_LOG_PARSER_H_
#define TOOLS_SYMBOLIZER_LOG_PARSER_H_

#include <deque>
#include <iostream>
#include <optional>
#include <string>
#include <string_view>

#include "src/lib/fxl/memory/ref_counted.h"
#include "src/lib/fxl/memory/ref_ptr.h"
#include "tools/symbolizer/symbolizer.h"

namespace symbolizer {

// This is the "main class" of the symbolizer. A LogParser takes an input stream, reads lines and
// writes to an output stream. If a line contains symbolizer markups, i.e., {{{...}}}, the
// LogParser will parse its content and pass to the real symbolizer. The symbolizer markup format
// is documented in //docs/reference/kernel/symbolizer_markup.md.
//
// For simplicity, this implementation has the following assumptions/limitations.
// 1. Interleaved stack traces are not supported. There can be at most 1 stack trace at a time.
// 2. Log will presume its order. When a symbolizer markup is being processed, e.g., downloading the
//    symbol file, the output will stall, even if the next line contains no markup.
// 3. Multiple markups per line are supported.
//
// The markup parsing operates via a lazy, zero-allocation pull parser architecture:
// - Lexing (LogTokenIterator): Scans lines into zero-allocation string_view tokens (kText/kMarkup)
//   on demand via Next(). If an opening `{{{` delimiter is followed by a second `{{{` prior to
//   `}}}`, the earlier `{{{` fragment is classified as kText. This cleanly recovers from stray
//   `{{{` delimiters (e.g., `random {{{ text {{{bt:0:0x1000}}}`).
// - Evaluation: Consumes tokens from LogTokenIterator in a single streaming pass without allocating
//   an intermediate token vector. Surrounding syslog headers (e.g., "context1: ") are discarded
//   for lines containing only valid non-printing metadata tags (module, reset) and no active output
//   tags.

enum class TokenType {
  kText,
  kMarkup,
};

struct LogToken {
  TokenType type;
  std::string_view text;
  std::string_view raw;
};

// Lazy, zero-allocation pull parser over a line of symbolizer markup.
class LogTokenIterator {
 public:
  explicit LogTokenIterator(std::string_view line) : line_(line) {}

  std::optional<LogToken> Next();

  bool PeekNextMarkup() const;

  std::string_view RemainingText() const { return line_.substr(pos_); }

 private:
  std::string_view line_;
  size_t pos_ = 0;
};

//
// In addition to the symbolizer markup format described above, this class also supports symbolizing
// Dart stack traces in AOT mode with --dwarf_stack_traces option, which looks like
// *** *** *** *** *** *** *** *** *** *** *** *** *** *** *** ***
// pid: 12, tid: 30221, name some.ui
// build_id: '0123456789abcdef'
// isolate_dso_base: f2e4c8000, vm_dso_base: f2e4c8000
// isolate_instructions: f2f9f8e60, vm_instructions: f2f9f4000
// #00 abs 0000000f2fbb51c7 virt 00000000016ed1c7 _kDartIsolateSnapshotInstructions+0x1bc367
// #01 ...
class LogParser {
 public:
  // Initializes the LogParser. All of the parameters must outlive this LogParser.
  LogParser(std::istream& input, std::ostream& output, Symbolizer* symbolizer)
      : input_(input), output_(output), symbolizer_(symbolizer) {}

  // Reads the next line from the input, sends it to the symbolizer or writes to the output.
  // Returns false if there's no more line in the input.
  bool ProcessNextLine();

 private:
  struct OutputEntry : public fxl::RefCountedThreadSafe<OutputEntry> {
    // Lifecycle state of an async symbolizer output callback:
    // - kPending: Output callback object has been created, but not yet invoked or destroyed.
    // - kInvoked: Output callback was invoked to emit text.
    // - kDropped: Output callback was destroyed (dropped) without being invoked.
    enum class State {
      kPending,
      kInvoked,
      kDropped,
    };

    std::string text;
    bool ready = false;
    State state = State::kPending;
  };

  // Processes one markup. Returns whether the markup could be processed successfully.
  bool ProcessMarkup(std::string_view markup, Symbolizer::StringOutputFn output);

  // Processes one line of Dart stack traces. Return false if it's not valid.
  bool ProcessDart(std::string_view line, Symbolizer::StringOutputFn output);

  // Create an async output function for the symbolizer and its corresponding OutputEntry.
  std::pair<Symbolizer::StringOutputFn, fxl::RefPtr<OutputEntry>> CreateOutputFn(
      std::string_view prefix, std::string_view suffix);

  // Flushes output_buffers_ from the front while entries are ready.
  void FlushOutputBuffers();

  // Output a raw message. If there's no async output pending, output directly. Otherwise, append
  // the output to the output buffer.
  void OutputRaw(std::string_view message);

  std::istream& input_;
  std::ostream& output_;
  Symbolizer* symbolizer_;

  // Whether we're symbolizing Dart stack traces. Triggered by the "***" line.
  bool symbolizing_dart_ = false;
  std::string dart_process_name_;

  // To facilitate async output of the symbolizer while keeping the same order, we cache input lines
  // following the symbolization markups if they are pending. It's a deque because there might be
  // multiple pending symbolization markups. See the comments in |CreateOutputFn|.
  std::deque<fxl::RefPtr<OutputEntry>> output_buffers_;
};

}  // namespace symbolizer

#endif  // TOOLS_SYMBOLIZER_LOG_PARSER_H_
