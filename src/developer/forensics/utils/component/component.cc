// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/utils/component/component.h"

#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/interface_request.h>
#include <lib/syslog/cpp/macros.h>

#include <string>

#include "src/lib/files/directory.h"
#include "src/lib/files/file.h"

namespace forensics {
namespace component {
namespace {

constexpr char kComponentDirectory[] = "/tmp/component";
constexpr char kInstanceIndexPath[] = "/tmp/component/instance_index.txt";

}  // namespace

Component::Component()
    : loop_(&kAsyncLoopConfigAttachToCurrentThread),
      dispatcher_(loop_.dispatcher()),
      context_(sys::ComponentContext::CreateAndServeOutgoingDirectory()),
      inspector_(dispatcher_, {}),
      clock_(),
      instance_index_(InitialInstanceIndex()) {
  WriteInstanceIndex();
}

Component::Component(async_dispatcher_t* dispatcher, std::unique_ptr<sys::ComponentContext> context)
    : loop_(&kAsyncLoopConfigNeverAttachToThread),
      dispatcher_(dispatcher),
      context_(std::move(context)),
      inspector_(dispatcher_, {}),
      clock_(),
      instance_index_(InitialInstanceIndex()) {
  WriteInstanceIndex();
}

async_dispatcher_t* Component::Dispatcher() { return dispatcher_; }

std::shared_ptr<sys::ServiceDirectory> Component::Services() { return context_->svc(); }

inspect::Node* Component::InspectRoot() { return &(inspector_.root()); }

timekeeper::Clock* Component::Clock() { return &clock_; }

bool Component::IsFirstInstance() const { return instance_index_ == 1; }

zx_status_t Component::RunLoop() { return loop_.Run(); }

void Component::ShutdownLoop() { return loop_.Shutdown(); }

size_t Component::InitialInstanceIndex() const {
  // The default is this is the first instance.
  size_t instance_index{1};
  if (!files::IsDirectory(kComponentDirectory) && !files::CreateDirectory(kComponentDirectory)) {
    FX_LOGS(INFO) << "Unable to create " << kComponentDirectory
                  << ", assuming first instance of component";
    return instance_index;
  }

  std::string starts_str;
  if (files::ReadFileToString(kInstanceIndexPath, &starts_str)) {
    instance_index = std::stoull(starts_str) + 1;
  }

  return instance_index;
}

void Component::WriteInstanceIndex() const {
  files::WriteFile(kInstanceIndexPath, std::to_string(instance_index_));
}

}  // namespace component
}  // namespace forensics
