// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/llvm-profdata/llvm-profdata.h>
#include <zircon/assert.h>

#include <ktl/utility.h>
#include <phys/elf-image.h>
#include <phys/symbolize.h>

#include <ktl/enforce.h>

// This code gets instantiated separately in each module when it calls
// AfterHandoff.  We try to keep only the code that needs to be built
// separately in each module in this file, in hopes that it will minimize the
// amount of code each module needs.  Most of the publication code only needs
// to be in the module that actually calls the PublishInstrumentation method.

void ElfImage::PublishSelf(const ElfImage& module, PublishSelfCallback llvmprofdata) {
  module.PublishSelfLlvmProfdata(ktl::move(llvmprofdata));
}

// Publish llvm-profdata if present.
void ElfImage::PublishSelfLlvmProfdata(PublishSelfCallback publish) const {
  LlvmProfdata profdata;
  ZX_DEBUG_ASSERT(build_id_);
  profdata.Init(build_id_->desc);
  if (profdata.size_bytes() > 0) {
    ktl::span buffer = publish(profdata.size_bytes());

    // Copy the fixed data and live data values and then start updating the
    // handoff data in place.
    auto live_data = profdata.WriteFixedData(buffer);
    profdata.CopyLiveData(live_data);
    LlvmProfdata::UseLiveData(live_data);
  }
}
