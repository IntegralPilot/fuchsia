# Driver for AMLogic g12 audio

This folder contains drivers for the AMLogic g12 audio subsystem. There are drivers available for:

1. The [Audio Streaming Interface](https://fuchsia.dev/fuchsia-src/development/audio/drivers/streaming.md)
   driver type implemented in audio-stream* files as a DFv1 driver.
2. The [Digital Audio Interface](https://fuchsia.dev/fuchsia-src/development/audio/drivers/dai.md) driver
   type implemented in the dai* files as a DFv1 driver.
3. The [Audio Composite](https://fuchsia.dev/fuchsia-src/development/audio/drivers/composite.md) driver type
   implemented in the composite* files as a DFv2 driver.

See [Audio Codec Interface](https://fuchsia.dev/fuchsia-src/development/audio/drivers/codec.md) for a
description of codec terms used in this driver.

See [aml-audio.h](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/lib/amlogic/include/soc/aml-common/aml-audio.h)
for descriptions of AMLogic specific metadata used in DFv1 drivers.
