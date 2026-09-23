// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_TESTS_TEST_BASE_H_
#define SRC_MEDIA_AUDIO_DRIVERS_TESTS_TEST_BASE_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <fuchsia/hardware/audio/cpp/fidl.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/interface_handle.h>
#include <lib/sys/component/cpp/testing/realm_builder.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/device/audio.h>
#include <zircon/rights.h>

#include <optional>

#include <gtest/gtest.h>

#include "src/media/audio/drivers/tests/durations.h"
#include "src/media/audio/lib/test/test_fixture.h"

namespace media::audio::drivers::test {

inline constexpr size_t kUniqueIdLength = 16;

// We enable top-level methods (e.g. TestBase::Retrieve[RingBuffer|Dai]Formats, TestBase::
// RetrieveProperties, AdminTest::RequestBuffer) to skip or produce multiple errors and then
// cause a test case to exit-early once they return, even if no fatal errors were triggered.
// Gtest defines NO macro for this case -- only ASSERT_NO_FATAL_FAILURE -- so we define our own.
// Macro definition in headers is discouraged (at best), but this is used in local test code only.
#define ASSERT_NO_FAILURE_OR_SKIP(statement, ...)          \
  do {                                                     \
    statement;                                             \
    if (TestBase::HasFailure() || TestBase::IsSkipped()) { \
      return;                                              \
    }                                                      \
  } while (0)

enum DriverType : uint8_t {
  Codec = 0,
  Composite = 1,
  Dai = 2,
  StreamConfigInput = 3,
  StreamConfigOutput = 4,
};

enum DeviceType : uint8_t {
  A2DP = 0,
  BuiltIn = 1,
  Virtual = 2,
};

struct DeviceEntry {
  std::variant<std::monostate, fidl::UnownedClientEnd<fuchsia_io::Directory>> dir;
  std::string filename;
  DriverType driver_type;
  DeviceType device_type;

  bool isA2DP() const { return device_type == DeviceType::A2DP; }
  bool isVirtual() const { return device_type == DeviceType::Virtual; }

  bool isCodec() const { return driver_type == DriverType::Codec; }
  bool isComposite() const { return driver_type == DriverType::Composite; }
  bool isDai() const { return driver_type == DriverType::Dai; }
  bool isStreamConfigInput() const { return driver_type == DriverType::StreamConfigInput; }
  bool isStreamConfigOutput() const { return driver_type == DriverType::StreamConfigOutput; }
  bool isStreamConfig() const {
    return driver_type == DriverType::StreamConfigInput ||
           driver_type == DriverType::StreamConfigOutput;
  }

  bool operator<(const DeviceEntry& rhs) const {
    return std::tie(dir, filename, driver_type, device_type) <
           std::tie(rhs.dir, rhs.filename, rhs.driver_type, rhs.device_type);
  }
};

// Used in registering separate test case instances for each enumerated device
//
// See googletest/docs/advanced.md for details
//
// Devices are displayed in the 'audio-output/a1b2c3d4' format, with 'Virtual' as the filename if
// this is a virtualaudio instance we added, or 'A2DP' if this is a Bluetooth instance we added.
std::string inline DevNameForEntry(const DeviceEntry& device_entry) {
  std::string device_name =
      (device_entry.device_type == DeviceType::Virtual ? "Virtual" : device_entry.filename);

  switch (device_entry.driver_type) {
    case DriverType::Codec:
      return "codec/" + device_name;
    case DriverType::Composite:
      return "audio-composite/" + device_name;
    case DriverType::Dai:
      return "dai/" + device_name;
    case DriverType::StreamConfigInput:
      return "audio-input/" + device_name;
    case DriverType::StreamConfigOutput:
      return "audio-output/" + device_name;
  }
}
std::string inline TestNameForEntry(const std::string& test_class_name,
                                    const DeviceEntry& device_entry) {
  return DevNameForEntry(device_entry) + ":" + test_class_name;
}

// TestBase methods are used by both BasicTest and AdminTest cases.
class TestBase : public media::audio::test::TestFixture {
 public:
  explicit TestBase(const DeviceEntry& device_entry) : device_entry_(device_entry) {}

