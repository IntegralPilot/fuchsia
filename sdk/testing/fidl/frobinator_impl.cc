// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "frobinator_impl.h"

#include <lib/fit/function.h>

#include <cstdint>
#include <utility>
#include <vector>

#include "lib/fpromise/result.h"

namespace fidl {
namespace test {

FrobinatorImpl::FrobinatorImpl(fit::closure on_destroy) : on_destroy_(std::move(on_destroy)) {}

FrobinatorImpl::~FrobinatorImpl() { on_destroy_(); }

void FrobinatorImpl::Frob(std::string value) { frobs.push_back(std::move(value)); }

void FrobinatorImpl::Grob(std::string value, GrobCallback callback) {
  grobs.push_back(std::move(value));
  callback("response");
}

void FrobinatorImpl::Fail(bool fail, FailCallback callback) {
  if (fail) {
    callback(fpromise::error(42U));
  } else {
    callback(fpromise::ok());
  }
}

void FrobinatorImpl::FailHard(bool fail, FailHardCallback callback) {
  if (fail) {
    callback(fpromise::error(42U));
  } else {
    callback(fpromise::ok(std::string("hello, world")));
  }
}

void FrobinatorImpl::FailHardest(bool fail, FailHardestCallback callback) {
  if (fail) {
    callback(fpromise::error(42U));
  } else {
    callback(fpromise::ok(std::make_tuple(std::string("hello"), std::string("world"))));
  }
}

void FrobinatorImpl::SendEventHandle(zx::event event) {}
void FrobinatorImpl::SendProtocol(fidl::InterfaceHandle<fidl::test::frobinator::EmptyProtocol> ep) {
}

void FrobinatorImpl::SendBasicUnion(fidl::test::frobinator::BasicUnion u) {
  send_basic_union_received_value_ = u.v();
}

void FrobinatorImpl::SendBasicTable(fidl::test::frobinator::BasicTable t) {
  send_basic_table_received_value_ = t.v();
}

void FrobinatorImpl::SendComplexTables(::std::vector<::fidl::test::frobinator::ComplexTable> ct,
                                       SendComplexTablesCallback callback) {
  send_complex_tables_received_entry_count_ = static_cast<uint32_t>(ct.size());
  for (auto &entry : ct) {
    if (entry.has_x()) {
      if (entry.x().is_a()) {
        send_complex_tables_received_x_a_count_++;
      } else {
        send_complex_tables_received_x_b_count_++;
      }
    }

    if (entry.has_y()) {
      if (entry.y()) {
        send_complex_tables_received_y_true_count_++;
      } else {
        send_complex_tables_received_y_false_count_++;
      }
    }
  }

  callback({{1, 2, 3}, {4, 5, 6}, {7, 8, 9}});
}

}  // namespace test
}  // namespace fidl
