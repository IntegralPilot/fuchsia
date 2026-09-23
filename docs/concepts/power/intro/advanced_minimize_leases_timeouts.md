# Advanced Usage

## Minimizing wake lease acquisition with timeouts

Note: This solution requires timeouts for correctness and is therefore not
recommended.

Imagine that you have a Driver X which receives interrupts which can wake the
system. The driver does some processing on the interrupt and passes an event on
to Driver Y. We really only want Driver X to obtain a wake lease when an
interrupt arrives which actually wakes the system or when Driver X observes the
system starting to suspend within a timeout period of the last time we received
an interrupt.

This strategy is not relevant when using
[Protocol Interlocks][protocol_interlocks], since that strategy provides
guarantees that Driver Y processed the event.

Refer to [Taking Action on Suspend or Resume][taking_action] for how to observe
system suspend transitions. If Driver X receives an interrupt after it observes
suspend and before it observes resume then it should call
`ActivityGovernor.AcquireWakeLease`. Driver X can then ack the interrupt,
schedule a time to drop the wake lease, and pass the event on to Driver Y.

Driver X must keep a record of the last time an interrupt arrived. When Driver X
observes the system suspending it must see if the current time is less than the
last interrupt arrival time plus the timeout and if so, acquire a wake lease and
hold it until the timeout for the last received interrupt arrives.

Note: The [`TimeoutWakeLease`][wake_lease] class implements this strategy in C++
with the use of its `HandleInterrupt` method.

Using timeouts as a solution here is challenging because we must guess how long
it will take Driver Y to receive the event and process it. Practically speaking,
a good timeout value is even harder because often it won't be just the time in
Driver Y we must account for, but also Component 1, Component 2, etc. Another
challenge is picking a timeout that works on all system configurations Driver X
runs on.

[protocol_interlocks]: basic_pipeline_processing.md#protocol-interlocks
[taking_action]: basic_suspend_resume.md#taking-action-on-system-suspend-or-resume
[wake_lease]: https://cs.opensource.google/fuchsia/fuchsia/+/39b9a242c6e2b09731a426cdcf9f1353206fd034:sdk/lib/driver/power/cpp/wake-lease.h
