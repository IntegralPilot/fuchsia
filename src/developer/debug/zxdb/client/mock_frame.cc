// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/mock_frame.h"

#include "gtest/gtest.h"
#include "src/developer/debug/shared/message_loop.h"
#include "src/developer/debug/zxdb/client/arch_info.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/source_file_provider_impl.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/expr/eval_context_impl.h"
#include "src/developer/debug/zxdb/expr/expr_parser.h"
#include "src/developer/debug/zxdb/expr/parsed_identifier.h"
#include "src/developer/debug/zxdb/symbols/function.h"
#include "src/developer/debug/zxdb/symbols/mock_symbol_data_provider.h"
#include "src/developer/debug/zxdb/symbols/namespace.h"
#include "src/developer/debug/zxdb/symbols/source_file_provider.h"

namespace zxdb {

MockFrame::MockFrame(Session* session, Thread* thread, const Location& location, uint64_t sp,
                     uint64_t cfa, std::vector<debug::RegisterValue> regs, uint64_t frame_base,
                     const Frame* physical_frame, bool is_ambiguous_inline)
    : Frame(session),
      thread_(thread),
      sp_(sp),
      cfa_(cfa),
      general_registers_(std::move(regs)),
      frame_base_(frame_base),
      physical_frame_(physical_frame),
      location_(location),
      is_ambiguous_inline_(is_ambiguous_inline) {}

MockFrame::MockFrame(Session* session, Thread* thread, TargetPointer ip, TargetPointer sp,
                     const std::string& func_name, FileLine file_line)
    : MockFrame(session, thread, Location{}, sp) {
  ParsedIdentifier ident;
  auto err = ExprParser::ParseIdentifier(func_name, &ident);
  if (err.has_error()) {
    ADD_FAILURE() << "Failed to parse " << func_name << " as identifier: " << err.ToString();
  }

  std::vector<std::string> string_components;
  for (const auto& component : ident.components()) {
    string_components.push_back(component.GetName(false));
  }
  std::string function_name = string_components.back();
  string_components.pop_back();
  MakeLocation(ip, function_name, std::move(file_line), string_components);
}

MockFrame::MockFrame(Session* session, Thread* thread, TargetPointer ip, TargetPointer sp,
                     FileLine file_line, std::vector<std::string> absolute_function_name)
    : MockFrame(session, thread, Location{}, sp) {
  auto function_name = absolute_function_name.back();
  absolute_function_name.pop_back();
  MakeLocation(ip, function_name, std::move(file_line), absolute_function_name);
}

MockFrame::~MockFrame() = default;

void MockFrame::SetAddress(uint64_t address) {
  location_ = Location(address, location_.file_line(), location_.column(),
                       location_.symbol_context(), location_.symbol());
}

void MockFrame::SetTrust(debug_ipc::StackFrame::Trust trust) { trust_ = trust; }

void MockFrame::SetFileLine(const FileLine& file_line) {
  location_ = Location(location_.address(), file_line, location_.column(),
                       location_.symbol_context(), location_.symbol());
}

MockSymbolDataProvider* MockFrame::GetMockSymbolDataProvider() {
  GetSymbolDataProvider();  // Force creation.
  return symbol_data_provider_.get();
}

std::unique_ptr<SourceFileProvider> MockFrame::GetSourceFileProvider() const {
  if (source_file_provider_)
    return std::make_unique<MockSourceFileProvider>(*source_file_provider_);
  // Don't return nullptr here as existing or future tests that use MockFrame may not
  // always provide mock source files, which could lead to nullptr dereferences.
  // Handling the fallback here prevents client code from becoming bloated with
  // redundant fallback logic.
  return std::make_unique<SourceFileProviderImpl>(
      GetThread()->GetProcess()->GetTarget()->settings());
}

Thread* MockFrame::GetThread() const { return thread_; }

bool MockFrame::IsInline() const { return !!physical_frame_; }

const Frame* MockFrame::GetPhysicalFrame() const {
  if (physical_frame_)
    return physical_frame_;
  return this;
}

const Location& MockFrame::GetLocation() const { return location_; }
uint64_t MockFrame::GetAddress() const { return location_.address(); }
debug_ipc::StackFrame::Trust MockFrame::GetTrust() const { return trust_; }

const std::vector<debug::RegisterValue>* MockFrame::GetRegisterCategorySync(
    debug::RegisterCategory category) const {
  if (category == debug::RegisterCategory::kGeneral)
    return &general_registers_;
  return nullptr;
}

void MockFrame::GetRegisterCategoryAsync(
    debug::RegisterCategory category, bool always_request,
    fit::function<void(const Err&, const std::vector<debug::RegisterValue>&)> cb) {
  Err err;
  std::vector<debug::RegisterValue> regs;
  if (category == debug::RegisterCategory::kGeneral)
    regs = general_registers_;
  else
    err = Err("Register category unavailable from mock.");

  debug::MessageLoop::Current()->PostTask(
      FROM_HERE, [err, regs, cb = std::move(cb)]() mutable { cb(err, regs); });
}

void MockFrame::WriteRegister(debug::RegisterID id, std::vector<uint8_t> data,
                              fit::callback<void(const Err&)> cb) {
  debug::MessageLoop::Current()->PostTask(FROM_HERE, [cb = std::move(cb)]() mutable {
    cb(Err("Writing registers not (yet) supported by the mock."));
  });
}

std::optional<uint64_t> MockFrame::GetBasePointer() const { return frame_base_; }

void MockFrame::GetBasePointerAsync(fit::callback<void(uint64_t)> cb) {
  debug::MessageLoop::Current()->PostTask(
      FROM_HERE, [bp = frame_base_, cb = std::move(cb)]() mutable { cb(bp); });
}

uint64_t MockFrame::GetStackPointer() const { return sp_; }

uint64_t MockFrame::GetCanonicalFrameAddress() const { return cfa_; }

fxl::RefPtr<SymbolDataProvider> MockFrame::GetSymbolDataProvider() const {
  if (!symbol_data_provider_)
    symbol_data_provider_ = fxl::MakeRefCounted<MockSymbolDataProvider>();
  return symbol_data_provider_;
}

fxl::RefPtr<EvalContext> MockFrame::GetEvalContext() const {
  if (!eval_context_) {
    eval_context_ = fxl::MakeRefCounted<EvalContextImpl>(session()->arch_info().abi(),
                                                         fxl::WeakPtr<const ProcessSymbols>(),
                                                         GetSymbolDataProvider(), location_);
  }
  return eval_context_;
}

bool MockFrame::IsAmbiguousInlineLocation() const { return is_ambiguous_inline_; }

void MockFrame::MakeLocation(TargetPointer ip, const std::string& function_name, FileLine file_line,
                             const std::vector<std::string>& namespace_strings) {
  // Pointer to the last created namespace.
  fxl::RefPtr<Namespace> last = nullptr;
  std::vector<fxl::RefPtr<Namespace>> namespaces;
  namespaces.resize(namespace_strings.size());

  // This is very important. If we don't reserve the right amount of parents
  // here, then emplace_back will drop the Symbol references leading to the
  // function having an incomplete identifier.
  parent_setters_.reserve(namespace_strings.size());

  for (const auto& ns : namespace_strings) {
    auto nsptr = fxl::MakeRefCounted<Namespace>(ns);

    if (last.get() == nullptr) {
      last = nsptr;
    } else {
      parent_setters_.emplace_back(nsptr, last);
      last = nsptr;
    }

    namespaces.push_back(nsptr.Clone());
  }

  auto function = fxl::MakeRefCounted<Function>(DwarfTag::kSubprogram);
  function->set_assigned_name(function_name);
  if (!namespaces.empty()) {
    parent_setters_.emplace_back(function, namespaces.back());
  }

  location_ =
      Location(ip, std::move(file_line), 0, SymbolContext::ForRelativeAddresses(), function);
}

}  // namespace zxdb
