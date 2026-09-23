# Input report reader library

The [`input_report_reader`][input_report_reader] library facilitates the implementation
of input drivers by providing one of the commonly required functionalities: managing and
keeping track of upstream drivers that want to receive reports.

`InputReportReaderManager` in the library creates and manages `InputReportReaders`, which
implement [`fuchsia.input.report.InputReportsReader`][device.fidl]. The manager can send
reports to all existing `InputReportReaders`.

An upstream driver that wants to read input reports from an input device may register with
`InputReportReaderManager` by calling the `CreateReader` method. When an input report arrives
(whether in the form of HID reports or device readings by polling and so on), the report is
pushed to all the registered readers using the `SendReportToAllReaders` method. The report is
then translated to `fuchsia_input_report::InputReport`. When the `InputReportReaderManager`
instance is destructed, all of the existing `InputReportReaders` are freed.

## How to use

The steps below walk through how to use the input report reader library in an input device:

1. Define a class that holds all the data that needs to be reported (this struct must declare
   the required `ToFidlInputReport` method), for example:

   ```c++ {:.devsite-disable-click-to-copy}
   struct MyExampleMouseReport {
     zx::time event_time;
     int64_t x;
     int64_t y;

     void ToFidlInputReport(fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>&
                            input_report, fidl::AnyArena& allocator) const;
   }
   ```

1. Implement the `ToFidlInputReport` method, for example:

   ```c++ {:.devsite-disable-click-to-copy}
   void MyExampleMouseReport::ToFidlInputReport(
       fidl::WireTableBuilder<::fuchsia_input_report::wire::InputReport>& input_report,
       fidl::AnyArena& allocator) const {
     auto mouse_input_rpt = fuchsia_input_report::wire::MouseInputReport::Builder(allocator);
     mouse_input_rpt.movement_x(x);
     mouse_input_rpt.movement_y(y);

     input_report.mouse(mouse_input_rpt.Build());
     input_report.event_time(event_time.get());
   }
   ```

1. In your input device, declare an instance of `InputReportReaderManager` templated on
   your struct, for example:

   ```c++ {:.devsite-disable-click-to-copy}
   input_report_reader::InputReportReaderManager<MyExampleMouseReport> readers_;
   ```

1. When a report arrives, parse the report, fill out `MyExampleMouseReport`
   with the report data, and call the `SendReportToAllReaders` method, for example:

   ```c++ {:.devsite-disable-click-to-copy}
   readers_.SendReportToAllReaders(report);
   ```

1. In the implementation of [`fuchsia.input.report.InputDevice.GetInputReportsReader`][device.fidl],
   call the `CreateReader` method, for example:

   ```c++ {:.devsite-disable-click-to-copy}
   void MyExampleInputDevice::GetInputReportsReader(GetInputReportsReaderRequestView request,
                                                 GetInputReportsReaderCompleter::Sync& completer) {
     auto status = readers_.CreateReader(dispatcher_, std::move(request->reader));
     if (status != ZX_OK) {
        // ...
     }
   }
   ```

<!-- Reference links -->

[input_report_reader]: /src/ui/input/lib/input-report-reader
[device.fidl]: /sdk/fidl/fuchsia.input.report/device.fidl
