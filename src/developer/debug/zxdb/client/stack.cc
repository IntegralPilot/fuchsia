// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/stack.h"

#include <lib/syslog/cpp/macros.h>

#include <map>

#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/shared/message_loop.h"
#include "src/developer/debug/zxdb/client/arch_info.h"
#include "src/developer/debug/zxdb/client/frame.h"
#include "src/developer/debug/zxdb/client/frame_fingerprint.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/source_file_provider_impl.h"
#include "src/developer/debug/zxdb/client/target.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/common/err.h"
#include "src/developer/debug/zxdb/expr/abi.h"
#include "src/developer/debug/zxdb/expr/eval_context_impl.h"
#include "src/developer/debug/zxdb/symbols/function.h"
#include "src/developer/debug/zxdb/symbols/process_symbols.h"
#include "src/lib/fxl/macros.h"

namespace zxdb {

namespace {

// Implementation of Frame for inlined frames. Inlined frames have a different location in the
// source code, but refer to the underlying physical frame for most data.
class InlineFrame final : public Frame {
 public:
  // The physical_frame must outlive this class. Normally both are owned by the Stack and have the
  // same lifetime.
  InlineFrame(Frame* physical_frame, Location loc)
      : Frame(physical_frame->session()), physical_frame_(physical_frame), location_(loc) {}
  ~InlineFrame() override = default;

  std::unique_ptr<SourceFileProvider> GetSourceFileProvider() const override {
    return std::make_unique<SourceFileProviderImpl>(
        GetThread()->GetProcess()->GetTarget()->settings());
  }
  // Frame implementation.
  Thread* GetThread() const override { return physical_frame_->GetThread(); }
  bool IsInline() const override { return true; }
  const Frame* GetPhysicalFrame() const override { return physical_frame_; }
  const Location& GetLocation() const override { return location_; }
  uint64_t GetAddress() const override { return location_.address(); }
  debug_ipc::StackFrame::Trust GetTrust() const override { return physical_frame_->GetTrust(); }
  const std::vector<debug::RegisterValue>* GetRegisterCategorySync(
      debug::RegisterCategory category) const override {
    return physical_frame_->GetRegisterCategorySync(category);
  }
  void GetRegisterCategoryAsync(
      debug::RegisterCategory category, bool always_request,
      fit::function<void(const Err&, const std::vector<debug::RegisterValue>&)> cb) override {
    return physical_frame_->GetRegisterCategoryAsync(category, always_request, std::move(cb));
  }
  void WriteRegister(debug::RegisterID id, std::vector<uint8_t> data,
                     fit::callback<void(const Err&)> cb) override {
    return physical_frame_->WriteRegister(id, std::move(data), std::move(cb));
  }
  std::optional<uint64_t> GetBasePointer() const override {
    return physical_frame_->GetBasePointer();
  }
  void GetBasePointerAsync(fit::callback<void(uint64_t bp)> cb) override {
    return physical_frame_->GetBasePointerAsync(std::move(cb));
  }
  uint64_t GetCanonicalFrameAddress() const override {
    return physical_frame_->GetCanonicalFrameAddress();
  }
  uint64_t GetStackPointer() const override { return physical_frame_->GetStackPointer(); }
  fxl::RefPtr<SymbolDataProvider> GetSymbolDataProvider() const override {
    return physical_frame_->GetSymbolDataProvider();
  }
  fxl::RefPtr<EvalContext> GetEvalContext() const override {
    if (!symbol_eval_context_) {
      // Tolerate a null thread here because it makes testing much simpler. The EvalContext supports
      // a null ProcessSymbols for this case.
      fxl::WeakPtr<const ProcessSymbols> process_syms;
      if (Thread* thread = GetThread())
        process_syms = thread->GetProcess()->GetSymbols()->GetWeakPtr();
      symbol_eval_context_ = fxl::MakeRefCounted<EvalContextImpl>(
          session()->arch_info().abi(), process_syms, GetSymbolDataProvider(), location_);
    }
    return symbol_eval_context_;
  }
  bool IsAmbiguousInlineLocation() const override {
    const Location& loc = GetLocation();

    // Extract the inline function.
    if (!loc.symbol())
      return false;
    const Function* function = loc.symbol().Get()->As<Function>();
    if (!function)
      return false;
    if (!function->is_inline())
      return false;

    // There could be multiple code ranges for the inlined function, consider any of them as being a
    // candidate.
    for (const auto& cur : function->GetAbsoluteCodeRanges(loc.symbol_context())) {
      if (loc.address() == cur.begin())
        return true;
    }
    return false;
  }

 private:
  Frame* physical_frame_;  // Non-owning.
  Location location_;

  mutable fxl::RefPtr<EvalContextImpl> symbol_eval_context_;  // Lazy.

