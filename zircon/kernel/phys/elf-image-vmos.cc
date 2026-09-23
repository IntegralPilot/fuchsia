// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/llvm-profdata/llvm-profdata.h>

#include <ktl/string_view.h>
#include <phys/elf-image.h>
#include <phys/symbolize.h>

#include <ktl/enforce.h>

template <const ktl::string_view& kSinkName, const ktl::string_view& kSuffix,
          const ktl::string_view& kAnnounce>
ElfImage::PublishSelfCallback ElfImage::MakePublishSelfCallback(
    ElfImage::PublishDebugdataFunction& publish_debugdata) const {
  return [this, &publish_debugdata](size_t stream_size) {
    gSymbolize->DumpFile(kAnnounce, stream_size, kSinkName, name_, kSuffix);
    return publish_debugdata(kSinkName, name_, kSuffix, stream_size);
  };
}

// PublishDebugdata is called in the module collecting the instrumentation VMOs
// and so this code is in *that* module's instantiation, not the `*this`
// module.  The publish_self_ member points to the PublishSelf static method as
// instantiated inside the `*this` module's code (see elf-image-self-vmos.cc).
// We call into the target module so it can determine the size of the VMO it
// needs to create, then it calls us back to publish it and yield the span of
// data to fill in.
void ElfImage::PublishDebugdata(PublishDebugdataFunction publish_debugdata) const {
  if (publish_self_) {
    publish_self_(*this,
                  MakePublishSelfCallback<LlvmProfdata::kDataSinkName, LlvmProfdata::kFileSuffix,
                                          LlvmProfdata::kAnnounce>(publish_debugdata));
  }
}
