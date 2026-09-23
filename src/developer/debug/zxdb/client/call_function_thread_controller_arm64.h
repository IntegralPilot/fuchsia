// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_CALL_FUNCTION_THREAD_CONTROLLER_ARM64_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_CALL_FUNCTION_THREAD_CONTROLLER_ARM64_H_

#include "src/developer/debug/zxdb/client/call_function_thread_controller.h"

namespace zxdb {

class CallFunctionThreadControllerArm64 : public CallFunctionThreadController {
 public:
  // Constructs the thread controller. |on_function_completed| is called once the thread state has
  // been restored to what it was before the function call, but before this controller has been
  // destroyed.
  CallFunctionThreadControllerArm64(const AddressRanges& range,
                                    const std::vector<ExprValue>& parameters,
                                    EvalCallback on_function_completed,
                                    fit::deferred_callback on_done = {});

  ~CallFunctionThreadControllerArm64() override;

  void InitWithThread(Thread* thread, fit::callback<void(const Err&)> cb) override;
  const char* GetName() const override { return "CallFunctionArm64"; }

  void CollectAllRegisterCategories(Thread* thread, fit::callback<void(const Err& err)> cb) final;

 private:
  Frame* PushStackFrame(uint64_t new_pc, uint64_t old_sp,
                        const std::vector<debug::RegisterValue>& regs) final;

  fxl::WeakPtrFactory<CallFunctionThreadControllerArm64> weak_factory_;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_CALL_FUNCTION_THREAD_CONTROLLER_ARM64_H_