  FXL_DISALLOW_COPY_AND_ASSIGN(InlineFrame);
};

// Returns a fixed-up location referring to an indexed element in an inlined function call chain.
// This also handles the case where there are no inline calls and the function is the only one (this
// returns the same location).
//
// The main_location is the location returned by symbol lookup for the current address.
Location LocationForInlineFrameChain(const std::vector<fxl::RefPtr<Function>>& inline_chain,
                                     size_t chain_index, const Location& main_location) {
  // The file/line is the call location of the next (into the future) inlined function. Fall back on
  // the file/line from the main lookup.
  const FileLine* new_line = &main_location.file_line();
  int new_column = main_location.column();
  if (chain_index > 0) {
    const Function* next_call = inline_chain[chain_index - 1].get();
    if (next_call->call_line().is_valid()) {
      new_line = &next_call->call_line();
      new_column = 0;  // DWARF doesn't contain inline call column.
    }
  }

  return Location(main_location.address(), *new_line, new_column, main_location.symbol_context(),
                  inline_chain[chain_index]);
}

}  // namespace

Stack::Stack(Delegate* delegate) : delegate_(delegate), weak_factory_(this) {}

Stack::~Stack() = default;

fxl::WeakPtr<Stack> Stack::GetWeakPtr() { return weak_factory_.GetWeakPtr(); }

std::optional<size_t> Stack::IndexForFrame(const Frame* frame) const {
  for (size_t i = hide_ambiguous_inline_frame_count_; i < frames_.size(); i++) {
    if (frames_[i].get() == frame)
      return i - hide_ambiguous_inline_frame_count_;
  }
  return std::nullopt;
}

size_t Stack::InlineDepthForIndex(size_t index) const {
  FX_DCHECK(index < frames_.size());
  for (size_t depth = 0; index + depth < frames_.size(); depth++) {
    if (!frames_[index + depth]->IsInline())
      return depth;
  }

  FX_NOTREACHED();  // Should have found a physical frame that generated it.
  return 0;
}

FrameFingerprint Stack::GetFrameFingerprint(size_t virtual_frame_index) const {
  size_t frame_index = virtual_frame_index + hide_ambiguous_inline_frame_count_;

  // Should reference a valid index in the array.
  if (frame_index >= frames_.size()) {
    FX_NOTREACHED();
    return FrameFingerprint();
  }

  // The inline frame count is the number of steps from the requested frame index to the current
  // physical frame.
  size_t inline_count = InlineDepthForIndex(frame_index);

  return FrameFingerprint(frames_[frame_index]->GetCanonicalFrameAddress(), inline_count);
}

size_t Stack::GetAmbiguousInlineFrameCount() const {
  // This can't be InlineDepthForIndex() because that takes an index relative to the
  // hide_ambiguous_inline_frame_count_ and this function always wants to return the same thing
  // regardless of the hide count.
  for (size_t i = 0; i < frames_.size(); i++) {
    if (!frames_[i]->IsAmbiguousInlineLocation())
      return i;
  }

  // Should always have a non-inline frame if there are any.
  FX_DCHECK(frames_.empty());
  return 0;
}

void Stack::SetHideAmbiguousInlineFrameCount(size_t hide_count) {
  FX_DCHECK(hide_count <= GetAmbiguousInlineFrameCount());
  hide_ambiguous_inline_frame_count_ = hide_count;
}

void Stack::SyncFrames(const SyncFrameOptions& opts, fit::callback<void(const Err&)> callback) {
  if (opts.force_update)
    force_update_in_progress_ = true;
  delegate_->SyncFramesForStack(opts, std::move(callback));
}

void Stack::EnsureFrames(const SyncFrameOptions& opts, fit::callback<void(const Err&)> callback) {
  if (!opts.force_update && has_all_frames()) {
    debug::MessageLoop::Current()->PostTask(
        FROM_HERE, [callback = std::move(callback)]() mutable { callback(Err()); });
    return;
  }

  SyncFrames(opts, std::move(callback));
}

void Stack::SetFrames(debug_ipc::ThreadRecord::StackAmount amount,
                      const std::vector<debug_ipc::StackFrame>& new_frames) {
  bool initial_has_frames = !frames_.empty();

  // See if the new frames are an extension of the existing frames or are a replacement. This
  // avoids overwriting existing frames when possible because:
  //  - There may be in-process operations with weak references to the frame that we don't want
  //    to invalidate.
  //  - Inline frame state, especially with the hide_ambiguous_inline_frame_count_, needs to
  //    stay the same. Keeping this state across frame replacements and potential symbol changes
  //    isn't possible.
  size_t old_ambiguous_frame_count = GetAmbiguousInlineFrameCount();
  size_t old_hide_ambiguous_inline_frame_count = hide_ambiguous_inline_frame_count_;
  if (force_update_in_progress_) {
    // Replace all frames.
    hide_ambiguous_inline_frame_count_ = 0;
    frames_.clear();
  }

  size_t appending_from = 0;  // First index in new_frames to append.
  for (size_t i = 0; i < frames_.size(); i++) {
    // The input will not contain any inline frames so skip over those when doing the checking.
    if (frames_[i]->IsInline())
      continue;

    if (appending_from >= new_frames.size() ||
        frames_[i]->GetAddress() != new_frames[appending_from].ip ||
        frames_[i]->GetStackPointer() != new_frames[appending_from].sp) {
      // New frames are not a superset of our existing stack, replace everything.
      hide_ambiguous_inline_frame_count_ = 0;
      frames_.clear();
      appending_from = 0;
      break;
    }

    appending_from++;
  }

  for (size_t i = appending_from; i < new_frames.size(); i++)
    AppendFrame(new_frames[i]);

  has_all_frames_ = amount == debug_ipc::ThreadRecord::StackAmount::kFull;

  if (force_update_in_progress_ && old_ambiguous_frame_count == GetAmbiguousInlineFrameCount()) {
    // When forcing a refresh of the stack frames, optimistically assume that the actual stack
    // hasn't changed and the user wants the existing one re-evaluated (maybe with new symbols).
    // When the number of inline frames at the top of the stack hasn't changed, assume the old hide
    // count is also still valid and keep it from before.
    hide_ambiguous_inline_frame_count_ = old_hide_ambiguous_inline_frame_count;
  }

  force_update_in_progress_ = false;

  // Skip sending notifications when 0 frames replaced 0 frames.
  if (new_frames.size() != 0 || initial_has_frames)
    delegate_->DidUpdateStackFrames();
}

void Stack::SetFramesForTest(std::vector<std::unique_ptr<Frame>> frames, bool has_all) {
  frames_ = std::move(frames);
  has_all_frames_ = has_all;
  hide_ambiguous_inline_frame_count_ = 0;
  delegate_->DidUpdateStackFrames();
}

void Stack::ClearFrames() {
  has_all_frames_ = false;
  hide_ambiguous_inline_frame_count_ = 0;

  if (frames_.empty())
    return;  // Nothing to do.

  frames_.clear();
  delegate_->DidUpdateStackFrames();
}

Location Stack::SymbolizeFrameAddress(
    uint64_t address, debug_ipc::StackFrame::AddressType pc_is_return_address) const {
  // Adjust locations for non-topmost frames because we should display the locations of function
  // calls instead of return addresses. Inlined functions should also be expanded from the call
  // sites rather than the return sites.
  uint64_t address_to_symbolize = address;
  if (pc_is_return_address == debug_ipc::StackFrame::AddressType::kReturn)
    address_to_symbolize -= 1;

  // The symbols will provide the location for the innermost inlined function.
  Location loc = delegate_->GetSymbolizedLocationForAddress(address_to_symbolize);

  // Restore the actual address so that the register value and commands like "disassemble" are
  // still correct.
  if (pc_is_return_address == debug_ipc::StackFrame::AddressType::kReturn)
    loc.AddAddressOffset(1);

  return loc;
}

void Stack::AppendFrame(const debug_ipc::StackFrame& record) {
  // This symbolizes all stack frames since the expansion of inline frames depends on the symbols.
  // Its possible some stack objects will never have their frames queried which makes this duplicate
  // work. A possible addition is to just save the debug_ipc::StackFrames and only expand the inline
  // frames when the frame list is accessed.

  Location inner_loc = SymbolizeFrameAddress(record.ip, record.pc_is_return_address);

  const Function* cur_func = inner_loc.symbol().Get()->As<Function>();
  if (!cur_func) {
    // No function associated with this location.
    frames_.push_back(delegate_->MakeFrameForStack(record, inner_loc));
    return;
  }

  // The Location object will reference the most-specific inline function but
  // we need the whole chain.
  std::vector<fxl::RefPtr<Function>> inline_chain = cur_func->GetInlineChain();
  if (inline_chain.back()->is_inline()) {
    // A non-inline frame was not found. The symbols are corrupt so give up on inline processing and
    // add the physical frame only.
    frames_.push_back(delegate_->MakeFrameForStack(record, inner_loc));
    return;
  }

  // Need to make the base "physical" frame first because all of the inline frames refer to it.
  auto physical_frame = delegate_->MakeFrameForStack(
      record, LocationForInlineFrameChain(inline_chain, inline_chain.size() - 1, inner_loc));

  // Add inline functions (skipping the last which is the physical frame made above).
  for (size_t i = 0; i < inline_chain.size() - 1; i++) {
    frames_.push_back(std::make_unique<InlineFrame>(
        physical_frame.get(), LocationForInlineFrameChain(inline_chain, i, inner_loc)));
  }

  // Physical frame goes last (back in time).
  frames_.push_back(std::move(physical_frame));
}

}  // namespace zxdb
