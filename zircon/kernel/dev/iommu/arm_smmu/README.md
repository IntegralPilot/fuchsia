# The Zircon ARM SMMU driver.

## Overview

This document is intended to give a high level overview of:
1) ARM SMMU hardware and generally how it functions.
2) The overall structure of the Zircon driver and how it uses SMMU hardware to
   satisfy the Zircon IOMMU syscall API.
3) Assumptions and limitations of the current driver implementation.
4) Possible future directions to add more functionality as the needs arise.

## References

This driver was written using
[`ARM IHI 0062D.c`](https://developer.arm.com/documentation/ihi0062/latest),
aka the "ARM System Memory Management Unit Architecture Specification ®SMMU
architecture version 2.0", as its primary reference.  All references to section
and figure numbers in code comments have been taken from this document, and it
is recommended that engineers working on the code familiarize themselves with
it.

Additionally,
[`ARM DDI 0487K.a`](https://developer.arm.com/documentation/ddi0487/latest/),
aka the "Arm Architecture Reference Manual for A-profile architecture".
Specifically section `D8.3.1` "VMSAv8-64 descriptor formats".

Hardware discovery currently depends on hardware descriptions being present in
the device tree passed to physboot/Zircon by lower levels during boot.
Documentation on how SMMU is described by the device tree can be found
[here](https://www.kernel.org/doc/Documentation/devicetree/bindings/iommu/arm%2Csmmu.txt)

### Terms and Abbreviations

1) SMMU: System Memory Management Unit.  The on-chip hardware used to virtualise
   address spaces for different bus initiators.
2) BTI: Bus Transaction Initiator.  A non-AP device capable of placing
   load/store transactions on the main system bus.  For example, the DMA
   controller of an XHCI unit.  BTIs are a Zircon term, and part of the Zircon
   IOMMU APIs.  It is *not* a term used in ARM documentation.
3) Stream ID or SID:  A unique identifier present in every bus transaction
   initiated by a BTI.  Used by SMMU hardware to identify how transactions
   should be treated, whether it be to allow them with or without address
   translation, or block them as well as what to do when blocking a transaction.
   SIDs are frequently hardwired by a designer into the fixed hardware blocks
   for their SoCs, but can also be somewhat dynamic as when PCIe BDFs are used
   as SIDs for modular hardware.
4) SMRG: Stream Match Register Group.  A set of SMMU registers which implements
   one of the (several) ways to recognize the Stream ID present in a
   transaction, and route it to the appropriate set of user-defined transaction
   handling policies.
5) SMR: Stream Match Register.  One of the two registers which together
   compromise an SMRG.  The SMR holds the filter used to match a set of Stream
   IDs.
6) CB: Context Bank.  A block of SMMU hardware used to implement transaction
   policy for one or more Stream IDs, particularly address translation.  A
   context bank can be thought of as the hardware used to provide a virtual
   address space for a set of Stream IDs linked to this context bank via SMRG
   configuration

## Hardware Overview

A SMMU is logically responsible for either allowing or blocking load/store
transactions initiated by non-AP devices on a per-Stream ID basis, in addition
to determining the final physical address used for the operation, as well as
determining what the initiator hardware sees in the case that a transaction is
blocked.  The three options for a blocked transaction from the initiator's point
of view are:

+ RAZ/WI    : Read as zero, Write ignored.  The initiator will see only zeros
              for reads, and writes will be silently ignored.
+ Bus ABORT : The transaction will not take place, and an ABORT signal will be
              sent to the initiator hardware via the system bus to handle as it
              sees fit.
+ Stall     : The transaction will be stalled and an interrupt raised, allowing
              the OS to "handle" the fault as it sees fit before resuming the
              transaction.

