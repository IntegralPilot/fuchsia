// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/examples/tones/midi_keyboard.h"

#include <lib/component/incoming/cpp/service_member_watcher.h>
#include <lib/fit/defer.h>
#include <lib/syslog/cpp/macros.h>
#include <unistd.h>

#include <cstdio>
#include <iostream>

#include "src/media/audio/examples/tones/midi.h"
#include "src/media/audio/examples/tones/tones.h"

namespace examples {

zx::result<std::unique_ptr<MidiKeyboard>> MidiKeyboard::Create(Tones* owner) {
  component::SyncServiceMemberWatcher<fuchsia_hardware_midi::Service::Controller> watcher;

  while (true) {
    zx::result controller = watcher.GetNextInstance(/*stop_at_idle=*/true);
    if (controller.is_error()) {
      if (controller.status_value() == ZX_ERR_STOP) {
        break;
      }
      return controller.take_error();
    }
    auto [client, server] = fidl::Endpoints<fuchsia_hardware_midi::Device>::Create();

    {
      const fidl::Status result =
          fidl::WireCall(controller.value())->OpenSession(std::move(server));
      if (!result.ok()) {
        return zx::error(result.status());
      }
    }

    const fidl::WireResult result = fidl::WireCall(client)->GetInfo();
    if (!result.ok()) {
      return zx::error(result.status());
    }
    const fuchsia_hardware_midi::wire::Info& info = result.value().info;

    if (info.is_source) {
      std::cout << "Creating MIDI source\n";
      std::unique_ptr<MidiKeyboard> keyboard(new MidiKeyboard(owner, std::move(client)));
      keyboard->IssueRead();
      return zx::ok(std::move(keyboard));
    }
  }
  return zx::error(ZX_ERR_NOT_FOUND);
}

void MidiKeyboard::IssueRead() {
  dev_->Read().Then([this](::fidl::WireUnownedResult<fuchsia_hardware_midi::Device::Read>& result) {
    if (!result.ok()) {
      FX_LOGS(WARNING) << "Failed to read from MIDI device: " << result.error();
      return;
    }
    if (result->is_error()) {
      FX_LOGS(WARNING) << "Shutting down MIDI keyboard (status " << result->error_value() << " )";
      return;
    }
    HandleRead(*result->value());
  });
}

void MidiKeyboard::HandleRead(const fuchsia_hardware_midi::wire::DeviceReadResponse& response) {
  if (response.event.size() == 0) {
    IssueRead();
    return;
  }

  if (response.event.size() > 3) {
    FX_LOGS(WARNING) << "Shutting down MIDI keyboard, bad event size (" << response.event.size()
                     << ")";
    return;
  }

  std::array<uint8_t, 3> event;
  memcpy(event.data(), response.event.data(), response.event.size());

  // In theory, USB MIDI event sizes are always supposed to be 4 bytes.  1
  // byte for virtual MIDI cable IDs, and then 3 bytes of the MIDI event
  // padded using 0s to normalize the size.  The Fuchsia USB MIDI driver is
  // currently stripping the first byte and passing all virtual cable events
  // along as the same, but the subsequent bytes may or may not be there.
  //
  // For now, we fill our RX buffers with zero before reading, and attempt
  // to be handle the events in that framework.  Specifically, NOTE_ON events
  // with a 7-bit velocity value of 0 are supposed to be treated as NOTE_OFF
  // values.
  uint8_t cmd = event[0] & MIDI_COMMAND_MASK;
  if ((cmd == MIDI_NOTE_ON) || (cmd == MIDI_NOTE_OFF)) {
    // By default, MIDI event sources map the value 60 to middle C.
    static constexpr int kOffsetMiddleC = 60;
    int note = static_cast<int>(event[1] & MIDI_NOTE_NUMBER_MASK) - kOffsetMiddleC;
    int velocity = static_cast<int>(event[2] & MIDI_NOTE_VELOCITY_MASK);
    bool note_on = (cmd == MIDI_NOTE_ON) && (velocity != 0);

    owner_->HandleMidiNote(note, velocity, note_on);
  }

  IssueRead();
}

}  // namespace examples
