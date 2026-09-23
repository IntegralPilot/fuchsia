// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/tools/print-input-report/devices.h"

#include <fidl/fuchsia.input.report/cpp/wire.h>
#include <lib/ddk/device.h>
#include <lib/trace/event.h>
#include <zircon/status.h>

#include <cinttypes>

namespace print_input_report {

zx_status_t PrintFeatureReports(std::string filename, Printer* printer,
                                fidl::WireSharedClient<fuchsia_input_report::InputDevice> client,
                                fit::closure callback) {
  client->GetFeatureReport().ThenExactlyOnce(
      [filename = std::move(filename), printer, callback = std::move(callback), _ = client.Clone()](
          fidl::WireUnownedResult<fuchsia_input_report::InputDevice::GetFeatureReport>&
              call_result) {
        if (!call_result.ok()) {
          return;
        }
        auto* result = call_result.Unwrap();
        if (result->is_error()) {
          callback();
          return;
        }
        auto& report = result->value()->report;

        printer->SetIndent(0);
        printer->Print("Feature Report from file: %s\n", filename.c_str());
        printer->IncreaseIndent();
        if (report.has_sensor()) {
          printer->Print("Sensor Feature Report:\n");
          printer->IncreaseIndent();
          if (report.sensor().has_report_interval()) {
            printer->Print("Report Interval: %ld\n", report.sensor().report_interval());
          }
          if (report.sensor().has_reporting_state()) {
            printer->Print("Reporting State: %u\n", report.sensor().reporting_state());
          }
          if (report.sensor().has_sensitivity()) {
            printer->Print("Sensitivity:\n");
            printer->IncreaseIndent();
            for (size_t i = 0; i < report.sensor().sensitivity().size(); i++) {
              printer->Print("%ld ", report.sensor().sensitivity().data()[i]);
            }
            printer->Print("\n");
            printer->DecreaseIndent();
          }
          if (report.sensor().has_threshold_high()) {
            printer->Print("Threshold High:\n");
            printer->IncreaseIndent();
            for (size_t i = 0; i < report.sensor().threshold_high().size(); i++) {
              printer->Print("%ld ", report.sensor().threshold_high().data()[i]);
            }
            printer->Print("\n");
            printer->DecreaseIndent();
          }
          if (report.sensor().has_threshold_low()) {
            printer->Print("Threshold Low:\n");
            printer->IncreaseIndent();
            for (size_t i = 0; i < report.sensor().threshold_low().size(); i++) {
              printer->Print("%ld ", report.sensor().threshold_low().data()[i]);
            }
            printer->Print("\n");
            printer->DecreaseIndent();
          }
          if (report.sensor().has_sampling_rate()) {
            printer->Print("Sampling Rate: %u\n", report.sensor().sampling_rate());
          }
          printer->DecreaseIndent();
        }

        if (report.has_touch()) {
          printer->Print("Touch Feature Report:\n");
          printer->IncreaseIndent();
          if (report.touch().has_input_mode()) {
            printer->Print("Input Mode: %u\n", report.touch().input_mode());
          }
          if (report.touch().has_selective_reporting()) {
            printer->Print("Selective Reporting:\n");
            printer->IncreaseIndent();
            if (report.touch().selective_reporting().has_surface_switch()) {
              printer->Print("Surface Switch: %u\n",
                             report.touch().selective_reporting().surface_switch());
            }
            if (report.touch().selective_reporting().has_button_switch()) {
              printer->Print("Button Switch: %u\n",
                             report.touch().selective_reporting().button_switch());
            }
            printer->DecreaseIndent();
          }
          printer->DecreaseIndent();
        }
        printer->DecreaseIndent();
        callback();
      });
  return ZX_OK;
}

zx_status_t PrintInputDescriptor(std::string filename, Printer* printer,
                                 fidl::WireSharedClient<fuchsia_input_report::InputDevice> client,
                                 fit::closure callback) {
  client->GetDescriptor().ThenExactlyOnce(
      [filename = std::move(filename), printer, callback = std::move(callback), _ = client.Clone()](
          fidl::WireUnownedResult<fuchsia_input_report::InputDevice::GetDescriptor>& call_result) {
        if (!call_result.ok()) {
          return;
        }
        auto* result = call_result.Unwrap();
        printer->SetIndent(0);
        printer->Print("Descriptor from file: %s\n", filename.c_str());
        if (result->descriptor.has_device_information()) {
          PrintDeviceInfo(printer, result->descriptor.device_information());
        }
        if (result->descriptor.has_mouse()) {
          if (result->descriptor.mouse().has_input()) {
            PrintMouseDesc(printer, result->descriptor.mouse().input());
          }
        }
        if (result->descriptor.has_sensor()) {
          if (result->descriptor.sensor().has_input()) {
            for (const auto& input : result->descriptor.sensor().input()) {
              PrintSensorDesc(printer, input);
            }
          }
        }
        if (result->descriptor.has_touch()) {
          PrintTouchDesc(printer, result->descriptor.touch());
        }
        if (result->descriptor.has_keyboard()) {
          PrintKeyboardDesc(printer, result->descriptor.keyboard());
        }
        if (result->descriptor.has_consumer_control()) {
          PrintConsumerControlDesc(printer, result->descriptor.consumer_control());
        }
        callback();
      });
  return ZX_OK;
}

void PrintDeviceInfo(Printer* printer,
                     const fuchsia_input_report::wire::DeviceInformation& device_info) {
  printer->Print("Device Info:\n");
  printer->IncreaseIndent();

  printer->Print("Vendor ID:\n");
  printer->IncreaseIndent();
  if (device_info.has_vendor_id()) {
    printer->Print("0x%" PRIx32 "\n", device_info.vendor_id());
  } else {
    printer->Print("No vendor ID available\n");
  }
  printer->DecreaseIndent();

  printer->Print("Product ID:\n");
  printer->IncreaseIndent();
  if (device_info.has_product_id()) {
    printer->Print("0x%" PRIx32 "\n", device_info.product_id());
  } else {
    printer->Print("No product ID available\n");
  }
  printer->DecreaseIndent();

  printer->Print("Version:\n");
  printer->IncreaseIndent();
  if (device_info.has_version()) {
    printer->Print("0x%" PRIx32 "\n", device_info.version());
  } else {
    printer->Print("No version available\n");
  }
  printer->DecreaseIndent();

  printer->Print("Polling Rate:\n");
  printer->IncreaseIndent();
  if (device_info.has_polling_rate()) {
    printer->Print("%" PRIi64 " usec\n", device_info.polling_rate());
  } else {
    printer->Print("No polling rate available\n");
  }
  printer->DecreaseIndent();

  printer->Print("Manufacturer Name:\n");
  printer->IncreaseIndent();
  if (device_info.has_manufacturer_name()) {
    printer->Print("%s\n", device_info.manufacturer_name().data());
  } else {
    printer->Print("None\n");
  }
  printer->DecreaseIndent();

  printer->Print("Product Name:\n");
  printer->IncreaseIndent();
  if (device_info.has_product_name()) {
    printer->Print("%s\n", device_info.product_name().data());
  } else {
    printer->Print("None\n");
  }
  printer->DecreaseIndent();

  printer->Print("Serial Number:\n");
  printer->IncreaseIndent();
  if (device_info.has_serial_number()) {
    printer->Print("%s\n", device_info.serial_number().data());
  } else {
    printer->Print("None\n");
  }
  printer->DecreaseIndent();

  printer->DecreaseIndent();
}

void PrintMouseDesc(Printer* printer,
                    const fuchsia_input_report::wire::MouseInputDescriptor& mouse_desc) {
  printer->Print("Mouse Descriptor:\n");
  printer->IncreaseIndent();
  if (mouse_desc.has_movement_x()) {
    printer->Print("Movement X:\n");
    printer->PrintAxisIndented(mouse_desc.movement_x());
  }
  if (mouse_desc.has_movement_y()) {
    printer->Print("Movement Y:\n");
    printer->PrintAxisIndented(mouse_desc.movement_y());
  }
  if (mouse_desc.has_position_x()) {
    printer->Print("Position X:\n");
    printer->PrintAxisIndented(mouse_desc.position_x());
  }
  if (mouse_desc.has_position_y()) {
    printer->Print("Position Y:\n");
    printer->PrintAxisIndented(mouse_desc.position_y());
  }
  if (mouse_desc.has_buttons()) {
    for (uint8_t button : mouse_desc.buttons()) {
      printer->Print("Button: %d\n", button);
    }
  }
  printer->DecreaseIndent();
}

void PrintSensorDesc(Printer* printer,
                     const fuchsia_input_report::wire::SensorInputDescriptor& sensor_desc) {
  printer->Print("Sensor Descriptor:\n");
  if (!sensor_desc.has_values()) {
    return;
  }

  printer->IncreaseIndent();
  if (sensor_desc.has_report_id()) {
    printer->Print("ReportID: %02d\n", sensor_desc.report_id());
  }
  for (size_t i = 0; i < sensor_desc.values().size(); i++) {
    printer->Print("Value %02d:\n", i);
    printer->IncreaseIndent();
    printer->Print("SensorType: %s\n", Printer::SensorTypeToString(sensor_desc.values()[i].type));
    printer->PrintAxis(sensor_desc.values()[i].axis);
    printer->DecreaseIndent();
  }
  printer->DecreaseIndent();
}

void PrintTouchDesc(Printer* printer,
                    const fuchsia_input_report::wire::TouchDescriptor& touch_desc) {
  printer->Print("Touch Descriptor:\n");
  printer->IncreaseIndent();
  if (touch_desc.has_input()) {
    printer->Print("Input Report:\n");
    printer->IncreaseIndent();
    const auto& input = touch_desc.input();
    if (input.has_touch_type()) {
      printer->Print("Touch Type: %s\n", Printer::TouchTypeToString(input.touch_type()));
    }
    if (input.has_max_contacts()) {
      printer->Print("Max Contacts: %ld\n", input.max_contacts());
    }
    if (input.has_buttons()) {
      for (size_t i = 0; i < input.buttons().size(); i++) {
        printer->Print("Button: %s\n", Printer::TouchButtonToString(input.buttons()[i]));
      }
    }
    if (input.has_contacts()) {
      for (size_t i = 0; i < input.contacts().size(); i++) {
        const fuchsia_input_report::wire::ContactInputDescriptor& contact = input.contacts()[i];

        printer->Print("Contact: %02d\n", i);
        printer->IncreaseIndent();

        if (contact.has_position_x()) {
          printer->Print("Position X:\n");
          printer->PrintAxisIndented(contact.position_x());
        }
        if (contact.has_position_y()) {
          printer->Print("Position Y:\n");
          printer->PrintAxisIndented(contact.position_y());
        }
        if (contact.has_pressure()) {
          printer->Print("Pressure:\n");
          printer->PrintAxisIndented(contact.pressure());
        }
        if (contact.has_contact_width()) {
          printer->Print("Contact Width:\n");
          printer->PrintAxisIndented(contact.contact_width());
        }
        if (contact.has_contact_height()) {
          printer->Print("Contact Height:\n");
          printer->PrintAxisIndented(contact.contact_height());
        }

        printer->DecreaseIndent();
      }
    }

    printer->DecreaseIndent();
  }

  if (touch_desc.has_feature()) {
    printer->Print("Feature Report:\n");
    printer->IncreaseIndent();
    const auto& feature = touch_desc.feature();
    printer->Print("Supports InputMode: %u\n",
                   feature.has_supports_input_mode() ? feature.supports_input_mode() : false);
    printer->Print("Supports SelectiveReporting: %u\n", feature.has_supports_selective_reporting()
                                                            ? feature.supports_selective_reporting()
                                                            : false);
    printer->DecreaseIndent();
  }
  printer->DecreaseIndent();
}

void PrintKeyboardDesc(Printer* printer,
                       const fuchsia_input_report::wire::KeyboardDescriptor& keyboard_desc) {
  printer->Print("Keyboard Descriptor:\n");

  if (keyboard_desc.has_input()) {
    const fuchsia_input_report::wire::KeyboardInputDescriptor& input = keyboard_desc.input();
    printer->Print("Input Report:\n");
    printer->IncreaseIndent();
    if (input.has_keys3()) {
      for (size_t i = 0; i < input.keys3().size(); i++) {
        printer->Print("Key: %8ld\n", input.keys3()[i]);
      }
    }
    printer->DecreaseIndent();
  }
  if (keyboard_desc.has_output()) {
    const fuchsia_input_report::wire::KeyboardOutputDescriptor& output = keyboard_desc.output();
    printer->Print("Output Report:\n");
    printer->IncreaseIndent();
    if (output.has_leds()) {
      for (size_t i = 0; i < output.leds().size(); i++) {
        printer->Print("Led: %s\n", Printer::LedTypeToString(output.leds()[i]));
      }
    }
    printer->DecreaseIndent();
  }
}

void PrintConsumerControlDesc(
    Printer* printer, const fuchsia_input_report::wire::ConsumerControlDescriptor& descriptor) {
  printer->Print("ConsumerControl Descriptor:\n");

  if (descriptor.has_input()) {
    const fuchsia_input_report::wire::ConsumerControlInputDescriptor& input = descriptor.input();
    printer->Print("Input Report:\n");
    printer->IncreaseIndent();
    if (input.has_buttons()) {
      for (size_t i = 0; i < input.buttons().size(); i++) {
        printer->Print("Button: %16s\n",
                       Printer::ConsumerControlButtonToString(input.buttons()[i]));
      }
    }
    printer->DecreaseIndent();
  }
}

namespace {

class ReportEventHandler
    : public fidl::WireAsyncEventHandler<fuchsia_input_report::InputReportsReaderV2> {
 public:
  ReportEventHandler(std::string filename, Printer* printer, size_t num_reads,
                     fit::closure callback)
      : filename_(std::move(filename)),
        printer_(printer),
        reads_left_(num_reads),
        callback_(std::move(callback)) {}

  void SetClient(fidl::WireSharedClient<fuchsia_input_report::InputReportsReaderV2> client) {
    client_ = std::move(client);
  }

  void OnInputReports(
      fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) override {
    TRACE_DURATION("input", "print-input-report ReadReports");
    for (auto& report : event->reports) {
      if (reads_left_ == 0) {
        break;
      }
      reads_left_ -= 1;
      printer_->SetIndent(0);
      printer_->Print("Report from file: %s\n", filename_.c_str());
      if (report.has_event_time()) {
        printer_->Print("EventTime: 0x%016lx\n", report.event_time());
      }
      if (report.has_trace_id()) {
        TRACE_FLOW_END("input", "input_report", report.trace_id());
      }
      if (report.has_report_id()) {
        printer_->Print("ReportID: %02d\n", report.report_id());
      }
      if (report.has_mouse()) {
        PrintMouseInputReport(printer_, report.mouse());
      }
      if (report.has_sensor()) {
        PrintSensorInputReport(printer_, report.sensor());
      }
      if (report.has_touch()) {
        PrintTouchInputReport(printer_, report.touch());
      }
      if (report.has_keyboard()) {
        PrintKeyboardInputReport(printer_, report.keyboard());
      }
      if (report.has_consumer_control()) {
        PrintConsumerControlInputReport(printer_, report.consumer_control());
      }
      printer_->Print("\n");
    }

    if (client_.is_valid()) {
      if (auto status = client_->AcknowledgeReports(event->last_report_stamp); !status.ok()) {
        // Channel may be closing.
      }
    }

    if (reads_left_ == 0) {
      Finish();
    }
  }

  void on_fidl_error(::fidl::UnbindInfo info) override { Finish(); }

  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_input_report::InputReportsReaderV2> metadata) override {}