See [Context bank faults](#context-bank-faults) for more details.

Systems are *not* limited to having a single SMMU.  Multiple SMMUs can exist in
system designs, and different initiators in that design may be connected to
different SMMUs, or even no SMMU at all.  The specific topology is entirely
determined by the system designer, and is described in the device tree for the
system.

### Secure and non-secure transactions

When a bus transaction is initiated, there are many properties of the
transaction presented to the hardware in addition to the Stream ID.  One of
these properties is a bit indicating whether or not the transaction is
considered to be "secure".  For example, the transactions used by the output of
a media decryption unit involved in DRM operations may all be flagged as
"secure" so as to allow them to target secure memory set aside by a system's
secure monitor.  The ability of a device to place a secure transaction on the
bus is determined by the SOC implementer and is usually associated with specific
Stream IDs in a hardwired fashion.

SMMU hardware may, but is not required to, support a partitioning in hardware
for handing secure and non-secure transactions.  When supported, many of the
SMMU registers become "banked", which is to say that there are two different
views of the registers seen by APs accessing them depending on whether or not an
AP is currently executing in secure mode or not.  The hardware is arranged such
that secure mode (presumably during early boot) may "claim" groups of hardware
(SMRGs and CBs) needed to implement policy in a way which hides the claimed
hardware from the non-secure mode.

By the time Zircon starts to initialize SMMU hardware, all of the secure
hardware (if any) will have been claimed by the secure monitor and will be
invisible to Zircon.  In general, handling of secure transactions is the domain
of the secure monitor and completely opaque to Zircon.  Secure transaction
processing is outside of the scope of the Zircon driver.

### The life of a transaction

#### Stream Identification

When handling a transaction, the first thing which must happen is stream
identification, starting with secure vs. non-secure identification.
Transactions identified as secure are sent to the secure view of the SMMU
hardware, which is controlled by the secure monitor and outside of the scope of
the Zircon driver.  Non-secure transactions are processed by the hardware which
is under Zircon control.

Following this, the transaction's Stream ID needs to be matched to a policy
configured by the Zircon driver.  There are multiple ways to do this.  Which of
these methods are available will be determined by the version and configuration
of the SMMU hardware.  The purpose of this process is to find an SMRG which
contains the configuration for how to handle the transaction.  If no SMRG can be
found, the global policy will be applied instead, which would typically be
configured to block the transaction.

##### Indexed Mode

The oldest way of doing this is called "Indexed Mode".  When operating in
indexed mode, the Stream ID is used as an index into the array of SMRGs present
in the hardware.  So, if an SMMU has 64 SMRG instances, a Stream ID of 7 would
map directly to SMRG[7] in the hardware.  The only valid stream IDs in the
system would be IDs [0, 63].  It is the responsibility of the system implementer
to ensure that their implemented stream IDs all exist within the valid indexing
range.  If they do not, indexed mode cannot be used.

The Zircon driver does not currently support indexed mode as it is expected to
only ever be used with early versions of the hardware.  If support for hardware
which only implements indexed mode is ever needed, the driver will need to be
extended.

##### Stream Matching

This is the more common mode for Stream ID matching, and the one used by the
Zircon driver.  By default, there are a maximum of 128 SMRGs in an SMMU
instance, each of which contains two registers.  They are:

+ SMR: The "Stream Match Register".  Used to match a group of Stream IDs to this
  SMRG.
+ S2CR: The "Stream to Context Register".  Used to specify either a default
  transaction handling policy, or to forward the handling to a configured
  Context Bank.

As its name implies, the SMR is responsible for the matching process.  It
consists of two 15-bit fields, the mask and the ID, in addition to a single
"valid" bit.  The ID field specifies the Stream ID to match, while the mask
field specifies the "don't care" bits during the matching.  So, a stream ID `X`
matches an SMR if `X & ~SMR.mask == SMR.ID & ~SMR.mask`.  This provision allows
multiple Stream IDs to match a single SMRG as it is not uncommon for devices to
be assigned multiple Stream IDs to use in transactions by the system designer.

During matching, HW logically attempts to match X against all SMRGs for which
`SMR.valid` is set, ignoring invalidated SMRGs.  It is a configuration error to
ever allow a single stream ID to match multiple SMRs.  While it is possible for
some SMMU implementations to detect this condition and raise an exception, the
capability to do so is optional, and the behavior in a situation like this is
considered to be undefined by the base specification.

Basic stream matching supports a maximum of 128 SMRGs and stream IDs of up to 15
bits.  This is currently the only mode supported by the Zircon driver.

##### Extended Stream ID Matching

SMMU hardware may optionally support "extended stream ID matching".  Extended
matching expands the ID and mask fields of the SMR to be 16 bits each, allowing
for 16 bit stream IDs.  A side effect of this is that there is no longer room
for a "valid" bit in the 32-bit SMR, and so the bit needs to be moved to a spare
bit in the S2CR.  The layout of these registers is determined by whether or not
extended matching is enabled via the CR0.EXIDENABLE bit.

The Zircon driver does not currently support extended stream matching, and will
always disable it if the feature is present in the hardware.  It is likely that
support will need to be added the first time a device with an SMMU which has
PCI/PCIe needs to be supported.  Stream IDs used by PCI devices tend to be the
BDF (bus-device-function) address of the PCI device, and these addresses are 16
bits.

##### Extended SMRG registers

In addition to providing more bits to use when matching, SMMUs may optionally
extend the number of SMRGs beyond the default maximum of 128, up to a new
maximum of 1024.  When supported, additional SMRGs will be present in a register
page separate from the global registers where the default SMRGs live, and will
be used when `CR2.EXSMRGENABLE` is set.

The Zircon driver does not currently support extended sets of SMRG registers.

##### Compressed index matching

In addition to the default index matching, SMMUs may optionally support
"compressed index matching".  Compressed index matching takes a stream ID and
maps it to a 32-bit LUT, whose index is determining by dividing the Stream ID by
four.  Within the 32-bit value, four 4-bit indexes are present with the index
selected being the Stream ID modulo four.  The final 4-bit index is used to
select an SMRG, and importantly a specific S2CR instance.

Compressed index matching allows a system with many Stream IDs to efficiently
map those IDs to a small number of context banks - 16 at most.

Compressed index matching is not currently supported or used by the Zircon
driver.

#### Default Policy implementation

At this point in the transaction, a policy _may_ be implemented.  If the Stream
ID is not matched, the global policy is used.  Otherwise, an SMRG may be
configured to accept the transaction without performing any address translation,
or to block the transaction without translation and produce a fault instead. The
final option is for the SMRG to be configured for "translation" in which case
the transaction proceeds to the context bank configured by the S2CR member of
the SMRG.

Except for "adopted configurations" (see below), the Zircon driver will never
implement policy at the SMRG level.  Instead, all SMRGs will be configured to
use a specific CB, where the transaction's address may or may not be translated
depending on the configure mode of the driver.

#### Context bank processing

After determining the SMRG to use for a transaction, the ID of the associated
context bank is determined and will be used to finish processing of the
transaction.

##### Registers

SMMUs support up to a maximum of 128 context banks.  Each context bank has three
registers in the global address space, as well as a significantly larger
dedicated page of registers located in the second half of the total SMMU address
space.  The global registers are:

+ CBAR: The "context bank attribute register"
+ CBA2R: The second CBAR, now with an awkward 2 in the middle of it.
+ CBFRSYNRA: The "context bank fault restricted syndrome register A", a register
  used in fault handling.

##### Stage 1 vs. Stage 2 translation

In order to support direct hardware access from inside of a virtual guest, SMMUs
may optionally support a second stage of translation in addition to the first.
This said, it is important to note that SMMUs are not required to support _any_
stages of address translation.

When stage two translation is supported and desired, each stage of translation
requires its own context bank.  The first context bank used is determined using
the matched SMRG, while the second (when used) is determined by a stage 1 CBAR
field.  Stage one is responsible for producing an IPA (or intermediate physical
address), while stage two will translate the IPA into a final physical address.
The design intention here is to allow a hypervisor to virtualise memory provided
to a guest operating system, while still allowing hardware under a guest's
direct control to access the host's physical memory subject to hypervisor
translation.  Context banks may be configured via the CBAR for one of four modes
of operation:

+ Stage 2 Translation Only - Used by hypervisors to perform the second stage of
  translation.
+ Stage 1 Translation with Stage 2 bypass - S1 translation is performed, but S2
  translation is skipped entirely.  Presuming stage 1 does not fault, the
  transaction will be accepted.
+ Stage 1 Translation with Stage 2 fault - S1 translation is performed, and S2
  translation is skipped, but will always result in a fault.
+ Stage 1 Translation with Stage 2 translation - S1 translation is performed,
  and then passed along to a stage two translation context.

The Zircon SMMU driver does not currently support stage 2 translation, and will
always configure its context banks to use stage 1 translation with stage 2
bypass.

Note that it is possible for a platform to force Stage 2 translation regardless
of what mode we configure the CBAR for by virtualising the register using
EL2/sEL3, and either explicitly changing the write from Stage 2 Bypass to Stage
2 Translate, or by programming the register that way, but hiding the fact that
it do so from EL1 by reporting Stage 2 Bypass when it is actually configured for
Translate.

##### Stage 1 processing

Stage 1 Context Bank processing of a transaction *always* begins with address
translation, however there is a rather large `*` attached to this statement.
Translation can mean what is traditionally thought of as address translation.
In other words, consulting a HW TLB cache followed by falling back on in-memory
PTEs in the case of a TLB miss.  However, this translation step _can_ be skipped
entirely if the context bank is configured to skip the MMU translation via the
context bank's `SCTLR.M` bit.  Additionally, the MMU may be enabled while also
disabling both of the translation table base registers (TTBRs) ensuring that
every transaction will always fail translation.

Using this feature, the Zircon driver is able to always map recognized stream
IDs to specific context banks, and then implement one of three possible
behaviors.

1) Always allow the transaction.  Stage 1 Translation with Stage 2 Bypass, and
   with the MMU disabled for stage 1.
