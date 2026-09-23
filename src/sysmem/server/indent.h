// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_SYSMEM_SERVER_INDENT_H_
#define SRC_SYSMEM_SERVER_INDENT_H_

#include <inttypes.h>

class IndentScope;
class IndentTracker {
 public:
  IndentTracker(uint32_t indent_level) : indent_level_(indent_level) {}
  IndentScope Current();
  IndentScope Nested(uint32_t nesting_levels = 1);

 private:
  friend class IndentScope;
  static constexpr uint32_t kIndentSpaces = 2;
  void AddIndentLevel(uint32_t level_count) { indent_level_ += level_count; }
  void SubIndentLevel(uint32_t level_count) { indent_level_ -= level_count; }
  int num_spaces() { return kIndentSpaces * indent_level_; }
  int indent_level_ = 0;
};

class IndentScope {
 public:
  ~IndentScope() { parent_.SubIndentLevel(level_count_); }
  uint32_t num_spaces() { return parent_.num_spaces(); }

 private:
  friend class IndentTracker;
  IndentScope(IndentTracker& parent, uint32_t level_count)
      : parent_(parent), level_count_(level_count) {
    parent_.AddIndentLevel(level_count_);
  }
  IndentTracker& parent_;
  uint32_t level_count_ = 0;
};

#endif  // SRC_SYSMEM_SERVER_INDENT_H_
