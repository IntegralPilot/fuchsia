# Honeydew

[TOC]

Honeydew is a test framework agnostic device controller written in Python that
provides Host-(Fuchsia)Target interaction.

Supported host operating systems:
* Linux

Assumptions:
* This tool was built to be run locally. Remote workflows (i.e. where the Target
  and Host are not colocated) are in limited support, and have the following
  assumptions:
    * You use a tool like `fssh tunnel` or `funnel` to forward the Target from
      your local machine to the remote machine over a SSH tunnel
    * Only one device is currently supported over the SSH tunnel.
    * If the device reboots during the test, it may be necessary to re-run
      the `fssh tunnel` command manually again in order to re-establish the
      appropriate port forwards.

## Contributing

One of Honeydew's primary goals is to make it easy for anyone working on
host target interactions to contribute.

Honeydew is meant to be the one stop solution for any Host-(Fuchsia)Target
interactions. We can only make this possible when more people contribute to
Honeydew and add more and more interactions that others can also benefit.

### Getting started

* Use a Linux machine for Honeydew development and testing
* Follow [instructions on how to submit contributions to the Fuchsia project](https://fuchsia.dev/fuchsia-src/development/source_code/contribute_changes)
  for the Gerrit developer work flow

### Create a new user affordance

Please refer to the [Affordance](markdowns/affordance.md) doc on instructions
for creating a new affordance in Honeydew code base.