2) Always block the transaction.  Stage 1 Translation with Stage 2 Bypass with
   the MMU enabled, but both TTBRs disabled, guaranteeing a fault.
3) Perform address translation and conditionally allow or block the transaction.
   Stage 1 Translation with Stage 2 Bypass with the MMU enabled for stage 1, and
   TTBR0 enabled and configured to point to a hierarchy of PTEs used to
   determine specific policy on a page by page basis.

The primary reason for wanting to always use a context bank for determining
policy, even if that policy could be implemented at the SMRG level, is that
using a context bank allows faults to be reported as context bank specific
faults instead of global faults.  This allows a dedicated set of context bank
registers to be used for fault handing, meaning that we will have access to the
faulting address and transaction type, and that we can stall the entire context
bank and never miss a fault.  When faults occur in the global context, there are
limited registers for reporting faults meaning that it is possible for a second
fault to be missed as the registers used for recording the fault may already be
in use because of a previous first fault.

##### Stage 1 translation

When the MMU is enabled for a context bank, address translation is performed in
a fashion more or less identical to how it is done for APs through the main
system.  There exists a single logical TLB cache for the entire SMMU, however
its size and resource partitioning is an implementation detail and not defined
by the common SMMU specification.

Assuming that a valid PTE is found for a given address, the type of access
(read, write, execute) is checked against the PTE's permissions.  If a valid
entry is discovered and the permissions allow the transaction type, the
transaction is permitted.  Otherwise it is denied.

After stage one processing is complete (and because Zircon does not support
stage 2 processing), if the transaction is permitted, it will be conducted.  If
not, it will generate a fault, record the details of the fault in the CB's fault
syndrome registers, and finally deliver an interrupt to the CB's assigned IRQ.

### Interrupts and fault handling

#### Interrupts

As of SMMUv2, an SMMU instance will always have assigned to it:

+ At least one "global" interrupt.
+ One interrupt for every context bank in the system.

While not specified by the SMMUv2 documentation, it is expected that typical
devices will use GIC SPIs to implement these interrupts.  Regardless of how the
interrupts are manifested, the interrupt configuration (which interrupts are are
SMMU global interrupts for a given SMMU, and which interrupts go with each
context bank of an SMMU) are specified out of band, typically by the device tree
configuration supplied to Zircon.

Prior to v2 hardware, there was no guarantee that there would be a dedicated
interrupt for each context bank in the SMMU, and interrupt resources needed to
be shared via explicit assignment by the driver.  The Zircon driver does not
currently support explicit assignment of interrupts in v1 hardware.

#### Global faults

Global faults and their associated interrupts are used to report two main types
of errors: Configuration errors and transaction errors where no assigned context
bank can be located.

Configuration errors are caused by things like:
+ Stream ID matching is ambiguous because there are multiple enabled SMRs which
  match a transactions SID.  The ability of SMMU HW to detect this is optional,
  not all SMMU implementation can do so.
+ An SMR was found, but it attempts to route the transaction to a type of CB
  which is not supported by the hardware, or to a CB instance which does not
  exist.  For example, the transaction could be routed to a valid context bank
  instance, but the context bank could be configured to perform stage 2
  translation, and the SMMU does not support stage 2 translation.
  Alternatively, the SMR might direct the transaction to a context bank which
  does not exist at all (index out of range).
+ An AP attempts to access an SMMU register which is not implemented.

Examples of transaction errors which lack a context bank include:
+ The transaction's SID is not matched by any SMRs.
+ An SMR is discovered, but is configured to fault the transaction before
  forwarding to a context bank.

In either case, the details of the first global fault received will be recorded
in the SMMUs global fault syndrome registers, however there is no way to prevent
faults occurring concurrently.  The first fault wins, and aside from setting a
"more faults happened" bit, the details of subsequent faults will be lost until
the fault is handled and the syndrome registers are reset.  The SMMU
specification published by ARM consider the race handling behavior to be an
implementation detail, and provides no guarantees on _which_ fault will "win the
race" aside from saying that there will be only one winner.  Further details for
concurrent global fault handling can be found in sections 3.3 and 3.8.1 of the
[ARM SMMU TRM](#references).

Global faults are generally difficult to handle, especially when they are
triggered by an unrecognized HW initiator as there are not many good ways to
prevent the HW from continuing its attempted accesses.  The Zircon driver's
current behavior is to attempt to report the first fault's details, but to then
mask the interrupt which signaled the fault and never re-enable it.

It should be noted that there exist systems which don't permit Zircon to handle
global interrupts, blocking any attempt to enable them from the hypervisor.  In
these systems, forcing a global interrupt to happen (by forcing a SID-match
failure) has been observed to cause the system to hang and then reboot after a
short amount of time.  Apparently, for these systems, the system designer
considered a global fault to be so serious as to warrant an immediate reboot,
and handles the processing with a combination of EL2/sEL3 behavior with no
opportunity for the HLOS to collect debugging information.

#### Context bank faults

Context bank faults occur in response to a transaction for which a specific
context bank was successfully identified to process the transaction.  They
typically happen because of either an inability to translate an address due to
the lack of a valid PTE, or because a permission error such as an attempt to
write to a location when only read access is permitted.

One of three behaviors for handling the transaction in hardware can be
configured when a transaction faults and a context bank fault is raised.  They
are:

+ RAZ/WI
+ Bus ABORT
+ Stall

In the case of RAZ/WI behavior, the initiator's transactions read zeros instead
of the memory it wanted to read, and its transaction writes are silently
ignored. The initiator continues to operate and may continue to generate faults
before the first fault can be handled.  In this situation, there is no place for
the SMMU to record details of subsequent faults, so a "multiple-fault" bit is
set in the CB's fault registers and additional details are lost.

In the case of a Bus ABORT, the transaction is not completed at all.  Instead,
an abort signal is raised on the SoC internal system bus (for example, an AXI
bus).  Subsequent behavior depends entirely on the initiator's implementation.
Some initiators may raise an appropriate interrupt indicating that a DMA
encountered a Bus ABORT, but this is not a guarantee.  It could also simply lock
up the hardware and not communicate anything to the driver.  It is entirely up
to the hardware.

Finally, in the case that a transaction is "stalled", the transaction pauses and
an SMMU fault interrupt is raised.  Driver code _could_ handle the fault by
supplying physical memory to back the target transaction and updating the PTE
before allowing the transaction to resume, or it could allow the fault to
propagate delivering either RAZ/WI or Bus ABORT behavior.  The ability to stall
a transaction depends on the SMMUs specific capabilities.  Even when an SMMU
supports stalling, there are a finite number of transaction which can be
concurrently stalled based on the number of stall contexts the SMMU
implementation was synthesized with.

All of this said, the Zircon driver does not currently support of use
transaction stalling features, even when an SMMU supports such a thing.

## Zircon SMMU Driver Details

### Initialization

Zircon SMMU driver instances are instantiated using details provided in the ZBI
in an array of `zbi_dcfg_arm_smmu_driver_t`, one driver instance per structure
instance.  The initialization information passed via ZBI contains a number of
details, however the most critical of these are:

1) The physical base address of the SMMU registers.  This value not only allows
   the driver to discover and configure the hardware, it also serves as the
   unique identifier for the hardware instance.