 protected:
  void SetUp() override;
  void TearDown() override;

  template <typename DeviceType, typename ConnectorType = void>
  fidl::InterfaceHandle<DeviceType> ConnectWithTrampoline(const DeviceEntry& device_entry);
  template <typename DeviceType>
  DeviceType Connect(const DeviceEntry& device_entry);
  void ConnectToBluetoothDevice();
  void CreateCodecFromChannel(fidl::InterfaceHandle<fuchsia::hardware::audio::Codec> channel);
  void CreateCompositeFromChannel(
      fidl::InterfaceHandle<fuchsia::hardware::audio::Composite> channel);
  void CreateDaiFromChannel(fidl::InterfaceHandle<fuchsia::hardware::audio::Dai> channel);
  void CreateStreamConfigFromChannel(
      fidl::InterfaceHandle<fuchsia::hardware::audio::StreamConfig> channel);

  const DeviceEntry& device_entry() const { return device_entry_; }
  DeviceType device_type() const { return device_entry_.device_type; }
  DriverType driver_type() const { return device_entry_.driver_type; }

  std::optional<bool> IsIncoming();

  void RequestHealthAndExpectHealthy();
  void GetHealthState(fuchsia::hardware::audio::Health::GetHealthStateCallback cb);

  // BasicTest (non-destructive) and AdminTest (destructive or RingBuffer) cases both need to
  // know at least whether ring buffers are outgoing or incoming, so this is implemented in this
  // shared parent class.
  void RetrieveProperties();
  void ValidateProperties();
  void DisplayBaseProperties();

  // BasicTest (non-destructive) and AdminTest (destructive or RingBuffer) cases both need to
  // know the supported formats, so this is implemented in this shared parent class.
  virtual void RetrieveDaiFormats();
  static void ValidateDaiFormatSets(
      const std::vector<fuchsia::hardware::audio::DaiSupportedFormats>& dai_format_sets);
  static void LogDaiFormatSets(
      const std::vector<fuchsia::hardware::audio::DaiSupportedFormats>& dai_format_sets,
      const std::string& tag = "");
  static void ValidateDaiFormat(const fuchsia::hardware::audio::DaiFormat& dai_format);
  static void LogDaiFormat(const fuchsia::hardware::audio::DaiFormat& format,
                           const std::string& tag = {});
  static fuchsia::hardware::audio::DaiFormat GetMinDaiFormat(
      const std::vector<fuchsia::hardware::audio::DaiSupportedFormats>& dai_format_sets);
  static fuchsia::hardware::audio::DaiFormat GetMaxDaiFormat(
      const std::vector<fuchsia::hardware::audio::DaiSupportedFormats>& dai_format_sets);
  const std::vector<fuchsia::hardware::audio::DaiSupportedFormats>& dai_formats() const;

  virtual void RetrieveRingBufferFormats();
  static void ValidatePcmSupportedFormats(
      const std::vector<fuchsia::hardware::audio::PcmSupportedFormats>& pcm_format_sets);
  void LogPcmFormat(const fuchsia::hardware::audio::PcmFormat& format, const std::string& tag);
  static fuchsia::hardware::audio::PcmFormat GetMinPcmFormat(
      const std::vector<fuchsia::hardware::audio::PcmSupportedFormats>& pcm_format_sets);
  static fuchsia::hardware::audio::PcmFormat GetMaxPcmFormat(
      const std::vector<fuchsia::hardware::audio::PcmSupportedFormats>& pcm_format_sets);
  const std::vector<fuchsia::hardware::audio::PcmSupportedFormats>& ring_buffer_pcm_formats()
      const {
    return ring_buffer_pcm_formats_;
  }

  std::vector<fuchsia::hardware::audio::PcmSupportedFormats>& ring_buffer_pcm_formats() {
    return ring_buffer_pcm_formats_;
  }

  std::vector<fuchsia::hardware::audio::DaiSupportedFormats>& dai_formats() { return dai_formats_; }

