# Advanced Usage

## Minimizing wake lease acquisition without timeouts

Imagine that you have a Driver X which receives interrupts which can wake the
system. Driver X then does some processing on the interrupt and passes an event
on to Driver Y. Ideally Driver X only obtains a wake lease when an interrupt
arrives which actually wakes the system or when Driver X observes the system
starting to suspend and **does not know if Driver Y has processed a previous
event**.

For both of the approaches below we need to observe when the system is
suspending and resuming. Refer to
[Taking Action on Suspend or Resume][taking_action] for how to observe these
system transitions.

Note: There is no helper library to implement this strategy, but making one is
possible and we'd love to do the work! If you want or need to use this approach,
please reach out so we can prioritize accordingly.

### In-band leases

Let's first assume that Driver X and Driver Y speak a protocol where messages
can contain event data, a wake lease, or event data and a wake lease. In FIDL
we'd represent this as a table with entries for wake lease and event data.

When Driver X receives an interrupt while the system is suspended it:

* Creates a wake lease
* Acks the interrupt
* Increments its sequence number for the last event it received
* Increments its sequence number for the last event when it made a wake lease
* Sends Driver Y the event and the wake lease

When Driver X receives an event while the system is resumed (ie. not suspended)
it:

* Acks the interrupt
* Increments its sequence number for the last event it received
* Sends Driver Y the event

When Driver X observes the system starting to suspend it:

* Compares the sequence number for the last event received to the number for the
  last created wake lease and if they don't match it:

  * Creates a wake lease
  * Sends Driver Y the wake lease

Driver Y receives messages and if the message contains a wake lease, it holds it
until it processes whatever its most recent event is from Driver X.

### Out-of-band leases

Let's first assume that Driver X and Driver Y speak a protocol that only talks
about their domain events and use a separate protocol (or separate methods of a
single protocol) for passing wake leases.

When Driver X receives an interrupt while the system is suspended:

*   Driver X:

    *   Creates a wake lease
    *   Acks the interrupt
    *   Increments its sequence number for the last event it received
    *   Increments its sequence number for the last event when it made a wake lease
    *   Sends Driver Y the event
    *   Sends Driver Y the wake lease and sequence number

*   Driver Y (note that it can receive the two messages in either order):

    *   Receives the event:

        *   Processes it
        *   Increments its last received sequence number

    *   Receives the wake lease and sequence number:

        *   Holds the wake lease until its last seen sequence number matches the
            lease's sequence number

When Driver X receives an event while the system is resumed (ie. not suspended):

*   Driver X:

    *   Acks the interrupt
    *   Increments its sequence number for the last event it received
    *   Sends Driver Y the event

*   Driver Y:

    *   Receives the event and processes it
    *   Increments its last received sequence number

When Driver X observes the system starting to suspend:

*   Driver X:

    *   Compares the sequence number for the last event received to the number
        for the last created wake lease and if they don't match it:

        *   Creates a wake lease
        *   Updates its sequence number for the last event it created a wake
            lease for to match the sequence number of the last event received
        *   Sends Driver Y the wake lease and sequence number

*   Driver Y:

    *   Receives the wake lease and sequence number
    *   Holds the wake lease until its last seen sequence number matches the
        lease's sequence number

[taking_action]: basic_suspend_resume.md#taking-action-on-system-suspend-or-resume