2) A collections of interrupt IDs and configurations which serve as the Global
   and Context Bank IRQs.
3) An optional collection of stream IDs which are being "handed off" from the
   bootloader level to the HLOS level.  The SMMU hardware has already been
   configured to grant access to these SIDs, and care must be taken by the
   zircon drivers to preserve this access during operation.

When successfully initialized, the MMIO ranges and IRQs described by the ZBI
become claimed by the driver layer.  User-mode drivers will not be allowed to
create physical VMOs which intersect the MMIO ranges, nor will they be able to
create interrupt objects using the same IRQs.

When user-mode wishes to access an SMMU instance, it makes a syscall to
`zx_iommu_create` passing a `zx_iommu_desc_arm_smmu_t` instance which contains
the physical address of the desired SMMU instance.  If an SMMU driver instance
with this same address exists and has not already been claimed, the driver
instance becomes bound to the `IommuDispatcher` instance being created by the
user and the call succeeds.  Subsequent calls to `zx_iommu_create` will fail
until all of the handles to the originally created dispatcher have been closed
and the driver instance is returned to the global pool.

### Operational Modes

The Zircon SMMU driver subsystem can be configured to operate in one of three
modes based on the value via the `kernel.arm-smmu-mode` boot argument.  These
are:

#### Disabled

The drivers are _functionally_ disabled.  Driver instances will still be created
based on ZBI information, however no resources will be claimed, and the SMMU
hardware configuration will not be changed.  Any attempt by user-mode to create
an IOMMU Dispatcher backed by an SMMU will succeed, however an instance of the
`StubIommu` driver will be substituted instead and stub-iommu behavior will be
in effect.  No checking for base address validity or collision will be made, and
user-mode drivers will be allowed to access both MMIO and IRQ resources which
would otherwise be reserved for the kernel driver.  The primary purpose of
"disabled" mode is to provide a bringup path where the kernel driver can be
checked in while still delegating SMMU control to the user-mode driver
framework.  A secondary benefit is that it lets the kernel SMMU console examine
SMMU register state while the hardware is under user-mode control.

#### Passthru

The drivers are operational, but implement only limited enforcement at the
hardware level.  Users can create BTIs with specific stream IDs which will
result in SMR hardware being set up to recognize those stream IDs which will
then be sent to a context bank configured for translation, but with the MMU
disabled.  Initiators using these recognized stream IDs will effectively have
access to the entire system, their addresses will "passed thru" instead of
translated, and the transactions will always be permitted.

If user-mode fails to explicitly unpin a PMT before closing all handles to it,
the behavior of the kernel SMMU driver is similar to the behavior of a Stub
IOMMU driver, but not quite the same.  The leaking of a PMT is considered to be
an error, and will put a user's BTI into a "faulted" state where attempts to pin
new memory for the faulted BTI will be rejected, identical to the behavior of a
stub driver.  The main difference between a stub driver and an SMMU driver
operating in passthru mode has to do with what happens to the leaked pinned
memory.  A stub driver is forced to place the memory into a quarantine pool, not
returning it to the central kernel pool until the driver has completed the
quarantine recovery protocol.  A passthru-SMMU driver, however, will simply
revoke access at the hardware level from the collection of stream IDs which make
up the BTI. *All* transactions initiated by these stream IDs will now always
fault, this time at the context bank level as the context bank's MMU will be
enabled (but with no valid TTBRs) ensuring that translation table walks will now
always fail. This allows the pinned memory which was leaked to be immediately
returned to the central pool instead of needing to linger in a quarantined
state.  Driver code can recover from this always-fault state by following the
quarantine protocol, the same way as is done when using a passthru driver.

When operating in passthru mode, the kernel claims all of the hardware resources
associated with the SMMU instance.  User mode will not be able to access either
the MMIO registers, nor any of the assigned SMMU IRQs.

#### Enforced

