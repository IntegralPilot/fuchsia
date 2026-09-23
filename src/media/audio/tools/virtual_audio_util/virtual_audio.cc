// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include <fidl/fuchsia.virtualaudio/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>
#include <lib/fdio/directory.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/media/cpp/timeline_function.h>
#include <lib/media/cpp/timeline_rate.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/clock.h>
#include <poll.h>
#include <unistd.h>
#include <zircon/device/audio.h>
#include <zircon/status.h>
#include <zircon/syscalls/clock.h>

#include <cstddef>
#include <iterator>
#include <optional>

#include <fbl/algorithm.h>

#include "src/lib/fsl/tasks/fd_waiter.h"
#include "src/lib/fxl/command_line.h"
#include "src/lib/fxl/strings/string_number_conversions.h"

namespace virtual_audio {
namespace {

class VirtualAudioUtil;

class DeviceEventHandler : public fidl::AsyncEventHandler<fuchsia_virtualaudio::Device> {
 public:
  void OnSetFormat(fidl::Event<fuchsia_virtualaudio::Device::OnSetFormat>& event) override;
  void OnBufferCreated(fidl::Event<fuchsia_virtualaudio::Device::OnBufferCreated>& event) override;
  void OnStart(fidl::Event<fuchsia_virtualaudio::Device::OnStart>& event) override;
  void OnStop(fidl::Event<fuchsia_virtualaudio::Device::OnStop>& event) override;
  void OnPositionNotify(
      fidl::Event<fuchsia_virtualaudio::Device::OnPositionNotify>& event) override;
  void on_fidl_error(fidl::UnbindInfo info) override;
};

class VirtualAudioUtil {
  friend class DeviceEventHandler;

 public:
  explicit VirtualAudioUtil(async::Loop* loop) { VirtualAudioUtil::loop_ = loop; }

  void Run(fxl::CommandLine* cmdline);

 private:
  enum class Command : uint8_t {
    GET_NUM_VIRTUAL_DEVICES,

    SET_DEVICE_NAME,
    SET_MANUFACTURER,
    SET_PRODUCT_NAME,

    ADD_FORMAT_RANGE,
    CLEAR_FORMAT_RANGES,
    SET_CLOCK_DOMAIN,
    SET_INITIAL_CLOCK_RATE,
    SET_TRANSFER_BYTES,
    SET_INTERNAL_DELAY,
    SET_EXTERNAL_DELAY,
    SET_RING_BUFFER_RESTRICTIONS,
    RESET_CONFIG,

    ADD_DEVICE,
    REMOVE_DEVICE,
    GET_FORMAT,
    RETRIEVE_BUFFER,
    WRITE_BUFFER,
    GET_POSITION,
    SET_NOTIFICATION_FREQUENCY,
    ADJUST_CLOCK_RATE,

    SET_COMPOSITE,
    WAIT,
    HELP,
    INVALID,
  };

  static constexpr char kNumDevsSwitch[] = "num-devs";

  static constexpr char kDeviceNameSwitch[] = "dev";
  static constexpr char kManufacturerSwitch[] = "mfg";
  static constexpr char kProductNameSwitch[] = "prod";

  static constexpr char kAddFormatRangeSwitch[] = "add-format";
  static constexpr char kClearFormatRangesSwitch[] = "clear-format";
  static constexpr char kClockDomainSwitch[] = "domain";
  static constexpr char kInitialRateSwitch[] = "initial-rate";
  static constexpr char kTransferBytesSwitch[] = "transfer";
  static constexpr char kInternalDelaySwitch[] = "int-delay";
  static constexpr char kExternalDelaySwitch[] = "ext-delay";
  static constexpr char kBufferRestrictionsSwitch[] = "rb";
  static constexpr char kResetConfigSwitch[] = "reset";

  static constexpr char kAddDeviceSwitch[] = "add";
  static constexpr char kRemoveDeviceSwitch[] = "remove";

  static constexpr char kGetFormatSwitch[] = "get-format";
  static constexpr char kRetrieveBufferSwitch[] = "get-rb";
  static constexpr char kWriteBufferSwitch[] = "write-rb";
  static constexpr char kGetPositionSwitch[] = "get-pos";
  static constexpr char kNotificationFrequencySwitch[] = "notifs";
  static constexpr char kClockRateSwitch[] = "rate";

  static constexpr char kCompositeSwitch[] = "composite";
  static constexpr char kWaitSwitch[] = "wait";
  static constexpr char kHelp1Switch[] = "help";
  static constexpr char kHelp2Switch[] = "?";

  static constexpr char kDefaultDeviceName[] = "Vertex";
  static constexpr char kDefaultManufacturer[] = "Puerile Virtual Functions, Incorporated";
  static constexpr char kDefaultProductName[] = "Virgil, version 1.0";

  static constexpr int32_t kDefaultClockDomain = 0;
  static constexpr int32_t kDefaultInitialClockRatePpm = 0;

  static constexpr uint8_t kDefaultFormatRangeOption = 0;

  static constexpr uint32_t kDefaultTransferBytes = 0x100;
  static constexpr int64_t kDefaultInternalDelayNsec = zx::msec(0).get();
  static constexpr int64_t kDefaultExternalDelayNsec = zx::msec(1).get();
  static constexpr uint8_t kDefaultRingBufferOption = 0;

  // This repeated value can be interpreted various ways, at various sample_sizes and num_chans.
  static constexpr uint64_t kDefaultValueToWrite = 0x22446688AACCEE00;

  static constexpr uint32_t kDefaultNotificationFrequency = 4;