  fidl::InterfacePtr<fuchsia::hardware::audio::Codec>& codec() { return codec_; }
  fidl::InterfacePtr<fuchsia::hardware::audio::Composite>& composite() { return composite_; }
  fidl::InterfacePtr<fuchsia::hardware::audio::Dai>& dai() { return dai_; }
  fidl::InterfacePtr<fuchsia::hardware::audio::StreamConfig>& stream_config() {
    return stream_config_;
  }

  // Verifies that the driver channel remains healthy and responsive.
  //
  // Usage hints:
  // - Use this to ensure the driver hasn't disconnected after "fire-and-forget"
  //   commands (requests without a completion callback).
  // - Generally NOT needed if the previous step already waits for a response
  //   (e.g. ExpectCallbacks), as that implicitly verifies channel health.
  // - Can be used to flush pipelined requests or pending callbacks when no
  //   explicit response is expected.
  void WaitForError(zx::duration wait_duration = kWaitForErrorDuration) {
    // Instead of just polling for disconnect, we proactively confirm with a basic call & response.
    RequestHealthAndExpectHealthy();
  }

  // The union of [CodecProperties, CompositeProperties, DaiProperties, StreamProperties].
  struct BaseProperties {
    //       On codec/composite/dai/stream, member is   (o)ptional (r)equired (.)absent
    std::optional<bool> is_input;                                   // o.rr
    std::optional<std::array<uint8_t, kUniqueIdLength>> unique_id;  // oooo
    std::optional<std::string> manufacturer;                        // oooo
    std::optional<std::string> product;                             // oooo
    std::optional<uint32_t> clock_domain;                           // .rrr

    std::optional<fuchsia::hardware::audio::PlugDetectCapabilities>
        plug_detect_capabilities;       // r..r
    std::optional<bool> can_mute;       // ...o
    std::optional<bool> can_agc;        // ...o
    std::optional<float> min_gain_db;   // ...r
    std::optional<float> max_gain_db;   // ...r
    std::optional<float> gain_step_db;  // ...r
  };
  std::optional<BaseProperties>& properties() { return properties_; }
  const std::optional<BaseProperties>& properties() const { return properties_; }

 private:
  std::optional<BaseProperties> properties_;

  std::optional<component_testing::RealmRoot> realm_;
  fuchsia::component::BinderPtr audio_binder_;

  const DeviceEntry& device_entry_;

  fidl::InterfacePtr<fuchsia::hardware::audio::Codec> codec_;
  fidl::InterfacePtr<fuchsia::hardware::audio::Composite> composite_;
  fidl::InterfacePtr<fuchsia::hardware::audio::Dai> dai_;
  fidl::InterfacePtr<fuchsia::hardware::audio::StreamConfig> stream_config_;
  fidl::InterfacePtr<fuchsia::hardware::audio::Health> health_;

  zx::channel ProbeDevice(zx::channel channel, bool* is_busy);