The drivers are operational and implement full enforcement at the hardware level
on a per-page level, and with access type enforcement.  When operating in
enforced mode, the act of calling `zx_bti_pin` on a VMO not only pins the pages
(preventing the VMM from swapping them out for different physical pages), but
also creates a mapping in the device's address space with the proper permissions
allowing the device to access the underlying physical pages. Attempts by
recognized stream IDs to access memory which has not been explicitly pinned for
their BTI (meaning that there is no valid mapping in the device's address space)
will result in a [context bank fault](#context-bank-faults) and the BTI entering
a faulted state. Similarly, attempts to access memory with at an address with a
valid PTE, but with the wrong permissions (attempting to write read-only memory,
attempting to fetch for execute memory with no execute permissions) will also
result in a fault.  None of the existing PTEs will be invalidated as a result of
this; the hardware will continue to be allowed to access its pinned memory
provided that its addresses and access flags are valid as defined by the context
bank's current PTEs.  Attempts to pin new memory, however, will be denied. As
with the other modes, once a BTI is in a faulted state, drivers may recover from
the faulted state by following the standard quarantine protocol.

A side effect of operating in enforced mode is that hardware views of pinned
memory will now always be contiguous, thanks to the active re-mapping being done
by the MMU.

When operating in enforced mode, the kernel claims all of the hardware resources
associated with the SMMU instance.  User mode will not be able to access either
the MMIO registers, nor any of the assigned SMMU IRQs.

NOTE: At the time of this writing, enforced mode has not been implemented yet. A
request to operate in enforced mode will currently result in the system
configuring itself for passthru mode instead.

### Kernel Driver Internals

#### The Dispatcher Facing Objects

Kernel IOMMU drivers must provide implementations of three different interfaces
to the dispatcher level of the kernel so that it can implement the syscall
interface.  For the SMMU driver, these implementations are:

##### `arm_smmu::Smmu`

This is the driver's implementation of the `iommu::Iommu`.

The primary purpose of this interface is to create BTI interfaces with specific
stream IDs in response to user-mode requests.  In addition to the interface
contract, the `Smmu` implementation is generally responsible for managing much
of the hardware bookkeeping and resource allocation.

It:
+ Determines the capabilities of the underlying hardware during initialization
  based on the resources passed to it via the ZBI.
+ Implements the top level "operational mode" rules.
+ Manages the allocation of SMRG and Context Bank hardware.
+ Manages IRQ allocation and bookkeeping as well as implementing top-level IRQ
  handling policy.
+ Manages "adoption" of HW configuration which was "handed off" to Zircon by the
  bootloader chain (see [Initial Lock-down and Adopted
  Hardware](#initial-lock-down-and-adopted-hardware)).
+ Collects resources and constructs BTIs based on higher level requests.

##### `arm_smmu::SmmuBti`

This is the driver's implementation of the `iommu::Bti` interface.

A BTI (or "Bus Transaction Initiator", a Zircon term) is a kernel object
representing "Some non-AP hardware which is able to post read/write transactions
to the main system bus".  Basically, any HW block which does DMA is a BTI, as
are any non-AP co-processors who are not subject to the main AP MMU when doing
things like fetching instructions and fetching/storing data.

A good mental model for a BTI is that it is the object which manages the
bookkeeping which defines the virtual address space seen by a "device", and a
"device" is something which initiates read/write transactions against that
address space from any one of a collection of Stream IDs which are configured by
user-mode BTI creation requests.

SMMU BTIs are constructed during a call to `CreateBti` on the `arm_smmu::Smmu`
implementation of the `iommu::Iommu` interface.  A single `bus_txn_id` argument
is provided by user-mode and specifies the Stream ID(s) to be assigned to this
BTI instance.  Currently, the parameter carries the mask and value fields used
to program an SMR, with the value packed into bits 0-15 and the mask packed into
bits 16-31.

Any attempt to create a new BTI which shares any Stream IDs with a currently
active BTI will fail.  Each SID must always map to exactly one policy, or none
at all.

Each `arm_smmu::SmmuBti` instance logically owns at least one
`arm_smmu::StreamMatchRegisterGroup`, and exactly one `arm_smmu::ContextBank`
object.  Currently, the only way for a BTI instance to have _more_ than one SMRG
object in its collection would be for it to adopt that configuration from one
handed off from the bootloaders, however in the future it may be necessary to
extend the user-facing APIs with SMMU extensions which allow driver framework
code to add additional SMRG definitions to BTIs after creation as it is
perfectly legal for a system designer to group together multiple stream IDs into
one logical device which cannot be represented using a single SMR value/mask
pair.

The primary user-facing purpose of a BTI object is to manage the "mapping"
portion of a `zx_bti_pin` operation.  Higher level code is responsible for
"pinning" a VMO using a `PinnedVmObject`, which prevents the VMM from swapping
out or relocating any of the physical pages which back the pinned range of the
VMO.  Ownership of the pinned VMO object is then given to the `arm_smmu::SmmuBti`
implementation via the `Map` call.  If the `SmmuBti` instance is not in a
faulted state, it is responsible for setting up any required PTEs and performing
any TLB maintenance needed to locate the `PinnedVmObject` in the device's
virtual address space (with the proper permissions) before transferring
ownership of the `PinnedVmObject` to newly constructed `arm_smmu::SmmuPmt`
instance.

##### `arm_smmu::SmmuPmt`

This is the driver's implementation of the `iommu::Pmt` interface.

The primary job of a "Pinned Memory Token" is to manage the lifecycle of regions
of memory pinned using a `zx_bti_pin` syscall.  Three methods in particular need
to be implemented to allow the dispatcher level to operate properly.

###### `QueryAddress`

Given an offset into the pinned memory, returns the address of the memory in the
_devices virtual address_ space.  When the `SmmuBti` is operating in passthru
mode, this will simply be the physical address of the memory.  When operating in
enforced/translation mode, it will be an actual device vaddr which will be
translated by the MMU hardware to the true physical address during transactions.

The dispatcher and syscall levels of code use `QueryAddress` to build the
scatter-gather map for user-mode during a `zx_bti_pin` operation.  User-mode
does not have any direct access to this method.

###### `ReleasePinnedMemory`

Called in response to a user's call to `zx_pmt_unpin`.  This is the formal
signal to a BTI that a driver is finished accessing the memory which had been
pinned for it.  PMT instances work with their owner BTIs to remove any PTEs
which define the translation from the device virtual address space to the
underlying pinned physical pages, flushing the TLBs in the process to revoke
access.  Then the owned `PinnedVmObject` can be destroyed, unpinning the memory
and allowing it to be actively managed by the VMM once again.

###### `OnDispatcherZeroHandles`

Called by the dispatcher level when the final handle to the dispatcher has been
closed and the dispatcher is about to destroy itself.  Assuming that the
PMT has been formally unpinned already, no special actions are taken.
Otherwise, this PMT has been leaked, likely because a driver has crashed with
memory still in a pinned state and with their hardware potentially still
initiating transactions to access that pinned memory.

In this situation, PMTs work with their owner BTIs to cause the BTI to enter its
fault state, removing *all* access that the BTI's Stream IDs have been granted
and denying requests to pin new memory until the driver has signaled that it has
regained control of its hardware by calling `ReleaseQuarantine` on its BTI
instance (typically after a driver restart).

Note: Strictly speaking, a BTI does not *have* to enter a fault state when a PMT
is leaked if it is operating in fully enforced mode.  Unlike when operating in
passthru mode, where enforcement is an all-addresses-or-no-addresses thing,
access to the _specific_ pages which were leaked _could_ be revoked without
affecting other grants.  This said, a conscious design decision was made to put
the entire BTI into a faulted state and revoke all access (when operating in
passthru mode) anyway.  Drivers need to be portable and ready to run on any
Iommu, whether it is the stub implementation, an "SMMU" implementation running
in an arbitrary mode, or something totally different.  It is important that the
driver consistently follow the explicit pinning and unpinning rules at all
times, and consistently enforce these rules (even it they don't need to be
enforced to ensure security) is seen as an important part of helping them to do
so.

### Locking

The kernel SMMU driver needs to execute operations starting from two primary
locations.

+ In response to calls made from syscall and Dispatcher implementations.  These
  calls are made from thread context and can safely perform operations which
  involve allocating heap memory, fetching and returning pages from/to the PMM,
  looking up pinned physical addresses, and unpinning previously pinned memory.
+ In response to exceptions taken in response to fault IRQs.  No actions taken
  in this context are allowed to perform the memory-related operations described
  above.  Blocking is not allowed, and mutexes cannot be obtained.

Because of the fact that operations need to be able to execute concurrently from
both threaded context as well as IRQ context, spinlocks are used as the building
block to satisfy many of the synchronization requirements, and *any* time that
execution takes place in IRQ context.

#### Smmu Locks

The top level `arm_smmu::Smmu` object contains three locks.  They are:

+ The `InstanceLock`.  A static Mutex class member used to protect the global
  collection of Smmus instances as they are created, and as users attempt to
  create Iommu dispatchers representing specific Smmu instances.  When needed,
  this lock must be held before any other locks.
+ The main lock, or just `lock_`.  Also a mutex, the main Smmu lock needs to be
  held for the duration of most user-facing operations. Binding a specific SMMU
  to a dispatcher instance, creating or destroying BTIs, and allocating
  SMRG/Context-Bank objects to use with BTIs are all operations which require
  holding the main SMMU lock.
+ The `irq_lock`.  A spinlock which is used to protect state IRQ state during
  IRQ dispatch.  See (below)[#the-irq-lock-and-context-bank-interrupts] for more
  details.


##### The IRQ Lock and Context Bank Interrupts

The interrupts owned by the SMMU come in two flavors, global interrupts and
context bank interrupts.  During initialization, the SMMU will register all of
its interrupts with the Zircon interrupt driver (using `registger_int_handler`),
specifying the dispatch target and masking the interrupts in the process.  This
registration lasts for the life of the system, and prevents user-mode from
claiming the interrupts.  The Zircon interrupt drivers serialize calls to
`register_int_handler`, `unregister_int_handler`, and the dispatching of
interrupts using an internal spinlock.  Other interrupt operations (such as
masking and unmasking) are _not protected by this internal spinlock_.

Global interrupts are unmasked at the end of `Smmu::Init` and will be handled
by the top level `Smmu` class.

When a BTI is created, it needs to associate its instance with the specific
interrupt, and unmask the interrupt so that it can fire.  When it is time for
the BTI to shutdown and become destroyed, it needs to remove its association,
mask the interrupt, and synchronize with any interrupt dispatch which may be in
flight.

The `irq_lock` is used synchronize these association/disassociation events
against in-flight IRQ dispatch.  The main `Smmu` instance maintains a set of
`ContextIrqVector` instances protected by the `irq_lock`.  In this structure,
there is a `RefPtr` to the BTI instance which the interrupt targets, as well as
a flag indicating whether or not an interrupt is in flight.

If a BTI's lock needs to be held concurrently with the singleton `irq_lock`, the
BTI lock must be held first.

During association, the lock is held and the BTI records a reference to itself
in the proper `ContextIrqVector`.  The interrupt is then unmasked and the
`irq_lock` is dropped.

During dispatch, `Smmu::HandleContextIrq` will hold the `irq_lock` as it
examines the interrupt's `ContextIrqVector` state.  It will take a local
reference to any associated BTI, mark the interrupt as being in flight and mask
the interrupt before dropping the lock.

Then, if there was an associated BTI instance, it will obtain the BTI instance's
lock and dispatch the interrupt to it.  Once dispatch is complete and the BTI
lock dropped, the IRQ handler will grab the `irq_lock` one last time and record
the interrupt as being no longer in flight before dropping the lock and being
done.

Finally, as a BTI shuts down, it needs to remove its association and guarantee
that there are no interrupts in flight anymore.  It holds the `irq_lock` as it
clears the reference to itself from the `ContextIrqVector` instance.  It also
records whether the interrupt is in flight while it holds the lock.  After
dropping the `irq_lock`, if the vector indicated that there was an interrupt in
flight, the thread performing the shutdown will sleep for a small amount of time
before grabbing the lock to observe whether the interrupt is still in flight or
not.  When we finally observe the interrupt as no longer being in flight, we
have synchronized with any in-flight interrupts, and have successfully removed
the BTI association with the IRQ.

#### BTI Locks

The `arm_smmu::SmmuBti` object currently contains two locks, `lock_` which is a
spinlock, and `pmt_lock_` which is a mutex.  When both locks must be held, the
`pmt_lock_` must be acquired first (also implied by the fact that the pmt lock
is a mutex, while the main lock is a spinlock).

##### The main BTI `lock_`

The main `lock_` must be held for the majority of operations which affect the
underlying hardware state of the BTI.  These include:

+ Assigning a new SMRG.
+ Changing modes, including either entering the "faulted" or "shutdown" states.
+ Exiting a faulted state.
+ Generally, manipulating any of the registers owned by the BTI via its SMRG and
  Context Bank objects.

As noted earlier, if a BTI's lock must be held while also holding the owning
SMMU's `irq_lock`, the BTI's lock must be acquired first.  Currently, this only
needs to happen while:

1) Enabling a context bank interrupt during initialization.
2) Unregistering a context bank interrupt during shutdown.
3) Re-enabling interrupt after handling a context bank fault.

Special care is taken during the shutdown sequence to ensure that there is no
interrupt in flight after the BTI reference held by the IRQ registration has
been cleared, ensuring that the IRQ handler will never be left holding the final
reference to the BTI resulting in an attempt to destroy the object at IRQ time.

##### The main BTI `pmt_lock_`

A BTI's `pmt_lock_` is used:

+ When creating a new PMT and adding it to the active collection.
+ When removing a PMT from the active collection, whether it was leaked or not.
+ When manipulating page table entries when the BTI is operating in translation
  mode, either to create or remove entries in response to the creation,
  destructing, or quarantining of PMTs.
+ To protect the state of PMTs themselves (see below).

#### PMT Locks

The `arm_smmu::SmmuPmt` object currently does not have any lock members.  Instead,
all `SmmuPmt` hold a reference to their owning `SmmuBti` object which is valid
for the entire life of the `SmmuPmt` object, released only when the `SmmuPmt` is
finally destroyed.  `SmmuPmt` state is protected using their owning `SmmuBti`'s
`pmt_lock_`, along with a series of static annotations for operations which
begin from a `SmmuPmt` method, and runtime `AssertHeld` operations whose use is
demanded by the static analyzer for operations starting from a `SmmuBti`'s
methods.

### Resource ownership

The SMMU hardware resources consist mostly of registers which represent a pool
of SMRGs, and a pool of Context Banks.  `arm_smmu::StreamMatchRegGroup` and
`arm_smmu::ContextBank` objects are created to own the specific registers which
are associated with the logical hardware instances, however the registers
themselves are not protected with individual locks at this level as doing so can
easily lead to a very complex locking structure with a great deal of potential
for lock ordering issues leading to deadlock.

Instead, post-initialization, resource ownership is managed logically using an
ownership pattern, but not strictly enforced with a formal unique object
ownership structure in code.  When SMRG/CB registers are not in active use, they
are logically owned by the SMMU instance and are protected by the SMMU's main
lock.

During successful BTI initialization, available SMRG and CB instances are
identified, a BTI object is constructed, and ownership of the SMRG/CB registers
is transferred to the newly created BTI object.  While the BTI exists, the
SMRG/CB registers are owned by the BTI and protected by the BTI's main spinlock,
which must be held any time that registers are to be accessed, including during
context-bank specific IRQ handling.  At the end of the BTI's life, register
ownership is logically transferred back to the SMMU instance making the hardware
available for reuse by a new BTI instance in the future.

Global fault syndrome registers are *always* owned by an SMMU instance, and are
protected by the SMMU's `irq_lock`.

### Initial Lock-down and Adopted Hardware

When the Zircon SMMU driver is operating in either Passthru or Enforced mode,
during initialization it attempts to "lock down" all of the discovered hardware.
It will attempt to set the default global policy to fault transactions, disable
all SMRGs, and configure all context banks to always produce faults.  As the
system boots, user-mode code will create BTIs configured with the proper stream
IDs and delegate these to driver who will use them to pin memory, giving their
hardware access to the physical memory it needs.

That said, sometimes there is hardware which is configured in the early stages
of boot, and which the system designer means to leave in its early statically
configured state.  In other words, some of the SMMU configuration present when
the HLOS takes over is supposed to be simply kept as is.  This is signaled to
the HLOS via device-tree entries which provide a set of SIDs using SMR
value/mask notation.  The device-tree property used to signal this is
`qcom,handoff-smrs`.

Before locking everything down, the zircon driver goes looking for these
"handoff" SID values in the current hardware configuration.  It will proceed to
synthesize BTIs from the hardware state by:

1) Finding all SMRGs which intersect any of the handed-off SIDs.
2) Grouping all of the SMRGs together which share a target context bank into a
   single BTI which owns the set of SMRGs as well as the context bank.
