// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This test decodes a vp9 stream that was created by an Exynos HW encoder. Per b/431253839, the
// stream has a "prob stream header with more than one extra padding data". The current
// video_ucode.bin md5sum 9a4966e2fa40ae302562868befd4d4cc generates incorrect output frames. A
// candidate video_ucode.bin md5sum f06b7989fa8d253316ce01b313b7b551 generates only frame 0 then
// seems to get stuck (at least so far).
//
// This test will be enabled once we have a video_ucode.bin + driver that pass this test.
//
// If this test breaks and it's not immediately obvious why, please feel free to involve
// dustingreen@ (me) in figuring it out.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/media/codec_impl/fourcc.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>
#include <stdio.h>
#include <stdlib.h>

#include <map>
#include <set>

#include "../use_video_decoder.h"
#include "../util.h"
#include "use_video_decoder_test.h"

namespace {

constexpr char kInputFilePath[] = "/pkg/data/mirroring_exynos.ivf";
// This is from the following command:
//   * ffprobe mirroring_exynos.ivf -show_frames | grep media_type=video | wc -l
constexpr int kInputFileFrameCount = 100;

// This hash was obtained from the following commands:
//   * ffmpeg -i mirroring_exynos.ivf -vsync 0 -pix_fmt yuv420p -f rawvideo mirroring.yuv420p
//   * sha256sum mirroring.yuv420p
const char* kGoldenSha256 = "cc78ae2836c6196319028074f7063e03ce540a47bcd4437a14f4510124ebfe6a";

// Must be nullptr terminated.
const char* kPerFrameGoldenSha256[] = {
    "6902b0323250e47c986b725596694e7dbda0a9255b5dacc73507c9350c1bd8be",
    "b332b6e9e1bf509ab1a6dd63f09703f765a033cfaea645e7aa61bf9836edfece",
    "353dd1e1c11eb6e8bc348747bb93e0d52d4053cd1eb8d94e4e572f8171525cd4",
    "fbe51bad62460fa7aca018933541d93e90865500917612cddb6d403d594fe9ef",
    "e4e46752209f3b02206488c2f2777680bf8ca69328e4c1abf52c7d83ea489a81",
    "5a660fa4bcc9ae7d43475241fd11f12f8730522e12c8dd9508e87c3018441cc7",
    "68d9ced1aea0157dfc0f27ecad12970bc957c1bbd6b0252713d9e74ee1d58916",
    "4207f5e82c1ba78ae6c2f15aecc2e24deed952ff3676555e1b903d6cae487851",
    "e1037524f9007ffbef82f5ab2353f27bb007eb2e78dd6e078355b453877cee1b",
    "ca4d6e2d712086f7104634469b45ae42bb720608eb3df4ba8bdb29c0a07f121d",
    "7c52f388b0173d09abebc0b4ae63696db4f3bf5ef1d6ab6d9931fd0eaaff6c3f",
    "2dcb2a37b7d39c2494df78f352bb2870d9ee3a23b8fdcc509ae68bb6f8217038",
    "342512c7f3522988b1bc1a15d448f9e65727d2afa5d2911ef5c67740c705e082",
    "b4b97da628e61922bc8cf366e1b6a2685a9866bb83aa607d037e7ba124d02d54",
    "310cf13fc3dfff44fb631875ce99c3c3655fdbcdc530fc8dca9ea93c09b0c4c9",
    "b633b979f1d5ff211aedd9dcf85875289d214df84c9047e9202789464bb469af",
    "8804c54df08cba5518c75c4ff791366c8f69c049bbfcd636e10bc0d971d58181",
    "de63706371c8a5c787d48724739b04cc7dbbc4e1814223e24e054134c62d37fe",
    "d33e7b5c6d1a518a9a4505bc18f03e515bcaee3913a921dc66e023f689e07603",
    "67a048da70ac97aba4f2ecb35a9435e380b5fac71350d52a7183e753231e9af3",
    "deb97dafb7c6c0ba8955cb9b5a9871a733d773b7d5880a8edd363586db61caa4",
    "e43d7223ff015d9e17d08e4e21870ba2288d64da7de38ce66036e857509aa2e7",
    "b3b7cacdd87f49412b93e5dd872846dfa941ea96e5176449ed957effecaffe61",
    "0839d4509a8043492dc03c4659d1a0cf91c211cfe56b9abc47c403d60473c0e7",
    "b8fbebe150741f04389dd8c6cc62a7edb051ac780c8f6d2c637423cbfca6af08",
    "c0654e5d0c794803cb7bec18aec2c73bf866258c262958d237c34658a9b57a53",
    "e58d80de59115aba51a0b76fc7ec923eb6edb76280878b38e67c5753eb5ec4e3",
    "c3257a980a72d7f458f27e7e8af590746b06f9e9f98f1d4b2d4e8c5d939a657d",
    "662c5324417b94950619a7596cc88e9351c15bce217df7b44bf4ebd90de716ec",
    "216b2283a3772ebc01bceba63b1acde7dbdc3c36a1cda652f5d916542534550a",
    "be29b1221e9962a7aa9f31cc9847e40a4e0fc4678a375a62aad043feac5efda2",
    "13bdf0d4f7ea3c04211e2071f46560e8b8a3824ce55337858f6e6698c4578d9f",
    "df4d2c4f400579df7beb0bbb7cb28adecdbc33b0196c3af39bcf434eecd61ac5",
    "5327b8a3eb025ca4274063d2ff53ffd391abbb337caa69ab46f92b7f7755ae1b",
    "5c1c16a7e1d28b886e0af4b488923502a0604eb5809efd7485f857637a0907e6",
    "4224729c431631e592a8664e9b434e669f494401e8bfd1a32e8ecd6d25f6d892",
    "61c78f0b2ce6d030f59b08353e661f591d7fa9630337e4fd46f567b7fca45f3d",
    "2c4738ad65ee16b3dde3e2d138c742301ac205f0195bc3ec65f906efe2fc1a42",
    "28f25da207db2c8f26576dc234db1da074695599bb05ae9cdf79b77866b5c761",
    "c9ea5dc0c4608b5525a642a18fd9ad535ef33fe4a4a9e2152d499435dae6a3eb",
    "706ac5b415a217d05a953413aa2f7c20628ddabea7d18254a63b23a34219a56b",
    "089dbd9c05b0dee9f347a7e8f98d72d6723449003349c69cabc48ee34b1aade2",
    "c11e364fe503c67009ef54ca7ae9dcb345f6d8bd5c7ff12b51c50c80ddf8c622",
    "044077358443d2ba531c854e8ac995a12e21d297017a9a7c6c519296c8bfe7fd",
    "064ae803319d90ebebac07fae279331c85a44ed5a349ef0e4d460a167bc832a3",
    "60fd54904225d84b9279d59c03b51dae7358280890edd657b9eb2102cb6aff04",
    "6097499f3c0bbf112783396fe878f5d6b5ffbe935c1b9cce0573543a5497de5a",
    "8830296163615b2b928b10380a1aaa05cb39e41684a5435058c63a74cad0a83c",
    "d2d253c8cceda96673736f7e90e9f8f3cbf9b4702af104155b150716f22d1f63",
    "3ec7e4f318b210ee594c85364e2a177ada1c982c1b1dbebac502b40ee55883ca",
    "db84d53824d9eab255070f64167ae7d622db09469a1802b5bcce71674c9b76b8",
    "dd1b4be29d1848a8dcd33f402611bbd0d1f01abb23f8972d8fe31e82d2de845a",
    "2c7de1bcedb04cf478584a85c6d910a4c0034a9c71bfae86742ca917e2b49b43",
    "2a4609ba42b87a48e66f2cdb4ddeace8f64dd6445d6d40b234cf80e69b68a464",
    "cf57d556b1e5159d03b39804dc6d048417a7f15eeb2cb44e266fb0f5b93d03dc",
    "73126e3b9b0130c905cf241cf887e97ad19df1d203cad10c8837c3a3896280b5",
    "02406e88276d43d451429eeb0cd450d491f8638d7e738f23c224c8e1236a1447",
    "58aa2e02471821ade2c0989c36023e8fa6a8bf946276fd8e079d02cb00af8e95",
    "af3df77d811de0a049e05665803bb84c614c02f9a6b5653ce1dee8c856d51ae6",
    "fa64a394093637193dcf4bdb83709d67f27ebb22b030a61bcc511cab0dede13e",
    "4e2aa702975a6f47af2cf58c480cfb14f213567f170eacb4a15863764db10a85",
    "76ac9df7ddf464b3d1279da460a1e5635dc83feffcbc72d5797c27390b3893a1",
    "2f7a59072fdba9ac0bc834998a898626f653c5838d67e9daf1352cc23ee7b49a",
    "bef0758027e63abe56272c085e304112393506895a1af75190a19a89ce579dd5",
    "3cf2413c72fe57350cfc00c00089754c17966e3680b33d4b6329899af543d5e9",
    "4fa97ee9a535fc704a4216ca6a647e9d5791d598134a358355249e004f64fb88",
    "2afb262e0937a32e766c6c1e2cfadb3ca9c827243658b40045b46b66280fe310",
    "8105b41b637cc912124973aff0b01cfc8f9386a46edbf4982522e25dc56ae307",
    "7059b91d18edc1e3286ca667964253aa6f10f5d3fe839b361372e101b7ffb1d7",
    "c1b195832f2228693ec0eaac8da5637cfeda60760695fbaed42e736e558feda6",
    "8516be5feb1cd3961df2ba103f1b46d921dcb19613c452ae7402f002d3932988",
    "93a0b5d773487c0d570ea3ed1845cca0a6b36e78f04df14ba90f899b7a7dce6b",
    "d882488a16164d04a9d2ebe3f61c4196f584262a259b7475186c164a5b6f41db",
    "eab521c5c41d37419a1b5a109a79d63f7c63ffd4a6fc23cf3136a9e2b401a101",
    "e99fefc719a31912e7bff11d742454f474d88546d00a3cf337020b592b583945",
    "2ca0238e9c298e08a973fb1495bd64533b773fe67c7f705201aa7732658aa0b2",
    "04ce7e04636c5f4858552cce917343784562cacc1851ccf9ce2780d4d7e18c3d",
    "1f169d4cde4539b142c715022dfca08d898580999a8901fa16426632bd758433",
    "c57523e7a9e50b3b61a66126a97d7a6c63f5592190f570f05d3b09c137208b07",
    "2badefa88229176ae16965dbd757724c941ddc9e20726ebe21d263b798181827",
    "df324c83ac14bddd843ab3d6767ead132b43cea07061d0d71d658f4f87d8a397",
    "dc46b1d8b8828192d3016b9caf1abe892e03f75e2c98eacdbd6a764fc715cd58",
    "cda82ec2bb4d8bbc32c9888733f33d7c6a4f293a75b0e4b0f01f1d33227f4651",
    "cc702246181506b9747f9ae0004c06e3aefa88e61361c6c88d20eca5fd60615a",
    "f78eed8144a49dcf28a981697189aa80cd7b6e7a9869ec9dd38372bbccd5b97f",
    "75ad340c532ec69339eecdf5bb0192bfce1871b28affa527853ee9cdab381141",
    "cd6749806fb5538b1b5fe9ad0022910f65ab545f727dac61e23cbacfcf69e8bd",
    "da374b26bab0e0a909b840e4a2c630b6dcde5a00452a5d6bc2870b4062bcc286",
    "521f1911c68b99e9738f01553ff8f29a14ac84bbfb57cb63a248ab31cf514834",
    "2e587b6e702dcf8b1de03a6d2c10fefb185b4c98731e8cdac14b298c7604bba1",
    "062d9c87c40b58ee3696e16c844109a0343f8d692b982bcd236ba1f739a65917",
    "e1f4f2bfd59b2798e5e2d974e457a2534c8ca86ab81456550dc2086221456d22",
    "7d939fa0d3a587fc17dbc3fcf1be539d921af0dcba8c7faa85cce38fe37de0a5",
    "07f081e250b2a45e1879f71e1d425c4e4aacb1eb0574d1e04be8679de6e20490",
    "1981873b63f395cc330fdc8b2bf198a121763d700879294179e08b257259f57c",
    "ab067d4ae5599ab9a4aaff4789cda6c97df57ba2af58c03bceb94e3db11c9b8a",
    "212fea5d1d889100d0180b61864dc27c22c5463b304935c59e55e0910415fca4",
    "fc10a95b5eba53ce7184c7579053e959aa50019aa11ff7f1cbd2b7ad8512fee1",
    "f7b8be73b240efc1cc5079c0df378b123afd1f088a0989401212ce74ad704a3a",
    "cc78ae2836c6196319028074f7063e03ce540a47bcd4437a14f4510124ebfe6a",
    nullptr,
};

}  // namespace

int main(int argc, char* argv[]) {
  UseVideoDecoderTestParams test_params = {
      // This setting can be useful when debugging a HW decoder stall, to avoid unnecessarily
      // spamming
      // the log and the test stdout with tons of input that has no corresponding generated output
      // (yet):
      //
      // .input_stop_stream_after_frame_ordinal = 4,

      // For vp9, max_num_reorder_frames_threshold 1 means no frame reordering.
      .max_num_reorder_frames_threshold = 1,
      .per_frame_golden_sha256 = kPerFrameGoldenSha256,
      .golden_sha256 = kGoldenSha256,
  };
  return use_video_decoder_test(kInputFilePath, kInputFileFrameCount, use_vp9_decoder,
                                /*is_secure_output=*/false, /*is_secure_input=*/false,
                                /*min_output_buffer_count=*/0, &test_params);
}
