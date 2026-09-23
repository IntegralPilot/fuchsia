# CQHCI Driver

The CQHCI (Command Queuing Host Controller Interface) driver provides support
for eMMC block devices which support the CQHCI specification for asynchronous
I/O.

## High Level Architecture

### Relationship with the SDMMC Driver

When command queueing is enabled, the CQHCI driver sits on top of the SDMMC
driver and exposes the `fuchsia.hardware.block.volume.Service` protocol instead
of the SDMMC driver. This allows the CQHCI driver to handle block requests and
submit them via the command queue.

I/O requests submitted via the command queue can be handled completely by the
CQHCI driver without involving the lower-level drivers.

While command queueing is enabled in the SD Host Controller, all requests must
be submitted via CQHCI.  Certain interactions (e.g. RPMB) cannot be performed
through the command queue, requiring some degree of coordination between the
CQHCI and SDMMC drivers.

### Threading Model

The driver's threading model is designed to make the "fast" path of a typical
I/O request (read or write) as low-latency as possible, while supporting maximal
parallelism, and also providing a mechanism for certain operations which require
temporarily blocking requests from being submitted.

To this end, the thread model is as follows:

- **Dedicated threads per block session:** Each block session has a dedicated
  thread to handle its request FIFO.  These threads are only
  responsible for *submitting* tasks to the hardware; they do not block waiting
  for the hardware to finish.  These threads may be block (e.g. waiting for
  new FIFO messages, or waiting until they can submit a request to hardware).
- **Dedicated IRQ thread:** A dedicated thread is used to service hardware
  interrupts from.  The IRQ thread is responsible for completing in-flight
  requests and writing responses out on FIFOs.
- **Shared dispatcher for async tasks:** A shared asynchronous dispatcher is
  used for handling async tasks, as described below in "Async Task Loop".

### Async Task Loop

Certain operations in the command queue are stateful (requiring multiple
operations in sequence), or require temporarily pausing submission to the queue.
These operations run in an async task loop, which runs on a shared async
dispatcher.  Tasks are run one at a time, in FIFO order.

An example of such a task is an RPMB (Replay Protected Memory Block) request.
RPMB requests cannot be performed via CQHCI, so the driver must perform a
sequence of events:

1. Stop accepting new requests.
2. Wait for all active requests to finish.
3. Disable the command queue.
4. Forward the RPMB request to the underlying SDMMC driver.
5. Wait for the RPMB request to complete.
6. Re-enable the command queue and resume normal operation.

A second example is a Flush request.  Unlike an RPMB request, a Flush operation
does not need to stop the submission of other standard queued requests, but it
does require statefully submitting a sequence of commands.  This type of request
does not block new I/O requests from being submitted, but it is run
one-at-a-time in the async task loop to ensure proper sequencing.

### Interrupt Handling

The CQHCI driver handles physical hardware interrupts directly, without
involving the lower-level drivers, when possible.   This requires the CQHCI
driver to monitor the SDHCI registers and look for the "command queue interrupt"
bit in the interrupt status register.

The CQHCI driver cannot handle other SDHCI interrupts, and it forwards these to
be serviced by the lower-level drivers.  This is accomplished by triggering a
virtual IRQ object which the SDHCI driver is monitoring, which wakes up the
SDHCI driver to run its usual interrupt service routine.  The CQHCI driver's
interrupt thread waits until the SDHCI driver is done before acking the physical
interrupt, which re-arms it.