3) Creating an independent BTI with no context bank for any SMRGs which do not
   have any valid context bank configured.

The BTIs created this way are considered to be in the "Adopted" state.  Adopted
BTIs are the only BTIs in the driver who can exist without a context bank, and
only if that was the way they were configured during boot.  Any attempt to
create or configure a BTI with a SID which is currently part of an Adopted BTI
will be rejected.  Adopted BTIs cannot be managed by user-mode code, they simply
maintain the configuration they were given by the early stages of system boot.

### The kernel debug console

A small debug console is implemented by the driver in engineering builds which
have the kernel console enabled.  It can be accessed using the `k smmu` command.
It allows developers to enumerate the discovered SMMUs and the BTIs which
currently exist along with their state. Additionally, it can be used to dump the
low level register state of the SMMUs and provides physical register addresses
so that developers can more easily directly manipulate the register state using
`k pm` commands.  Finally, it can be used to force lockdown of non-adopted BTI
hardware, either at the context bank level or the SMRG level.  Revoking access
at the context bank level while hardware is still attempting to perform accesses
results in CB faults, while force-disabling the SMRs will result in a global
fault due to a sudden inability to match a transaction's Stream ID.

Extreme care should be taken when using the console to perform any modification
to SMMU state using the console.  Do not be surprised if attempts to modify a
register's state do not "stick", either because the SMMU hardware itself refused
to take on the desired state, or because either the hypervisor or secure monitor
intervened.  Additionally, revoking access from adopted hardware, or forcing
global faults via SID invalidation, have been known to trigger faults handled at
levels above non-secure EL1 and sometimes manifest as spontaneous reboots of the
system.

