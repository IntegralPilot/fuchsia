# Suspend States (from Zircon's perspective)

From Zircon's perspective, there are different kinds of "suspend": system
suspend, task scheduling suspend (per-CPU), and platform-specific suspend
(per-CPU), and CPU Idle.

## System Suspend State

This is a system-wide state that is entered when a user program calls
[`zx_system_suspend_enter()`].  The defining characteristic is that while in
this state, task execution is suspended for all CPUs.  That is, while in this
state, no user thread will execute.  However, Zircon's interrupt handlers as
well each CPU's special "idle power thread" (part of Zircon) *may* continue to
execute as necessary.  More on this below.

When entering this state Zircon identifies which interrupts are designated as
wake vectors (`ZX_INTERRUPT_WAKE_VECTOR`) and ensures that only those interrupts
will bring the system out of this state.

This state builds on Task Scheduling Suspend State (see below).  In order for
the system to enter this state, every CPU must enter the Task Scheduling Suspend
State.

## Task Scheduling Suspend State

This is a per-CPU state.  When a CPU is in this state, its special "idle power
thread" is the only task that may execute.  This task is responsible for
managing state transitions, waking the system when necessary, and putting the
CPU into a Platform-Specific Suspend State or none is available, a CPU Idle
State.

## Platform-Specific Suspend State

This is a per-CPU state that's defined by the platform layer (think "PC" or
"generic ARM").  Currently, this state is only supported on the arm64
architecture and only for QEMU and Sorrel.

On Sorrel, this state is implemented via PSCI CPU_SUSPEND (see below).

When entering a Platform-Specific Suspend State, Zircon makes no effort to mask
off or otherwise disable hardware interrupts (think SPIs).  If, prior to
entering this state, a particular device had been configured to generate an
interrupt on some event *and* that event occurs while in this state, the
interrupt will be generated and the CPU will leave this state.

The CPU's handler will then execute and determine whether the interrupt is a
wake vector or not.  If the interrupt *is* a wake vector, then the CPU's idle
power thread will begin to transition all CPUs out of their Task Scheduling
Suspend State and the system out of System Suspend.  If the interrupt is *not* a
wake vector, then the idle power thread will immediately re-enter the
Platform-Specific Suspend State.

### PSCI CPU_SUSPEND

This is a state that a CPU enters by issuing a `CPU_SUSPEND` PSCI call.  On
Sorrel, this call comes in two flavors.  One flavor is for putting just the
calling CPU into a suspend state.  The other flavor is for putting the last
non-suspended CPU into suspend and powering down the AP complex.  That is,
entering AP Power Collapse.

When in this state, interrupting the CPU will cause the CPU to resume execution
(i.e. leave this state).

## CPU Idle

This isn't really a suspend state.  A CPU enters this state whenever it has
nothing better to do.  The CPU will pause execution, enter a lower power state
and wait until an interrupt occurs or another CPU has asked it to resume
execution.  On arm64 and riscv64, this is done using a `WFI` instruction.  On
x64, we use either `MWAIT` or `HLT`.  While a CPU may enter this state during
any of the above suspend states, this state is not inherently tied to System
Suspend, or Task Scheduling Suspend.

[`zx_system_suspend_enter()`]: /reference/syscalls/system_suspend_enter.md