  std::vector<fuchsia::hardware::audio::PcmSupportedFormats> ring_buffer_pcm_formats_;
  std::vector<fuchsia::hardware::audio::DaiSupportedFormats> dai_formats_;
};

// ostream formatting for DriverType
inline std::ostream& operator<<(std::ostream& out, const DriverType& dev_dir) {
  switch (dev_dir) {
    case DriverType::Codec:
      return (out << "Codec");
    case DriverType::Composite:
      return (out << "Composite");
    case DriverType::Dai:
      return (out << "Dai");
    case DriverType::StreamConfigInput:
      return (out << "StreamConfig(In)");
    case DriverType::StreamConfigOutput:
      return (out << "StreamConfig(Out)");
  }
}

// ostream formatting for DeviceType
inline std::ostream& operator<<(std::ostream& out, const DeviceType& device_type) {
  switch (device_type) {
    case DeviceType::A2DP:
      return (out << "A2DP");
    case DeviceType::BuiltIn:
      return (out << "Built-in");
    case DeviceType::Virtual:
      return (out << "VirtualAudio");
  }
}

// ostream formatting for optional<PlugDetectCapabilities>
inline std::ostream& operator<<(
    std::ostream& out,
    const std::optional<fuchsia::hardware::audio::PlugDetectCapabilities>& plug_caps) {
  if (!plug_caps.has_value()) {
    return (out << "NONE");
  }
  switch (*plug_caps) {
    case fuchsia::hardware::audio::PlugDetectCapabilities::CAN_ASYNC_NOTIFY:
      return (out << "CAN_ASYNC_NOTIFY");
    case fuchsia::hardware::audio::PlugDetectCapabilities::HARDWIRED:
      return (out << "HARDWIRED");
  }
}

// ostream formatting for DaiSampleFormat
inline std::ostream& operator<<(std::ostream& out,
                                fuchsia::hardware::audio::DaiSampleFormat sample_format) {
  switch (sample_format) {
    case fuchsia::hardware::audio::DaiSampleFormat::PDM:
      return (out << "PDM");
    case fuchsia::hardware::audio::DaiSampleFormat::PCM_SIGNED:
      return (out << "PCM_SIGNED");
    case fuchsia::hardware::audio::DaiSampleFormat::PCM_UNSIGNED:
      return (out << "PCM_UNSIGNED");
    case fuchsia::hardware::audio::DaiSampleFormat::PCM_FLOAT:
      return (out << "PCM_FLOAT");
  }
}

// ostream formatting for DaiFrameFormatStandard
inline std::ostream& operator<<(std::ostream& out,
                                fuchsia::hardware::audio::DaiFrameFormatStandard format) {
  switch (format) {
    case fuchsia::hardware::audio::DaiFrameFormatStandard::NONE:
      return (out << "PDM");
    case fuchsia::hardware::audio::DaiFrameFormatStandard::I2S:
      return (out << "I2S");
    case fuchsia::hardware::audio::DaiFrameFormatStandard::STEREO_LEFT:
      return (out << "STEREO_LEFT");
    case fuchsia::hardware::audio::DaiFrameFormatStandard::STEREO_RIGHT:
      return (out << "STEREO_RIGHT");
    case fuchsia::hardware::audio::DaiFrameFormatStandard::TDM1:
      return (out << "TDM1");
    case fuchsia::hardware::audio::DaiFrameFormatStandard::TDM2:
      return (out << "TDM2");
    case fuchsia::hardware::audio::DaiFrameFormatStandard::TDM3:
      return (out << "TDM3");
  }
}

// ostream formatting for DaiFrameFormatCustom
inline std::ostream& operator<<(std::ostream& out,
                                fuchsia::hardware::audio::DaiFrameFormatCustom format) {
  return (out << "[left_justified " << format.left_justified << ", sclk_on_raising "
              << format.sclk_on_raising << ", frame_sync_sclks_offset "
              << static_cast<int16_t>(format.frame_sync_sclks_offset) << ", frame_sync_size "
              << static_cast<uint16_t>(format.frame_sync_size) << "]");
}

// ostream formatting for DaiFrameFormat
inline std::ostream& operator<<(std::ostream& out,
                                fuchsia::hardware::audio::DaiFrameFormat format) {
  if (format.is_frame_format_standard()) {
    return (out << format.frame_format_standard());
  }
  if (format.is_frame_format_custom()) {
    return (out << format.frame_format_custom());
  }
  ADD_FAILURE() << "INVALID frame_format";
  return (out << "[invalid frame_format union: neither standard nor custom]");
}

// ostream formatting for optional<UniqueId>
inline std::ostream& operator<<(std::ostream& out, std::optional<std::array<uint8_t, 16>> id) {
  if (!id.has_value()) {
    return (out << "NONE");
  }
  char id_buf[(2 * kUniqueIdLength) + 1];
  std::snprintf(id_buf, sizeof(id_buf),
                "%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x%02x", (*id)[0],
                (*id)[1], (*id)[2], (*id)[3], (*id)[4], (*id)[5], (*id)[6], (*id)[7], (*id)[8],
                (*id)[9], (*id)[10], (*id)[11], (*id)[12], (*id)[13], (*id)[14], (*id)[15]);
  id_buf[2 * kUniqueIdLength] = 0;
  return (out << id_buf);
}

}  // namespace media::audio::drivers::test

#endif  // SRC_MEDIA_AUDIO_DRIVERS_TESTS_TEST_BASE_H_