## Future API Enhancements

As of today, the user-mode facing API provided by Zircon is both rather simple,
and very generic.  Its concepts should map well to IOMMU hardware with varying
capabilities, as well as operating transparently in systems with no IOMMU
hardware.  Generally speaking, a driver written to follow the straightforward
pinning and unpinning rules, along with the quarantine protocol, should be
portable across various hardware systems provided that the BTIs they are given
have been constructed properly based on the specific system configuration.

This said, there are a few places where there seem to be functionality gaps
which will probably need to be addressed eventually.  This section is meant to
enumerate them so future developers can be thinking about how to surface the
functionality to user mode eventually.

### BTI Dispatchers should have a "faulted" signal

When the Stub IOMMU was the only IOMMU implementation in the system, the only
way to cause a BTI to enter a faulted state was to leak a PMT (eg, close the
last handle without an explicit unpin).  The quarantine protocol exists in these
systems to help to prevent a driver which crashes with hardware still running
from accidentally corrupting system state, however the situation is usually
considered to be Very Serious and likely to result in a total system reboot in
short order.

SMMU hardware support introduces new ways for BTIs to become faulted.  Now, a
transaction which attempt to read memory not currently mapped for a device, or
one which attempts to access memory which has been mapped but with the wrong
semantics (eg, attempting to write to read-only memory) can also cause a BTI to
enter a faulted state.

