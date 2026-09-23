// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include <zircon/types.h>

#include "src/media/audio/audio_core/device_id.h"
#include "src/media/audio/audio_core/test/api/audio_renderer_pipeline_test_shared.h"
#include "src/media/audio/lib/analysis/generators.h"

namespace media::audio::test {

// Validate that a slow effects pipeline registers an underflow. To guarantee that this occurs, this
// test must be run in an environment that is real-time capable, which means that it should not be
// run on either emulators or DEBUG builds.
TEST_F(AudioRendererPipelineUnderflowTest, HasUnderflow) {
  // Inject one packet and wait for it to be rendered.
  auto input_buffer = GenerateSequentialAudio(format(), kPacketFrames);
  auto packets = renderer()->AppendSlice(input_buffer, kPacketFrames);
  renderer()->PlaySynchronized(this, output(), 0);
  renderer()->WaitForPackets(this, packets);

  // Wait an extra 20ms to account for the sleeper filter's delay.
  RunLoopWithTimeout(zx::msec(20));

  // Expect an underflow.
  ExpectInspectMetrics(output(), DeviceUniqueIdToString(kUniqueId),
                       {
                           .children =
                               {
                                   {kPipelineUnderflows, {.nonzero_uints = {kCount}}},
                               },
                       });
  Unbind(renderer());
}

}  // namespace media::audio::test
