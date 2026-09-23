# TEE Client API

This library provides a communications API for connecting Client Applications
running in a rich operating environment (Fuchsia) with security related Trusted
Applications running inside a Trusted Execution Environment (TEE).

For details about the API, please refer to the [GlobalPlatform TEE Client API
Specification V1.0](https://globalplatform.org/specs-library/tee-client-api-specification/).

# About this fork

This is a fork of the tee-client-api at //src/security/lib/tee/tee-client-api.
This fork exists to enable client-TA communication within a Fuchsia environment by utilizing
the path exposed by the TA manager.

The original client API connects to /svc/fuchsia.tee.Application.(uuid).

This fork connects to /svc/ta/(uuid)/fuchsia.tee.Application.

The original API is backed by a TEE driver which forwards communication to a secure world TEE.
The fork connects directly to a TA served within the Fuchsia environment by the TA manager.
These implementations can be merged, but will require some careful design to ensure that both
pathways can be cleanly supported.