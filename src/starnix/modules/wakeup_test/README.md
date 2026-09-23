# Wakeup test module

 Wakeup Latency measures the time from the processor being suspended, to being fully awake.
 To measure the wakeup latency, the test needs to set a hardware backed
 timer that is used to wakeup at a point in the future. When the alarm
 goes off, the test sends an input event (currently the Power button) to
 wake up the entire system.

 This test needs to be able to be started from inside the starnix
 container, so a device IOCTL interface is used to pass the time to set
 the alarm for and the type of input to use to wake up the device.

 The implementation is a starnix module that is initialized based on a
 feature flag for the wakeup test. This is to support not enabling this
 interface in non-testing builds and deployments.

 The device type is not set, since this device is only used for the wakeup_test
 and there is low probability of callers using the wrong ABI.

 See go/fuchsia-wakeup-latency-test for more details.