 private:
  void Finish() {
    if (client_.is_valid()) {
      client_.AsyncTeardown();
      client_ = {};
    }
    if (callback_) {
      auto cb = std::move(callback_);
      cb();
    }
  }

  std::string filename_;
  Printer* printer_;
  size_t reads_left_;
  fit::closure callback_;
  fidl::WireSharedClient<fuchsia_input_report::InputReportsReaderV2> client_;
};

}  // namespace

void PrintInputReportsWithReader(std::string filename, Printer* printer,
                                 fidl::ClientEnd<fuchsia_input_report::InputReportsReaderV2> reader,
                                 async_dispatcher_t* dispatcher, size_t num_reads,
                                 fit::closure callback) {
  if (num_reads == 0) {
    callback();
    return;
  }

  auto event_handler = std::make_unique<ReportEventHandler>(std::move(filename), printer, num_reads,
                                                            std::move(callback));
  auto* event_handler_ptr = event_handler.get();

  fidl::WireSharedClient<fuchsia_input_report::InputReportsReaderV2> client(
      std::move(reader), dispatcher, std::move(event_handler));
  event_handler_ptr->SetClient(client.Clone());
}

void PrintInputReports(std::string filename, Printer* printer,
                       fidl::WireSharedClient<fuchsia_input_report::InputDevice> client,
                       async_dispatcher_t* dispatcher, size_t num_reads, fit::closure callback) {
  if (num_reads == 0) {
    callback();
    return;
  }

  auto endpoints = fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2>::Create();
  client
      ->GetInputReportsReaderV2(
          std::move(endpoints.server),
          static_cast<uint16_t>(fuchsia_input_report::wire::kMaxDeviceReportCount))
      .ThenExactlyOnce(
          [dispatcher, printer, filename = std::move(filename), num_reads,
           callback = std::move(callback), reader = std::move(endpoints.client),
           _ = client.Clone()](
              fidl::WireUnownedResult<fuchsia_input_report::InputDevice::GetInputReportsReaderV2>&
                  result) mutable {
            if (!result.ok()) {
              callback();
              return;
            }
            PrintInputReportsWithReader(std::move(filename), printer, std::move(reader), dispatcher,
                                        num_reads, std::move(callback));
          });
}

