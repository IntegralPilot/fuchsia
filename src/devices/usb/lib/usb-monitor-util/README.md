# USB Monitor

USB monitor tool can be used to capture USB transactions on the device. This tools is still under
development.

Currently these components are integrated with USB Monitor -

* USB Peripheral driver

## Usage

1. Build an image with tracing support as mentioned in [recording a trace
   document](/docs/development/tracing/tutorial/recording-a-fuchsia-trace.md).
2. Record trace when desired to monitor USB transactions.
3. USB Transaction information would be recorded in traces under "USB Monitor Util" category.