  static constexpr struct {
    const char* name;
    Command cmd;
  } COMMANDS[] = {
      {.name = kNumDevsSwitch, .cmd = Command::GET_NUM_VIRTUAL_DEVICES},

      {.name = kDeviceNameSwitch, .cmd = Command::SET_DEVICE_NAME},
      {.name = kManufacturerSwitch, .cmd = Command::SET_MANUFACTURER},
      {.name = kProductNameSwitch, .cmd = Command::SET_PRODUCT_NAME},

      {.name = kAddFormatRangeSwitch, .cmd = Command::ADD_FORMAT_RANGE},
      {.name = kClearFormatRangesSwitch, .cmd = Command::CLEAR_FORMAT_RANGES},
      {.name = kClockDomainSwitch, .cmd = Command::SET_CLOCK_DOMAIN},
      {.name = kInitialRateSwitch, .cmd = Command::SET_INITIAL_CLOCK_RATE},
      {.name = kTransferBytesSwitch, .cmd = Command::SET_TRANSFER_BYTES},
      {.name = kInternalDelaySwitch, .cmd = Command::SET_INTERNAL_DELAY},
      {.name = kExternalDelaySwitch, .cmd = Command::SET_EXTERNAL_DELAY},
      {.name = kBufferRestrictionsSwitch, .cmd = Command::SET_RING_BUFFER_RESTRICTIONS},
      {.name = kResetConfigSwitch, .cmd = Command::RESET_CONFIG},

      {.name = kAddDeviceSwitch, .cmd = Command::ADD_DEVICE},
      {.name = kRemoveDeviceSwitch, .cmd = Command::REMOVE_DEVICE},

      {.name = kGetFormatSwitch, .cmd = Command::GET_FORMAT},
      {.name = kRetrieveBufferSwitch, .cmd = Command::RETRIEVE_BUFFER},
      {.name = kWriteBufferSwitch, .cmd = Command::WRITE_BUFFER},
      {.name = kGetPositionSwitch, .cmd = Command::GET_POSITION},
      {.name = kNotificationFrequencySwitch, .cmd = Command::SET_NOTIFICATION_FREQUENCY},
      {.name = kClockRateSwitch, .cmd = Command::ADJUST_CLOCK_RATE},

      {.name = kCompositeSwitch, .cmd = Command::SET_COMPOSITE},
      {.name = kWaitSwitch, .cmd = Command::WAIT},
      {.name = kHelp1Switch, .cmd = Command::HELP},
      {.name = kHelp2Switch, .cmd = Command::HELP},
  };

  static async::Loop* loop_;
  static bool received_callback_;

  static void QuitLoop();
  static bool RunForDuration(zx::duration duration);
  static bool WaitForNoCallback();
  static bool WaitForCallback();

  void RegisterKeyWaiter();
  bool WaitForKey();

  bool ConnectToControllers();
  bool ConnectToDevice();

  void ParseAndExecute(fxl::CommandLine* cmdline);
  bool ExecuteCommand(Command cmd, const std::string& value);
  static void Usage();

  // Methods using the FIDL Service interface
  bool GetNumDevices();
  bool AddDevice();

  // Methods using the FIDL Configuration interface
  bool SetDeviceName(const std::string& name);
  bool SetManufacturer(const std::string& name);
  bool SetProductName(const std::string& name);
  bool AddFormatRange(const std::string& format_range_str);
  bool ClearFormatRanges();
  bool SetClockDomain(const std::string& clock_domain_str);
  bool SetInitialClockRate(const std::string& initial_clock_rate_str);
  bool SetTransferBytes(const std::string& transfer_bytes_str);
  bool SetInternalDelay(const std::string& delay_str);
  bool SetExternalDelay(const std::string& delay_str);
  bool SetRingBufferRestrictions(const std::string& rb_restr_str);
  zx_status_t ResetConfiguration();

  // Methods using the FIDL Device interface
  bool RemoveDevice();
  bool GetFormat();
  bool GetBuffer();
  bool WriteBuffer(const std::string& write_value_str);
  bool GetPosition();
  bool SetNotificationFrequency(const std::string& override_notifs_str);
  bool AdjustClockRate(const std::string& clock_adjust_str);
  bool SetDirection(std::optional<bool> is_input);

  fidl::Client<fuchsia_virtualaudio::Control>& controller() { return controller_; }

  std::unique_ptr<sys::ComponentContext> component_context_;
  fsl::FDWaiter keystroke_waiter_;
  bool key_quit_ = false;

  fidl::Client<fuchsia_virtualaudio::Control> controller_;

  fidl::Client<fuchsia_virtualaudio::Device> composite_;
  DeviceEventHandler event_handler_;
  fuchsia_virtualaudio::Configuration composite_config_;

  static zx::vmo ring_buffer_vmo_;

  static uint32_t BytesPerSample(uint32_t format);
  static void UpdateRunningPosition(uint32_t ring_position);

  static size_t rb_size_;
  static uint32_t last_rb_position_;
  static uint64_t running_position_;

 public:
  static uint32_t frame_size_;
  static media::TimelineRate ref_time_to_running_position_rate_;
  static media::TimelineFunction ref_time_to_running_position_;

 private:
  static void CallbackReceived();
  static void FormatNotification(uint32_t fps, uint32_t fmt, uint32_t chans, zx_duration_t delay);

  static void BufferNotification(zx::vmo ring_buffer_vmo, uint32_t num_ring_buffer_frames,
                                 uint32_t notifications_per_ring);

  static void StartNotification(zx_time_t start_time);
  static void StopNotification(zx_time_t stop_time, uint32_t ring_position);