void PrintMouseInputReport(Printer* printer,
                           const fuchsia_input_report::wire::MouseInputReport& mouse_report) {
  if (mouse_report.has_movement_x()) {
    printer->Print("Movement x: %08ld\n", mouse_report.movement_x());
  }
  if (mouse_report.has_movement_y()) {
    printer->Print("Movement y: %08ld\n", mouse_report.movement_y());
  }
  if (mouse_report.has_position_x()) {
    printer->Print("Position x: %08ld\n", mouse_report.position_x());
  }
  if (mouse_report.has_position_y()) {
    printer->Print("Position y: %08ld\n", mouse_report.position_y());
  }
  if (mouse_report.has_scroll_v()) {
    printer->Print("Scroll v: %08ld\n", mouse_report.scroll_v());
  }
  if (mouse_report.has_pressed_buttons()) {
    for (uint8_t button : mouse_report.pressed_buttons()) {
      printer->Print("Button %02d pressed\n", button);
    }
  }
}

void PrintSensorInputReport(Printer* printer,
                            const fuchsia_input_report::wire::SensorInputReport& sensor_report) {
  if (!sensor_report.has_values()) {
    return;
  }

  for (size_t i = 0; i < sensor_report.values().size(); i++) {
    printer->Print("Sensor[%02d]: %08d\n", i, sensor_report.values()[i]);
  }
}

