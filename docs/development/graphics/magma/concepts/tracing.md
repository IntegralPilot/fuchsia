# Magma: Tracing

For an overview of Magma including background, hardware requirements, and
description of architecture, please see [Magma: Overview](/docs/development/graphics/magma/README.md).

For an overview of Fuchsia tracing, please see [Fuchsia tracing guides](/docs/development/tracing/README.md).

This page outlines the trace events that a Magma System Driver should be instrumented with.

There are currently no requirements for instrumenting ICD code with tracing.

## Flow events

Flow events are important because they connect related tracing events across processes and threads.
They are also helpful at connecting tracing events across time within the same thread.

### gfx events

An MSD should be instrumented with specific flow events that connect the application's request to
the GPU, and connect the GPU to the display. These flow events will all have the `gfx` category.

#### event_signal

The MSD should start a flow event with the following info whenever it signals a semaphore:

- Trace type: Flow Start
- Category: `gfx`
- Name: `event_signal`
- Flow Id: The semaphore's koid

This flow event should be ended by the client that is waiting on this semaphore.

#### semaphore

The MSD should end a flow event with the following info when it receives a command buffer from a client.
A flow event end should be added for each signal semaphore that is present with the command buffer.

- Trace type: Flow End
- Category: `gfx`
- Name: `semaphore`
- Flow Id: The semaphore's koid

This flow event should be started by the client before sending the command buffer.

### magma events

An MSD should be instrumented with flow events that connect related work across time and across
the client and device thread. These flow events should have the `magma` category.

#### Command buffers

The MSD should start a flow event upon receiving a command buffer. The following should be instrumented:

- Receiving a command buffer
- Scheduling the command buffer on the GPU
- Running the command buffer on the GPU
- Finishing the command buffer

The name and tracking information for each flow event is left up to the discretion of each MSD.

#### Semaphores

The MSD should start a flow event when waiting on any semaphores. The following should be instrumented:

- Waiting on a semaphore
- Signaling a semaphore
- Finishing a semaphore wait

## Durations

Which trace durations should be added are left to the discretion of each MSD.
Each MSD should aim to have durations for the following:

- Scheduling and Executing commands
- Mapping and Unmapping buffers
- Handling FIDL requests from clients
- Any slow operation, such as blocking or sleeping

When adding instrumentation it can be helpful to take an existing trace and look at the CPU
scheduling of the MSD threads. If there are long periods where the MSD is running on the CPU
with no active durations, then more instrumentation should be added.

### Naming conventions

In general, MSD durations should use the `magma` category. Using a duration on a function is
the most common case, and the duration name should match the class and function name. Durations
within a function should be named as if it represented a sub function. For C++ this means durations
should be named in PascalCase, and for Rust they should be named in snake_case.

## Counters

### GPU utilization

There should be a counter with the following information:

- Category: `magma`
- Name: `GPU Utilization`
- Counter Id: 0
- Items: `utilization`

The `utilization` item should be a double from 0 to 1 that represents the percentage of time
the GPU has been actively performing work in the last 100 milliseconds.

## Virtual Threads

Virtual threads are a way of visualizing work that happens synchronously on a timeline
other than the CPU's. They are useful to represent work happening on the GPU.

### Job slots

A virtual thread should be created for each piece of hardware on the GPU that
can execute command buffers simultaneously.

A virtual thread duration should be started when command buffers are executing on the GPU and
they should be ended when the commands have completed.

Virtual thread flows should connect the durations to durations on the device thread that are
submitting work and handling completions.

