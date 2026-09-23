# Routing Capabilities

## Background

There are a few different components involved in a Starnix-powered user
experience:

1.  The `container` component, which is the component that is run by
    `starnix_runner`. This component describes the type of Linux environment
    that Starnix is meant to execute, including which system image to use and
    which `init` program to run.
2.  The `starnix_runner` receives the run request for `container` from the
    component framework, and instantiates a new `starnix_kernel`.
3.  The `starnix_kernel` is the component that executes all the Linux code
    described by the `container`.

## Picking a component

In order to decide which component is the best target for your capability,
consider the following:

* Is the capability used in core Starnix functionality?

  If the capability is used in the core of Starnix, in a way where you would
  need to route the capability to virtually every container, then it's often
  best to route the capability to the `starnix_kernel` directly.

* Is it used by a module, or code that is hidden behind a container feature?

  In this case, it's best to route the capability to the container. This
  minimizes the amount of capabilities available to *all* containers.

A final consideration is in regards to the power of the specific capability.

In order to route a capability to the `container` it often needs to be
routed through the session. Routing a capability directly to the
`starnix_kernel` keeps it contained within the platform.