  static void PositionNotification(zx_time_t monotonic_time_for_position, uint32_t ring_position);
};

void DeviceEventHandler::OnSetFormat(
    fidl::Event<fuchsia_virtualaudio::Device::OnSetFormat>& event) {
  VirtualAudioUtil::FormatNotification(event.frames_per_second(), event.sample_format(),
                                       event.num_channels(), event.external_delay());
}
void DeviceEventHandler::OnBufferCreated(
    fidl::Event<fuchsia_virtualaudio::Device::OnBufferCreated>& event) {
  VirtualAudioUtil::BufferNotification(std::move(event.ring_buffer()),
                                       event.num_ring_buffer_frames(),
                                       event.notifications_per_ring());
}
void DeviceEventHandler::OnStart(fidl::Event<fuchsia_virtualaudio::Device::OnStart>& event) {
  VirtualAudioUtil::StartNotification(event.start_time());
}
void DeviceEventHandler::OnStop(fidl::Event<fuchsia_virtualaudio::Device::OnStop>& event) {
  VirtualAudioUtil::StopNotification(event.stop_time(), event.ring_position());
}
void DeviceEventHandler::OnPositionNotify(
    fidl::Event<fuchsia_virtualaudio::Device::OnPositionNotify>& event) {
  VirtualAudioUtil::PositionNotification(event.monotonic_time(), event.ring_position());
}
void DeviceEventHandler::on_fidl_error(fidl::UnbindInfo info) {
  printf("device disconnected: %s\n", info.FormatDescription().c_str());
  VirtualAudioUtil::loop_->Quit();
}

::async::Loop* VirtualAudioUtil::loop_;
bool VirtualAudioUtil::received_callback_;
zx::vmo VirtualAudioUtil::ring_buffer_vmo_;

size_t VirtualAudioUtil::rb_size_;
uint32_t VirtualAudioUtil::last_rb_position_;
uint64_t VirtualAudioUtil::running_position_;
uint32_t VirtualAudioUtil::frame_size_;
media::TimelineRate VirtualAudioUtil::ref_time_to_running_position_rate_;
media::TimelineFunction VirtualAudioUtil::ref_time_to_running_position_;

uint32_t VirtualAudioUtil::BytesPerSample(uint32_t format_bitfield) {
  if (format_bitfield & (AUDIO_SAMPLE_FORMAT_20BIT_IN32 | AUDIO_SAMPLE_FORMAT_24BIT_IN32 |
                         AUDIO_SAMPLE_FORMAT_32BIT | AUDIO_SAMPLE_FORMAT_32BIT_FLOAT)) {
    return 4;
  }
  if (format_bitfield & AUDIO_SAMPLE_FORMAT_24BIT_PACKED) {
    return 3;
  }
  if (format_bitfield & AUDIO_SAMPLE_FORMAT_16BIT) {
    return 2;
  }
  if (format_bitfield & AUDIO_SAMPLE_FORMAT_8BIT) {
    return 1;
  }

  printf("\n--Unknown format, could not determine bytes per sample. Exiting.\n");

  return 0;
}

// VirtualAudioUtil implementation
//
void VirtualAudioUtil::Run(fxl::CommandLine* cmdline) {
  ParseAndExecute(cmdline);

  // If any lingering callbacks were queued, let them drain.
  if (!WaitForNoCallback()) {
    printf("Received unexpected callback!\n");
  }
}

void VirtualAudioUtil::QuitLoop() {
  async::PostTask(loop_->dispatcher(), [loop = loop_]() { loop->Quit(); });
}

// Below was borrowed from gtest, as-is
bool VirtualAudioUtil::RunForDuration(zx::duration duration) {
  auto canceled = std::make_shared<bool>(false);
  bool timed_out = false;
  async::PostDelayedTask(
      loop_->dispatcher(),
      [loop = loop_, canceled, &timed_out] {
        if (*canceled) {
          return;
        }
        timed_out = true;
        loop->Quit();
      },
      duration);
  loop_->Run();
  loop_->ResetQuit();

  if (!timed_out) {
    *canceled = true;
  }
  return timed_out;
}
// Above was borrowed from gtest, as-is

bool VirtualAudioUtil::WaitForNoCallback() {
  received_callback_ = false;
  bool timed_out = RunForDuration(zx::msec(5));

  // If all is well, we DIDN'T get a disconnect callback and are still bound.
  if (received_callback_) {
    printf("  ... received unexpected callback\n");
  }
  return (timed_out && !received_callback_);
}

bool VirtualAudioUtil::WaitForCallback() {
  received_callback_ = false;
  bool timed_out = RunForDuration(zx::msec(2000));

  if (!received_callback_) {
    printf("  ... expected a callback; none was received\n");
  }
  return (!timed_out && received_callback_);
}

void VirtualAudioUtil::RegisterKeyWaiter() {
  keystroke_waiter_.Wait(
      [this](zx_status_t, uint32_t) {
        int c = std::tolower(getc(stdin));
        if (c == 'q') {
          key_quit_ = true;
        }
        QuitLoop();
      },
      STDIN_FILENO, POLLIN);
}

bool VirtualAudioUtil::WaitForKey() {
  printf("\tPress Q to cancel, or any other key to continue...\n");
  setvbuf(stdin, nullptr, _IONBF, 0);  // Turn off buffering; immediately receive keypresses.
  RegisterKeyWaiter();

  while (RunForDuration(zx::sec(1))) {
  }

  return !key_quit_;
}

bool VirtualAudioUtil::ConnectToControllers() {
  const std::string kControlNodePath =
      std::string{"/dev/"} + fuchsia_virtualaudio::kControlNodeName;
  auto endpoints = fidl::CreateEndpoints<fuchsia_virtualaudio::Control>();
  if (endpoints.is_error()) {
    printf("ERROR: CreateEndpoints failed\n");
    return false;
  }
  zx_status_t status =
      fdio_service_connect(kControlNodePath.c_str(), endpoints->server.TakeChannel().release());
  if (status != ZX_OK) {
    printf("ERROR: failed to connect to '%s', status = %d\n", kControlNodePath.c_str(), status);
    return false;
  }

  controller_.Bind(std::move(endpoints->client), loop_->dispatcher());
  // let VirtualAudio disconnect if all is not well.
  bool success = (WaitForNoCallback() && controller_.is_valid());
  if (!success) {
    printf("Failed to establish channel to async controller\n");
    return false;
  }

  return true;
}

void VirtualAudioUtil::ParseAndExecute(fxl::CommandLine* cmdline) {
  if (!cmdline->has_argv0() || cmdline->options().empty()) {
    printf("No commands provided; no action taken\n");
    return;
  }

  // Looks like we will interact with the service; get ready to connect to it.
  component_context_ = sys::ComponentContext::CreateAndServeOutgoingDirectory();

  if (!ConnectToControllers()) {
    return;
  }

  if (ResetConfiguration() != ZX_OK) {
    QuitLoop();
    return;
  }

  for (const auto& option : cmdline->options()) {
    bool success = false;
    Command cmd = Command::INVALID;

    for (const auto& entry : COMMANDS) {
      if (option.name == entry.name) {
        cmd = entry.cmd;
        success = true;

        break;
      }
    }

    if (!success) {
      printf("Failed to parse command ID `--%s'\n", option.name.c_str());
      Usage();
      return;
    }

    printf("Executing `--%s' command...\n", option.name.c_str());
    success = ExecuteCommand(cmd, option.value);
    if (!success) {
      printf("  ... `--%s' command was unsuccessful\n", option.name.c_str());
      return;
    }
  }  // while (cmdline args) without default
}

bool VirtualAudioUtil::ExecuteCommand(Command cmd, const std::string& value) {
  bool success;
  switch (cmd) {
    // FIDL Service methods
    case Command::GET_NUM_VIRTUAL_DEVICES:
      success = GetNumDevices();
      break;

    // FIDL Configuration/Device methods
    case Command::SET_DEVICE_NAME:
      success = SetDeviceName(value);
      break;
    case Command::SET_MANUFACTURER:
      success = SetManufacturer(value);
      break;
    case Command::SET_PRODUCT_NAME:
      success = SetProductName(value);
      break;

    case Command::SET_CLOCK_DOMAIN:
      success = SetClockDomain(value);
      break;
    case Command::SET_INITIAL_CLOCK_RATE:
      success = SetInitialClockRate(value);
      break;
    case Command::ADD_FORMAT_RANGE:
      success = AddFormatRange(value);
      break;
    case Command::CLEAR_FORMAT_RANGES:
      success = ClearFormatRanges();
      break;
    case Command::SET_TRANSFER_BYTES:
      success = SetTransferBytes(value);
      break;
    case Command::SET_INTERNAL_DELAY:
      success = SetInternalDelay(value);
      break;
    case Command::SET_EXTERNAL_DELAY:
      success = SetExternalDelay(value);
      break;
    case Command::SET_RING_BUFFER_RESTRICTIONS:
      success = SetRingBufferRestrictions(value);
      break;
    case Command::RESET_CONFIG:
      success = (ResetConfiguration() == ZX_OK);
      break;

    case Command::ADD_DEVICE:
      success = AddDevice();
      break;
    case Command::REMOVE_DEVICE:
      success = RemoveDevice();
      break;

    case Command::GET_FORMAT:
      success = GetFormat();
      break;
    case Command::RETRIEVE_BUFFER:
      success = GetBuffer();
      break;
    case Command::WRITE_BUFFER:
      success = WriteBuffer(value);
      break;
    case Command::GET_POSITION:
      success = GetPosition();
      break;
    case Command::SET_NOTIFICATION_FREQUENCY:
      success = SetNotificationFrequency(value);
      break;
    case Command::ADJUST_CLOCK_RATE:
      success = AdjustClockRate(value);
      break;

    case Command::SET_COMPOSITE:
      success = true;
      break;
    case Command::WAIT:
      success = WaitForKey();
      break;
    case Command::HELP:
      Usage();
      success = true;
      break;
    case Command::INVALID:
      success = false;
      break;

      // Intentionally omitting default, so new enums are not forgotten here.
  }
  return success;
}

void VirtualAudioUtil::Usage() {
  printf("\nUsage: virtual_audio [options]\n");
  printf("Interactively configure and control virtual audio devices.\n");

  printf("\nValid options:\n");

  printf("\n  The following commands customize a device configuration, before it is added\n");
  printf("  --%s[=<DEVICE_NAME>]\t  Set the device name (default '%s')\n", kDeviceNameSwitch,
         kDefaultDeviceName);
  printf("  --%s[=<MANUFACTURER>]  Set the manufacturer name (default '%s')\n", kManufacturerSwitch,
         kDefaultManufacturer);
  printf("  --%s[=<PRODUCT>]\t  Set the product name (default '%s')\n", kProductNameSwitch,
         kDefaultProductName);

  printf("  --%s[=<NUM>]\t  Add format range [0,6] (default 8-44.1 Mono/Stereo 24-32)\n",
         kAddFormatRangeSwitch);
  printf("  --%s\t  Clear any format ranges (including the built-in default)\n",
         kClearFormatRangesSwitch);
  printf("  --%s[=<NUM>]\t  Set device clock domain (default %d)\n", kClockDomainSwitch,
         kDefaultClockDomain);
  printf("  --%s[=<NUM>]  Set initial device clock rate in PPM [-1000, 1000] (default %d)\n",
         kInitialRateSwitch, kDefaultInitialClockRatePpm);
  printf("  --%s[=<BYTES>]\t  Set the transfer bytes, in bytes (default %u)\n",
         kTransferBytesSwitch, kDefaultTransferBytes);

  printf("  --%s[=<NSEC>]\t  Set internal delay (default %zd ns)\n", kInternalDelaySwitch,
         kDefaultInternalDelayNsec);
  printf("  --%s[=<NSEC>]\t  Set external delay (default %zd ns)\n", kExternalDelaySwitch,
         kDefaultExternalDelayNsec);
  printf("  --%s[=<NUM>]\t\t  Set ring-buffer restrictions [0,2] (default 48k-72k frames mod 6k)\n",
         kBufferRestrictionsSwitch);
  printf("  --%s\t\t  Clear any customizations; return this configuration to the default\n",
         kResetConfigSwitch);

  printf("\n  --%s\t\t\t  Activate the current configuration (AddDevice)\n", kAddDeviceSwitch);

  printf("\n  Subsequent commands require an activated (added) virtual audio device\n");
  printf("  --%s\t\t  Retrieve the client-selected ring-buffer format\n", kGetFormatSwitch);
  printf("  --%s\t\t  Return a mapping of the ring buffer\n", kRetrieveBufferSwitch);
  printf(
      "  --%s[=<UINT64>]\t  Fill the ring-buffer with this uint64 (in hex, default "
      "0x%zX)\n",
      kWriteBufferSwitch, kDefaultValueToWrite);
  printf("  --%s\t\t  Retrieve the current ring-buffer position and corresponding ref time\n",
         kGetPositionSwitch);
  printf("  --%s[=<FREQ>]\t  Set an alternate notifications-per-ring frequency (default %u).\n",
         kNotificationFrequencySwitch, kDefaultNotificationFrequency);
  printf("\t\t\t  (Don't receive the same position notifications sent to the client)\n");
  printf("  --%s=<DELTA PPM>\t  Adjust the rate of the device clock, in parts-per-million\n",
         kClockRateSwitch);
  printf("\t\t\t  This is reflected in position notification delivery timing and timestamps.\n");

  printf("\n  --%s\t\t  Deactivate the current device configuration (RemoveDevice)\n",
         kRemoveDeviceSwitch);

  printf("\n  The following commands are on the virtualaudio::Control protocol:\n");
  printf("  --%s\t\t  Retrieve the number of currently active virtual audio devices\n",
         kNumDevsSwitch);

  printf("\n  --%s\t\t  Wait for a key press before executing subsequent commands\n", kWaitSwitch);
  printf("  --%s, --%s\t\t  Show this message\n", kHelp1Switch, kHelp2Switch);
  printf("\n");
}

bool VirtualAudioUtil::GetNumDevices() {
  bool success = false;
  controller_->GetNumDevices().Then(
      [&](fidl::Result<fuchsia_virtualaudio::Control::GetNumDevices>& result) {
        if (result.is_ok()) {
          printf("--Received NumDevices (%u inputs, %u outputs, %u unspecified direction)\n",
                 result->num_input_devices(), result->num_output_devices(),
                 result->num_unspecified_direction_devices());
          success = true;
        } else {
          printf("ERROR: GetNumDevices failed: %s\n",
                 result.error_value().FormatDescription().c_str());
        }
        CallbackReceived();
      });

  return WaitForCallback() && success;
}

bool VirtualAudioUtil::SetDeviceName(const std::string& name) {
  composite_config_.device_name() = name;
  return true;
}

bool VirtualAudioUtil::SetManufacturer(const std::string& name) {
  composite_config_.manufacturer_name() = name;
  return true;
}

bool VirtualAudioUtil::SetProductName(const std::string& name) {
  composite_config_.product_name() = name;
  return true;
}

bool VirtualAudioUtil::SetClockDomain(const std::string& clock_domain_str) {
  int32_t clock_domain =
      (clock_domain_str.empty() ? kDefaultClockDomain
                                : fxl::StringToNumber<int32_t>(clock_domain_str));

  auto composite = composite_config_.device_specific()->composite();
  composite->clock_properties()->domain() = clock_domain;

  if (clock_domain == 0 && composite->clock_properties()->rate_adjustment_ppm().has_value() &&
      composite->clock_properties()->rate_adjustment_ppm().value() != 0) {
    printf("WARNING: by definition, a clock in domain 0 should never have rate variance!\n");
  }

  return true;
}

bool VirtualAudioUtil::SetInitialClockRate(const std::string& initial_clock_rate_str) {
  int32_t clock_adjustment_ppm =
      (initial_clock_rate_str.empty() ? kDefaultInitialClockRatePpm
                                      : fxl::StringToNumber<int32_t>(initial_clock_rate_str));

  auto composite = composite_config_.device_specific()->composite();
  auto props = composite->clock_properties();
  props->rate_adjustment_ppm() = clock_adjustment_ppm;

  if (clock_adjustment_ppm < ZX_CLOCK_UPDATE_MIN_RATE_ADJUST ||
      clock_adjustment_ppm > ZX_CLOCK_UPDATE_MAX_RATE_ADJUST) {
    printf("ERROR: Clock rate adjustment must be within [%d, %d].\n",
           ZX_CLOCK_UPDATE_MIN_RATE_ADJUST, ZX_CLOCK_UPDATE_MAX_RATE_ADJUST);
    return false;
  }
  if ((props->domain().has_value() && props->domain().value() == 0) && clock_adjustment_ppm != 0) {
    printf("WARNING: by definition, a clock in domain 0 should never have rate variance!\n");
  }

  return true;
}

struct Format {
  uint32_t flags;
  uint32_t min_rate;
  uint32_t max_rate;
  uint8_t min_chans;
  uint8_t max_chans;
  uint16_t rate_family_flags;
};

// These formats exercise various scenarios:
// 0: full range of rates in both families (but not 48k), both 1-2 chans
// 1: float-only, 48k family extends to 96k, 2 or 4 chan
// 2: fixed 48k 2-chan 16b
// 3: 16k 2-chan 16b
// 4: 96k and 48k, 2-chan 16b
// 5: 3-chan device at 48k 16b
// 6: 1-chan device at 8k 16b
// 7: 1-chan device at 48k 16b
// 8: 2-chan device at 96k 16b
//
// Going forward, it would be best to have chans, rate and bitdepth specifiable individually.
constexpr Format kFormatSpecs[9] = {
    {
        .flags = AUDIO_SAMPLE_FORMAT_16BIT | AUDIO_SAMPLE_FORMAT_24BIT_IN32,
        .min_rate = 8000,
        .max_rate = 44100,
        .min_chans = 1,
        .max_chans = 2,
        .rate_family_flags = ASF_RANGE_FLAG_FPS_44100_FAMILY | ASF_RANGE_FLAG_FPS_48000_FAMILY,
    },
    {
        .flags = AUDIO_SAMPLE_FORMAT_32BIT_FLOAT,
        .min_rate = 32000,
        .max_rate = 96000,
        .min_chans = 2,
        .max_chans = 4,
        .rate_family_flags = ASF_RANGE_FLAG_FPS_48000_FAMILY,
    },
    {
        .flags = AUDIO_SAMPLE_FORMAT_16BIT,
        .min_rate = 48000,
        .max_rate = 48000,
        .min_chans = 2,
        .max_chans = 2,
        .rate_family_flags = ASF_RANGE_FLAG_FPS_CONTINUOUS,
    },
    {
        .flags = AUDIO_SAMPLE_FORMAT_16BIT,
        .min_rate = 16000,
        .max_rate = 16000,
        .min_chans = 2,
        .max_chans = 2,
        .rate_family_flags = ASF_RANGE_FLAG_FPS_48000_FAMILY,
    },
    {
        .flags = AUDIO_SAMPLE_FORMAT_16BIT,
        .min_rate = 48000,
        .max_rate = 96000,
        .min_chans = 2,
        .max_chans = 2,
        .rate_family_flags = ASF_RANGE_FLAG_FPS_48000_FAMILY,
    },
    {
        .flags = AUDIO_SAMPLE_FORMAT_16BIT,
        .min_rate = 48000,
        .max_rate = 48000,
        .min_chans = 3,
        .max_chans = 3,
        .rate_family_flags = ASF_RANGE_FLAG_FPS_48000_FAMILY,
    },
    {
        .flags = AUDIO_SAMPLE_FORMAT_16BIT,
        .min_rate = 8000,
        .max_rate = 8000,
        .min_chans = 1,
        .max_chans = 1,
        .rate_family_flags = ASF_RANGE_FLAG_FPS_CONTINUOUS,
    },
    {
        .flags = AUDIO_SAMPLE_FORMAT_16BIT,
        .min_rate = 48000,
        .max_rate = 48000,
        .min_chans = 1,
        .max_chans = 1,
        .rate_family_flags = ASF_RANGE_FLAG_FPS_48000_FAMILY,
    },
    {
        .flags = AUDIO_SAMPLE_FORMAT_16BIT,
        .min_rate = 96000,
        .max_rate = 96000,
        .min_chans = 2,
        .max_chans = 2,
        .rate_family_flags = ASF_RANGE_FLAG_FPS_CONTINUOUS,
    },
};

bool VirtualAudioUtil::AddFormatRange(const std::string& format_range_str) {
  uint8_t format_option =
      (format_range_str.empty() ? kDefaultFormatRangeOption
                                : fxl::StringToNumber<uint8_t>(format_range_str));
  if (format_option >= std::size(kFormatSpecs)) {
    printf("ERROR: Format range option must be %lu or less.\n", std::size(kFormatSpecs) - 1);
    return false;
  }
  fuchsia_virtualaudio::FormatRange range;
  range.sample_format_flags() = kFormatSpecs[format_option].flags;
  range.min_frame_rate() = kFormatSpecs[format_option].min_rate;
  range.max_frame_rate() = kFormatSpecs[format_option].max_rate;
  range.min_channels() = kFormatSpecs[format_option].min_chans;
  range.max_channels() = kFormatSpecs[format_option].max_chans;
  range.rate_family_flags() = kFormatSpecs[format_option].rate_family_flags;

  auto composite = composite_config_.device_specific()->composite();
  // Set formats for all ring buffers.
  for (auto& i : *composite->ring_buffers()) {
    i.ring_buffer()->supported_formats()->emplace_back(std::move(range));
  }
  return true;
}

bool VirtualAudioUtil::ClearFormatRanges() {
  auto composite = composite_config_.device_specific()->composite();
  // Clear format ranges for all ring buffers.
  for (auto& i : *composite->ring_buffers()) {
    i.ring_buffer()->supported_formats()->clear();
  }
  return true;
}

bool VirtualAudioUtil::SetTransferBytes(const std::string& transfer_bytes_str) {
  uint32_t driver_transfer_bytes = transfer_bytes_str.empty()
                                       ? kDefaultTransferBytes
                                       : fxl::StringToNumber<uint32_t>(transfer_bytes_str);

  auto composite = composite_config_.device_specific()->composite();
  // Set driver transfer bytes for all ring buffers.
  for (auto& i : *composite->ring_buffers()) {
    i.ring_buffer()->driver_transfer_bytes() = driver_transfer_bytes;
  }
  return true;
}

bool VirtualAudioUtil::SetInternalDelay(const std::string& delay_str) {
  zx_duration_t internal_delay =
      delay_str.empty() ? kDefaultInternalDelayNsec : fxl::StringToNumber<zx_duration_t>(delay_str);

  auto composite = composite_config_.device_specific()->composite();
  // For now, set internal delay for all ring buffers.
  for (auto& i : *composite->ring_buffers()) {
    i.ring_buffer()->internal_delay() = internal_delay;
  }
  return true;
}

bool VirtualAudioUtil::SetExternalDelay(const std::string& delay_str) {
  zx_duration_t external_delay =
      delay_str.empty() ? kDefaultExternalDelayNsec : fxl::StringToNumber<zx_duration_t>(delay_str);

  auto composite = composite_config_.device_specific()->composite();
  // Set external delay for all ring buffers.
  for (auto& i : *composite->ring_buffers()) {
    i.ring_buffer()->external_delay() = external_delay;
  }
  return true;
}

struct BufferSpec {
  uint32_t min_frames;
  uint32_t max_frames;
  uint32_t mod_frames;
};

// Buffer sizes (at default 48kHz rate): [0] 1.0-1.5 sec, in steps of 0.125;
// [1] 0.2-0.6 sec, in steps of 0.01;    [2] exactly 2 secs;    [3] exactly 6 secs.
constexpr BufferSpec kBufferSpecs[4] = {
    {.min_frames = 48000, .max_frames = 72000, .mod_frames = 6000},
    {.min_frames = 9600, .max_frames = 28800, .mod_frames = 480},
    {.min_frames = 96000, .max_frames = 96000, .mod_frames = 96000},
    {.min_frames = 288000, .max_frames = 288000, .mod_frames = 288000},
};

bool VirtualAudioUtil::SetRingBufferRestrictions(const std::string& rb_restr_str) {
  uint8_t rb_option = (rb_restr_str.empty() ? kDefaultRingBufferOption
                                            : fxl::StringToNumber<uint8_t>(rb_restr_str));
  if (rb_option >= std::size(kBufferSpecs)) {
    printf("ERROR: Ring buffer option must be %lu or less.\n", std::size(kBufferSpecs) - 1);
    return false;
  }

  fuchsia_virtualaudio::RingBufferConstraints ring_buffer_constraints;
  ring_buffer_constraints.min_frames() = kBufferSpecs[rb_option].min_frames;
  ring_buffer_constraints.max_frames() = kBufferSpecs[rb_option].max_frames;
  ring_buffer_constraints.modulo_frames() = kBufferSpecs[rb_option].mod_frames;

  auto composite = composite_config_.device_specific()->composite();
  // Set ring buffer constraints for all ring buffers.
  for (auto& i : *composite->ring_buffers()) {
    i.ring_buffer()->ring_buffer_constraints() = ring_buffer_constraints;
  }
  return true;
}

bool VirtualAudioUtil::AdjustClockRate(const std::string& clock_adjust_str) {
  int32_t clock_domain = 0;

  auto rate_adjustment_ppm = fxl::StringToNumber<int32_t>(clock_adjust_str);
  if (rate_adjustment_ppm < ZX_CLOCK_UPDATE_MIN_RATE_ADJUST ||
      rate_adjustment_ppm > ZX_CLOCK_UPDATE_MAX_RATE_ADJUST) {
    printf("ERROR: Clock rate adjustment must be within [%d, %d].\n",
           ZX_CLOCK_UPDATE_MIN_RATE_ADJUST, ZX_CLOCK_UPDATE_MAX_RATE_ADJUST);
    return false;
  }

  auto composite = composite_config_.device_specific()->composite();
  if (composite->clock_properties().has_value() &&
      composite->clock_properties()->domain().has_value()) {
    clock_domain = composite->clock_properties()->domain().value();
  }

  if (clock_domain == 0 && rate_adjustment_ppm != 0) {
    printf("WARNING: by definition, a clock in domain 0 should never have rate variance!\n");
  }
  composite_->AdjustClockRate({rate_adjustment_ppm})
      .Then([](fidl::Result<fuchsia_virtualaudio::Device::AdjustClockRate>& result) {
        CallbackReceived();
      });
  return WaitForCallback();
}

zx_status_t VirtualAudioUtil::ResetConfiguration() {
  bool success = false;
  zx_status_t status = ZX_OK;

  controller()
      ->GetDefaultConfiguration(
          {fuchsia_virtualaudio::DeviceType::kComposite, fuchsia_virtualaudio::Direction()})
      .Then([&](fidl::Result<fuchsia_virtualaudio::Control::GetDefaultConfiguration>& result) {
        if (result.is_error()) {
          printf("ERROR: GetDefaultConfiguration failed: %s\n",
                 result.error_value().FormatDescription().c_str());
          status = ZX_ERR_INTERNAL;
        } else {
          composite_config_ = std::move(result.value().config());

          auto composite = composite_config_.device_specific()->composite();
          if (!composite->ring_buffers().has_value()) {
            composite->ring_buffers().emplace(1);
          }
          for (auto& i : *composite->ring_buffers()) {
            if (!i.ring_buffer().has_value()) {
              i.ring_buffer().emplace();
            }
          }

          success = true;
        }
        CallbackReceived();
      });

  if (!WaitForCallback() || !success) {
    return status != ZX_OK ? status : ZX_ERR_INTERNAL;
  }
  return ZX_OK;
}

bool VirtualAudioUtil::AddDevice() {
  auto endpoints = fidl::CreateEndpoints<fuchsia_virtualaudio::Device>();
  if (endpoints.is_error()) {
    printf("ERROR: CreateEndpoints failed\n");
    return false;
  }

  bool success = false;
  zx_status_t status = ZX_OK;

  controller()
      ->AddDevice({std::move(composite_config_), std::move(endpoints->server)})
      .Then([&](fidl::Result<fuchsia_virtualaudio::Control::AddDevice>& result) {
        if (result.is_error()) {
          printf("ERROR: AddDevice failed: %s\n", result.error_value().FormatDescription().c_str());
          status = ZX_ERR_INTERNAL;
        } else {
          success = true;
        }
        CallbackReceived();
      });

  if (!WaitForCallback() || !success) {
    printf("ERROR: Failed to add device\n");
    QuitLoop();
    return false;
  }

  composite_.Bind(std::move(endpoints->client), loop_->dispatcher(), &event_handler_);

  // let VirtualAudio disconnect if all is not well.
  success = (WaitForNoCallback() && composite_.is_valid());

  if (!success) {
    printf("ERROR: Failed to establish channel to device\n");
  }
  return success;
}

bool VirtualAudioUtil::RemoveDevice() {
  composite_ = {};
  return WaitForNoCallback();
}

bool VirtualAudioUtil::GetFormat() {
  if (!composite_.is_valid()) {
    printf("ERROR: Device not bound - you must add the device before using this flag.\n");
    return false;
  }

  composite_->GetFormat().Then([](fidl::Result<fuchsia_virtualaudio::Device::GetFormat>& result) {
    CallbackReceived();
    if (result.is_error()) {
      printf("GetFormat failed: %s\n", result.error_value().FormatDescription().c_str());
      return;
    }
    FormatNotification(result.value().frames_per_second(), result.value().sample_format(),
                       result.value().num_channels(), result.value().external_delay());
  });

  return WaitForCallback();
}

bool VirtualAudioUtil::GetBuffer() {
  if (!composite_.is_valid()) {
    printf("ERROR: Device not bound - you must add the device before using this flag.\n");
    return false;
  }

  composite_->GetBuffer().Then([](fidl::Result<fuchsia_virtualaudio::Device::GetBuffer>& result) {
    CallbackReceived();
    if (result.is_error()) {
      printf("GetBuffer failed: %s\n", result.error_value().FormatDescription().c_str());
      return;
    }
    BufferNotification(std::move(result.value().ring_buffer()),
                       result.value().num_ring_buffer_frames(),
                       result.value().notifications_per_ring());
  });

  return WaitForCallback() && ring_buffer_vmo_.is_valid();
}

bool VirtualAudioUtil::WriteBuffer(const std::string& write_value_str) {
  size_t value_to_write =
      (write_value_str.empty() ? kDefaultValueToWrite
                               : fxl::StringToNumber<size_t>(write_value_str, fxl::Base::k16));

  if (!ring_buffer_vmo_.is_valid()) {
    if (!GetBuffer()) {
      printf("ERROR: Failed to retrieve RingBuffer for writing.\n");
      return false;
    }
  }

  auto rb_size = rb_size_;
  for (size_t offset = 0; offset < rb_size; offset += sizeof(value_to_write)) {
    zx_status_t status = ring_buffer_vmo_.write(&value_to_write, offset, sizeof(value_to_write));
    if (status != ZX_OK) {
      printf("ERROR: Writing %16ld (0x%016zX) to rb_vmo[%zu] failed (%d)\n", value_to_write,
             value_to_write, offset, status);
      return false;
    }
  }

  printf("--Wrote %16ld (0x%016zX) across the ring buffer\n", value_to_write, value_to_write);

  return WaitForNoCallback();
}

bool VirtualAudioUtil::GetPosition() {
  if (!composite_.is_valid()) {
    printf("ERROR: Device not bound - you must add the device before using this flag.\n");
    return false;
  }

  composite_->GetPosition().Then(
      [](fidl::Result<fuchsia_virtualaudio::Device::GetPosition>& result) {
        CallbackReceived();
        if (result.is_error()) {
          printf("GetPosition failed: %s\n", result.error_value().FormatDescription().c_str());
          return;
        }
        PositionNotification(result.value().monotonic_time(), result.value().ring_position());
      });

  return WaitForCallback();
}

bool VirtualAudioUtil::SetNotificationFrequency(const std::string& notifs_str) {
  if (!composite_.is_valid()) {
    printf("ERROR: Device not bound - you must add the device before using this flag.\n");
    return false;
  }

  uint32_t notifications_per_ring =
      (notifs_str.empty() ? kDefaultNotificationFrequency
                          : fxl::StringToNumber<uint32_t>(notifs_str));
  composite_->SetNotificationFrequency({notifications_per_ring})
      .Then([](fidl::Result<fuchsia_virtualaudio::Device::SetNotificationFrequency>& result) {
        CallbackReceived();
        if (result.is_error()) {
          printf("SetNotificationFrequency failed: %s\n",
                 result.error_value().FormatDescription().c_str());
        }
      });
  return WaitForCallback();
}

void VirtualAudioUtil::CallbackReceived() {
  VirtualAudioUtil::received_callback_ = true;
  VirtualAudioUtil::loop_->Quit();
}

void VirtualAudioUtil::FormatNotification(uint32_t fps, uint32_t fmt, uint32_t chans,
                                          zx_duration_t delay) {
  printf("--Received Format (%u fps, %x fmt, %u chan, %zu delay)\n", fps, fmt, chans, delay);

  frame_size_ = chans * BytesPerSample(fmt);
  ref_time_to_running_position_rate_ = media::TimelineRate(fps * frame_size_, ZX_SEC(1));
}

void VirtualAudioUtil::BufferNotification(zx::vmo ring_buffer_vmo, uint32_t num_ring_buffer_frames,
                                          uint32_t notifications_per_ring) {
  ring_buffer_vmo_ = std::move(ring_buffer_vmo);
  uint64_t vmo_size;
  ring_buffer_vmo_.get_size(&vmo_size);
  rb_size_ = (static_cast<size_t>(num_ring_buffer_frames * frame_size_));

  printf("--Received SetBuffer (vmo size: %zu, ring size: %zu, frames: %u, notifs: %u)\n", vmo_size,
         rb_size_, num_ring_buffer_frames, notifications_per_ring);
}

void VirtualAudioUtil::UpdateRunningPosition(uint32_t ring_position) {
  if (ring_position <= last_rb_position_) {
    running_position_ += rb_size_;
  }
  running_position_ -= last_rb_position_;
  running_position_ += ring_position;
  last_rb_position_ = ring_position;
}

void VirtualAudioUtil::StartNotification(zx_time_t start_time) {
  printf("--Received Start    (time: %zu)\n", start_time);

  ref_time_to_running_position_ =
      media::TimelineFunction(0, start_time, ref_time_to_running_position_rate_);

  running_position_ = 0;
  last_rb_position_ = 0;
}

void VirtualAudioUtil::StopNotification(zx_time_t stop_time, uint32_t ring_position) {
  auto expected_running_position = ref_time_to_running_position_.Apply(stop_time);
  UpdateRunningPosition(ring_position);

  printf("--Received Stop     (time: %zu, pos: %u)\n", stop_time, ring_position);
  printf("--Stop at  position: expected %zu; actual %zu\n", expected_running_position,
         running_position_);

  running_position_ = 0;
  last_rb_position_ = 0;
}

void VirtualAudioUtil::PositionNotification(zx_time_t monotonic_time_for_position,
                                            uint32_t ring_position) {
  printf("--Received Position (time: %13zu, pos: %6u)", monotonic_time_for_position, ring_position);

  if (monotonic_time_for_position > ref_time_to_running_position_.reference_time()) {
    int64_t expected_running_position =
        ref_time_to_running_position_.Apply(monotonic_time_for_position);

    UpdateRunningPosition(ring_position);
    FX_CHECK(running_position_ <= std::numeric_limits<int64_t>::max());
    int64_t delta = expected_running_position - static_cast<int64_t>(running_position_);
    printf(" - running byte position: expect %8zu  actual %8zu  delta %6zd",
           expected_running_position, running_position_, delta);
  }
  printf("\n");
}

}  // namespace
}  // namespace virtual_audio

int main(int argc, const char** argv) {
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"virtual_audio_util"}).BuildAndInitialize();

  fxl::CommandLine command_line = fxl::CommandLineFromArgcArgv(argc, argv);
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  virtual_audio::VirtualAudioUtil util(&loop);
  util.Run(&command_line);

  return 0;
}
