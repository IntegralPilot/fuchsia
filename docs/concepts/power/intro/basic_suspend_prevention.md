# Basic Usage


## Preventing system suspension to handle an event

If you want to keep the system awake to process an event, which might be an
interrupt or responding to a FIDL request, there is a very simple API for this.
[`fuchsia.power.system/ActivityGovernor.AcquireWakeLease`][acquire-wake-lease]
returns a [`LeaseToken`][lease-token] which prevents system suspension as long
as the token exists. If you are a driver processing an interrupt, you can ACK
the interrupt after getting the `LeaseToken`. After you are done processing the
event, simply drop the `LeaseToken`.

There are actually not that many use cases that are this simple on Fuchsia. Many
use cases require keeping the system awake while passing the event along to
another component. The next [section][suspend-prevention] discusses this use
case.

[acquire-wake-lease]: https://cs.opensource.google/fuchsia/fuchsia/+/39b9a242c6e2b09731a426cdcf9f1353206fd034:sdk/fidl/fuchsia.power.system/system.fidl;l=245
[lease-token]: https://cs.opensource.google/fuchsia/fuchsia/+/39b9a242c6e2b09731a426cdcf9f1353206fd034:sdk/fidl/fuchsia.power.system/system.fidl;l=72
[suspend-prevention]: basic_suspend_prevention.md