The former situation is a synchronous error which software could, in theory, be
aware of, but probably isn't.  The latter is an asynchronous error which
software is likely to be completely unaware of.  This lack of awareness makes
attempts to recover from the error state without rebooting the system very
difficult.

A good solution to this would be to add a generic `FAULTED` signal to BTI
Dispatcher objects which could be waited on by driver framework code.
Additionally, a "last fault" topic could be added to `zx_object_get_info` for
BTI objects which could allow user-mode to fetch details of the fault (the fault
address, the requested access type, etc) when available for logging and
diagnostic purposes.

With these tools in hand, one could imagine the driver framework waiting on its
set of BTIs that it delegated to drivers for a `FAULTED` signal, and taking
appropriate action when one is observed.  This would probably be something like
logging the error and attempting to restart the driver, forcibly if the driver
is not responding.  As time goes on and DF sophistication grows, this might even
result in rolling back to an earlier version of a driver which seems unstable,
or reverting to a generic driver instead of a manufacturer specific driver if
the specific driver seems unstable.

This would be a generic extension of SMMU APIs, applicable to all systems.

### Drivers should have some ability to configure fault behavior for SMMU BTIs.

As noted earlier in this document, when a transaction faults in an SMMU, there
are three main behaviors which can be implemented.

+ The transaction can be stalled, allowing it to be handled (or not) by the SMMU
  driver.  New transactions cannot be processed from this SID until the fault is
  handled.
+ The transaction can be be terminated with RAZ/WI behavior.  No abort will be
  delivered to the initiator, however all reads will read zero and all writes
  will be ignored.
+ The transaction can be terminated, delivering an ABORT to the initiator
  hardware.  Once again, the transaction will experience RAZ/WI behavior, but
  additionally, the initiator will be delivered an explicit abort signal.  What
  happens from here will depend on the initiator and how it responds to an
  ABORT.

Currently, the default behavior of the SMMU driver is to implement no-ABORT
RAZ/WI behavior.  This may not always be the appropriate behavior for some
hardware and systems.

Consider a block device driver which is attempting to load code via DMA into
system memory.  If the target buffer has not been properly pinned, a fault might
occur and the RAZ/WI configuration will (deliberately) leave the target memory
unchanged.  The hardware does not know that this has happened, no fault has
been delivered and we may not have processed a "faulted" signal from a BTI
yet (assuming that they have been implemented).

So, the read appears to have succeeded when it actually didn't, and without
another system to validate the contents of the read, whatever had been sitting
in RAM might end up getting incorrectly used, leading to undefined behavior at
best.

Perhaps a better thing to have done would be to have delivered an ABORT to the
hardware.  If the hardware is sophisticated enough, it might fail the read
command and report this failure to the driver.  Then again, it might ignore the
ABORT completely.

How about a stall?  A stall can guarantee that the transaction does not
complete, independent of hardware behavior, but comes with other complications.
First, SMMUs are not guaranteed to always be capable of stalling transactions,
so the tool might not be available at all.  Second, even if an SMMU can stall a
transaction, there is a finite limit to the number of stalled transactions which
can exist at any point in time.  As a capability limited by resources, it may
not be appropriate to always stall all transactions for all hardware.  Finally,
as with `ABORT`ing a transaction, it is not specified what a block of hardware
will do if its transactions are stalled.  Perhaps it is fine, but perhaps it
locks up the MMIO interface which (in turn) locks up an AP attempting to access
the hardware's registers.

So generally speaking, there are a few different policies which could be
implemented, but which policy *should* be implemented is not something for which
there is always an obvious default.  Control of this policy should be given to
user-mode drivers, likely via a set/get property topic which is specific to SMMU
BITs and would return `NOT_SUPPORTED` for anything else.  This would give the
platform bus level of the driver-stack the ability to configure the proper
behavior for the system on a per-BTI basis before handing the BTIs out to the
driver-user.

This would be an SMMU specific extension to BTIs.

### Drivers should be able to add additional SMRGs to a BTI.

As noted earlier, BTIs in the SMMU drivers are logically an address space which
is used by a collection of Stream IDs.  A BTI with multiple SIDs can be
constructed by user-mode as of today, but only if the value/mask encoding used
by SMRs can represent all of the SIDs needed.

There is no requirement, however, that a system ensures that all of its logical
BTIs can be represented by a single SMR.  Device-tree `iommu` properties for
SMMUs certainly allow multiple SMRs value/mask pairs to be specified.

If, one day, a system with a requirement like this needs to be supported, the
existing API will need to be extended to allow this more complex configuration.
An easy way to do this would once again be to add a simple set property topic
which allows someone to add another SMRG value/mask pair to an existing BTI.

This would be an SMMU specific extension to BTIs.

### Drivers should be able to set transaction property overrides to on a BTI.

When a transaction is placed on the system bus, it can carry other properties
(either implicit or explicit) in addition to things like the address, data,
security state, and stream ID.  These are things like the memory attributes and
cache properties of the transaction.  The SMMU can override these properties
when desired, or it can simply leave them as-is.

Right now, the default behavior is to leave the properties as-is.  Someday,
however, it may be important for a system to be able to override these
properties, for any of a number of reasons.  Adding another property to BTIs is
likely to be the proper way to do this, however the precise shape of this API
will be difficult to get right until a practical requirement shows up.

This would be an SMMU specific extension to BTIs.

### Drivers should be able request a specific location in a BTI's address space to map a pinned VMO.

Currently, when user-mode pins a VMO using a BTI, the SMMU driver is responsible
for determining the address(es) at which the pinned memory is located in the
device's address space.  When translation is not being performed, the SMMU
driver has no control either and simply gives back the physical addresses of the
VMO.  When translation is being performed, it is up to the SMMU driver to pick a
location in the device address space at which the pinned memory will be mapped.

When possible, SMMU drivers should be able to request a specific address for
pinned memory, allowing them to control their own address space if desired,
instead of leaving it up to the kernel SMMU driver.

To accomplish this, information about the device's address space (where it
starts, where it stops, whether the SMMU driver can control the mapping
addresses) will need to be made available to user-mode, potentially through some
sort of get-info topic for a BTI.  Additionally, the syscall used to pin memory
will need to be able to specify a target location when needed, perhaps by
defining `zx_bti_pin_v2` or something similar.

This would be a generic extension of the existing BTI APIs, not something that
is specific only to SMMUs.
