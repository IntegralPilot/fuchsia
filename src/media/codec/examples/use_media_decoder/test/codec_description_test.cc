// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/media/codec_impl/fourcc.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>
#include <stdio.h>
#include <stdlib.h>

#include <map>
#include <set>

#include <fbl/string_printf.h>

#include "../use_video_decoder.h"
#include "../util.h"
#include "use_video_decoder_test.h"

// If a detailed description for one of these 'mime_type'(s) is provided, it must have
// profile_descriptions.  The 'mime_type' for any codec with profiles should be in this list.
// These are not proper mime types, so the name is a misnomer; these just need to match the
// 'mime_type' reported by the codecs.  (We may switch to an extensible enum for this in V2.)
//
// Ideally all codecs that are these types would have a detailed description and also profiles, but
// currently since not all codecs provide detailed descriptions, we can only require that all
// codecs with these types that do provide a detailed description also provide profiles.
const std::set<std::string> mime_types_requiring_profiles = {"video/h264", "video/vp9",
                                                             "video/av1"};

std::string CodecPropertiesToString(fuchsia::mediacodec::CodecType codec_type,
                                    std::string mime_type, bool is_hw) {
  return std::string(fbl::StringPrintf("codec_type: %s : mime_type: %s : is_hw %u",
                                       codec_type == fuchsia::mediacodec::CodecType::DECODER
                                           ? "DECODER"
                                           : "ENCODER",
                                       mime_type.c_str(), is_hw)
                         .c_str());
}

int main(int argc, char* argv[]) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  auto component_context = sys::ComponentContext::Create();

  fuchsia::mediacodec::CodecFactoryHandle codec_factory;
  component_context->svc()->Connect<fuchsia::mediacodec::CodecFactory>(codec_factory.NewRequest());
  auto codec_factory_ptr = codec_factory.Bind();

  bool is_on_codec_list_seen = false;
  std::vector<fuchsia::mediacodec::CodecDescription> codec_list;
  codec_factory_ptr.events().OnCodecList =
      [&is_on_codec_list_seen,
       &codec_list](std::vector<fuchsia::mediacodec::CodecDescription> codec_list_param) {
        ZX_ASSERT(!is_on_codec_list_seen);
        is_on_codec_list_seen = true;
        codec_list = codec_list_param;
      };

  zx::time start_monotonic = zx::clock::get_monotonic();
  while (true) {
    loop.RunUntilIdle();
    auto wait_duration = zx::clock::get_monotonic() - start_monotonic;
    if (is_on_codec_list_seen || wait_duration > zx::sec(20)) {
      break;
    }
    zx::nanosleep(zx::deadline_after(zx::msec(10)));
  }
  ZX_ASSERT(is_on_codec_list_seen);

  // Wait a bit longer to check that CodecFactory is only sending OnCodecList() once near the start
  // per connection (to degree we can really check for this). As we're deprecating OnCodecList()
  // event-style in favor of a getter instead, the OnCodecList() will likely never be allowed to be
  // sent more than once by CodecFactory å(vs. a getter which can be called again based on a ping
  // event when/if the set of codecs changes, or similar).
  //
  // If OnCodecList() happens a second time within 1 second, we'll fail the assert above in the
  // OnCodecList lambda.
  loop.Run(zx::deadline_after(zx::sec(1)));
  // OnCodecList has been received once.

  fuchsia::mediacodec::CodecFactoryGetDetailedCodecDescriptionsResponse descriptions_response;
  bool is_get_done = false;
  codec_factory_ptr->GetDetailedCodecDescriptions(
      [&is_get_done, &descriptions_response](
          fuchsia::mediacodec::CodecFactoryGetDetailedCodecDescriptionsResponse response) {
        descriptions_response = std::move(response);
        is_get_done = true;
      });

  start_monotonic = zx::clock::get_monotonic();
  while (true) {
    loop.RunUntilIdle();
    if (is_get_done) {
      printf("is_get_done\n");
      break;
    }
    auto wait_duration = zx::clock::get_monotonic() - start_monotonic;
    if (wait_duration > zx::sec(20)) {
      break;
    }
    zx::nanosleep(zx::deadline_after(zx::msec(10)));
  }
  ZX_ASSERT(is_get_done);

  uint32_t codec_description_count = 0;
  if (descriptions_response.has_codecs()) {
    codec_description_count = descriptions_response.codecs().size();
  }

  printf("\ncodec_list:\n");
  for (auto& codec : codec_list) {
    printf("codec.mime_type: %s\n", codec.mime_type.c_str());
  }
  printf("end codec_list\n");

  printf("\ndescriptions_response.codecs():\n");
  for (auto& description : descriptions_response.codecs()) {
    printf("description.codec_type: %s\n",
           description.codec_type() == fuchsia::mediacodec::CodecType::DECODER ? "DECODER"
                                                                               : "ENCODER");
    printf("description.mime_type: %s\n", description.mime_type().c_str());
  }
  printf("end descriptions_response.codecs()\n\n");

  // Not all codecs will provide the detailed codec description yet, but every codec will be present
  // in OnCodecList(), so the number from OnCodecList() must be >= the number of detailed
  // descriptions.
  printf("codec_list.size(): %zu codec_description_count: %u\n", codec_list.size(),
         codec_description_count);
  fflush(nullptr);
  ZX_ASSERT(codec_list.size() >= codec_description_count);

  // For every codec detailed description, it must be possible to find the same codec in the list
  // from OnCodecList, and vice versa.  Since this mapping can potentially be somewhat ambiguous
  // depending on how many of each codec type exist, we verify based on counts per properties we can
  // get from both lists.
  std::map<std::string, uint32_t> on_codec_list_codecs;
  std::map<std::string, uint32_t> get_detailed_codec_descriptions_codecs;

  printf("\n");

  for (auto& description : descriptions_response.codecs()) {
    ZX_ASSERT(description.has_codec_type());
    ZX_ASSERT(description.has_mime_type());
    ZX_ASSERT(description.has_is_hw());
    printf("mime_type: %s has_profile_descriptions: %u\n", description.mime_type().c_str(),
           description.has_profile_descriptions());
    if (mime_types_requiring_profiles.find(description.mime_type()) !=
        mime_types_requiring_profiles.end()) {
      ZX_ASSERT(description.has_profile_descriptions());
    }
    auto codec_string = CodecPropertiesToString(description.codec_type(), description.mime_type(),
                                                description.is_hw());
    // In case the assert below fails.
    printf("OnCodecList codec_string: %s\n", codec_string.c_str());
    ++get_detailed_codec_descriptions_codecs[codec_string];
  }

  printf("\n");

  for (auto& codec : codec_list) {
    auto codec_string = CodecPropertiesToString(codec.codec_type, codec.mime_type, codec.is_hw);
    // In case the assert below fails.
    printf("GetDefailedCodecDescriptions codec_string: %s\n", codec_string.c_str());
    ++on_codec_list_codecs[codec_string];
  }

  ZX_ASSERT(on_codec_list_codecs == get_detailed_codec_descriptions_codecs);

  return 0;
}
