# Audience

This document is aimed at someone who wants to start using power framework and
has little or no prior exposure. It may be suitable as a reference for people
with prior exposure to power framework.


## Goals of power framework

Why have a power framework? The goal is to save power. To achieve this goal,
power framework provides mechanisms for coordination and debuggability related
to power. By coordination we mean a mechanism for components in Fuchsia to order
their actions so they can properly (ie. power efficiently) operate the system.
By debuggability we mean the ability to understand the state of the system with
respect to power, how that system changes over time, and what is causing the
state changes.

The goal of the power framework is not to save power via some clever mechanism
of scheduling or resource management. Rather it manages the resources as
directed and gives the system's developers the tools needed to understand and
optimize power usage.


## Coordination

Accomplishing certain work in the system requires the availability of certain
resources. These resources should remain available until the work completes.
Power framework provides a standardized coordination mechanism to manage
resources.

For example, consider a program that wants to download a file. Let us say that
the program requires two things to download the file: the CPU and the network
protocol stack. The network protocol stack in turn requires the CPU and the
network driver.

If the application, netstack, CPU manager, and network driver integrate with
power framework, then power framework can coordinate telling the owners of
resources when the resources should or should not be available. In our example,
this means that the application can request the system be able to download
something. Power framework will then send messages to resource owners in the
proper order to prepare the system, sending messages to the owner of various
power elements, first the CPU then the network driver then network protocol
stack to prepare and finally to the application that the system is ready. Once
the download is complete the application can withdraw its request that the
system be able to download something and power framework will coordinate turning
down unused resources.


## Debuggability

The power framework, as the coordinator of system state changes, has a huge
amount of information about the state of the system. It makes this information
available about what state a resource is told to be in and state of the resource
reported by its owner through things like Inspect. We can then build tools to
analyze this treasure trove, understand the system, and improve its function.