void PrintTouchInputReport(Printer* printer,
                           const fuchsia_input_report::wire::TouchInputReport& touch_report) {
  if (touch_report.has_contacts()) {
    for (size_t i = 0; i < touch_report.contacts().size(); i++) {
      const fuchsia_input_report::wire::ContactInputReport& contact = touch_report.contacts()[i];

      if (contact.has_contact_id()) {
        printer->Print("Contact ID: %2ld\n", contact.contact_id());
      } else {
        printer->Print("Contact: %2d\n", i);
      }

      printer->IncreaseIndent();
      if (contact.has_position_x()) {
        printer->Print("Position X:     %08ld\n", contact.position_x());
      }
      if (contact.has_position_y()) {
        printer->Print("Position Y:     %08ld\n", contact.position_y());
      }
      if (contact.has_pressure()) {
        printer->Print("Pressure:       %08ld\n", contact.pressure());
      }
      if (contact.has_contact_width()) {
        printer->Print("Contact Width:  %08ld\n", contact.contact_width());
      }
      if (contact.has_contact_height()) {
        printer->Print("Contact Height: %08ld\n", contact.contact_height());
      }
      if (contact.has_confidence()) {
        printer->Print("Confidence: %d\n", contact.confidence());
      }

      printer->DecreaseIndent();
    }
  }
  if (touch_report.has_pressed_buttons()) {
    for (size_t i = 0; i < touch_report.pressed_buttons().size(); i++) {
      printer->Print("Button %s pressed\n",
                     Printer::TouchButtonToString(touch_report.pressed_buttons()[i]));
    }
  }
}

void PrintKeyboardInputReport(
    Printer* printer, const fuchsia_input_report::wire::KeyboardInputReport& keyboard_report) {
  printer->Print("Keyboard Report\n");
  printer->IncreaseIndent();
  if (keyboard_report.has_pressed_keys3()) {
    for (size_t i = 0; i < keyboard_report.pressed_keys3().size(); i++) {
      printer->Print("Key: %8ld\n", keyboard_report.pressed_keys3()[i]);
    }
    if (keyboard_report.pressed_keys3().size() == 0) {
      printer->Print("No keys pressed\n");
    }
  }
  printer->DecreaseIndent();
}

void PrintConsumerControlInputReport(
    Printer* printer, const fuchsia_input_report::wire::ConsumerControlInputReport& report) {
  printer->Print("ConsumerControl Report\n");
  printer->IncreaseIndent();
  if (report.has_pressed_buttons()) {
    for (size_t i = 0; i < report.pressed_buttons().size(); i++) {
      printer->Print("Button: %16s\n",
                     Printer::ConsumerControlButtonToString(report.pressed_buttons()[i]));
    }
  }
  printer->DecreaseIndent();
}

}  // namespace print_input_report
