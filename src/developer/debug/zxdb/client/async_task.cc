// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/async_task.h"

namespace zxdb {

AsyncTask::AsyncTask(Session* session) : ClientObject(session), weak_factory_(this) {}

AsyncTask::~AsyncTask() = default;

fxl::WeakPtr<AsyncTask> AsyncTask::GetWeakPtr() { return weak_factory_.GetWeakPtr(); }

}  // namespace zxdb
