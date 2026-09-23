# TLS Trust Stores

## Overview

Fuchsia uses Transport Layer Security (TLS) to encrypt and authenticate network
communications across system services and user applications. To establish trust
during TLS handshakes, the operating system relies on a set of trusted Root
Certificate Authorities (CAs), commonly known as a **TLS trust store**.

To enforce the **principle of least privilege** in transport security, Fuchsia
separates general-purpose public web trust from scoped trust stores (such as the
Google Trust Services root store). Rather than exposing a single system-wide
trust store to all components, Fuchsia isolates certificate access through
component capability routing and product assembly configuration, ensuring that
components are granted access only to the specific root certificates required for
their operation.

## Dual Trust Store Architecture

Fuchsia provides two centrally maintained trust store packages:

```
+-----------------------------------------------------------------------------+
|                          Fuchsia Platform Services                          |
|                                                                             |
|  +-----------------------------+       +---------------------------------+  |
|  |  Google-Specific Services   |       |  Web & Third-Party Runtimes     |  |
|  |   (Feedback, Cobalt,        |       |  (WebEngine, HTTP Clients,      |  |
|  |    Timekeeper)              |       |   User Applications)            |  |
|  +--------------+--------------+       +----------------+----------------+  |
|                 |                                       |                   |
|                 |    +-----------------------------+    |                   |
|                 |    |   Configurable Subsystems   |    |                   |
|                 |    |      (Software Delivery)    |    |                   |
|                 |    +--------------+--------------+    |                   |
|                 |                   | (via Assembly)    |                   |
|                 v                   v                   v                   |
|  +-----------------------------+       +---------------------------------+  |
|  | google_root_ssl_certificates|       |     root_ssl_certificates       |  |
|  |   (Google Trust Services)   |       |      (Chrome Root Store)        |  |
|  +-----------------------------+       +---------------------------------+  |
+-----------------------------------------------------------------------------+
```

### 1. Public Trust Store (`root_ssl_certificates`)

* **Contents**: The **Chrome Root Store** certificate bundle.
* **Purpose**: General outbound HTTPS traffic, web browsers, and third-party
  services communicating with arbitrary endpoints across the public internet.
* **Routing**: Exposed to components through the `root-ssl-certificates` directory
  capability, typically mounted into a component's namespace at `/config/ssl`.

### 2. Google Trust Services (GTS) Trust Store (`google_root_ssl_certificates`)

* **Contents**: Root certificates from **Google Trust Services (GTS)**, the public
  Certificate Authority hierarchy operated by Google.
* **Purpose**: Services communicating with Google-hosted endpoints (such as
  telemetry, crash reporting, network time, and software delivery).
* **Security Benefits**: Restricting trust strictly to the authorities relevant
  to known backend endpoints provides defense-in-depth:
  * **Attack Surface Minimization**: Limits the set of trusted root authorities
    strictly to those needed for expected endpoints, significantly reducing the
    overall trust surface.
  * **Blast Radius Reduction**: Protects critical platform infrastructure from
    unintended trust delegation, certificate misconfiguration, or trust churn in
    the broader Web PKI.
  * **Scoped Trust Alignment**: Enforces the principle of least privilege by
    matching trust anchors directly to expected backend communication channels.



## Trust Store Assignment & Routing

Components receive trust store access according to their networking requirements:

### 1. Statically Routed Platform Services

Services that communicate exclusively with first-party Google backend endpoints
are statically routed to `google_root_ssl_certificates` in component topology:

* **Feedback & Forensics (`feedback`)**: Uploads crash reports and diagnostic
  snapshots strictly to Google backend services.
* **Cobalt Metrics (`cobalt`)**: Transmits privacy-preserving system telemetry
  to Google metrics pipelines.
* **Timekeeper (`timekeeper`)**: Validates TLS certificates during `httpsdate`
  network time synchronization against Google time servers.

### 2. Configurable Subsystems (Software Delivery)

Subsystems that serve both general open-source products and restricted first-party
products are configured at product assembly time. Currently, the **Software
Delivery (SWD)** stack (`omaha-client`, `system-updater`) is configurable via
assembly:

```json5
platform: {
  software_delivery: {
    trust_store: "restricted", // Options: "restricted" | "public"
  },
}
```

* **`"restricted"`**: Used by first-party products to ensure that update checks and
  package downloads authenticate strictly against Google infrastructure.
* **`"public"` (default)**: Used by generic open-source or custom products
  downloading packages from general public repositories.

During assembly, the platform dynamically includes the matching Platform Assembly
Input Bundle (AIB) (`swd_trust_store_restricted` or `swd_trust_store_public`) and
routes the selected certificate directory capability (`swd-root-ssl-certificates`)
from `#core` to the update stack.

## Verification & Architectural Guarantees

Fuchsia uses platform-level tooling to enforce trust store boundaries:

* **Assembly Build-Time Validation**: The platform assembly tool validates
  the product configuration during image construction, preventing invalid or
  conflicting trust store selections.
* **Scrutiny Static Analysis**: The Scrutiny framework provides build-time static
  verification of component topology and capability routing. Products can define
  routing policies to guarantee that security-critical components are only routed
  to approved trust stores.

## Platform Policy: Product-Supplied Trust Stores

Fuchsia as a platform **does not support products providing or injecting their own
arbitrary TLS trust stores** into core platform services.

### Rationale

* **Preserving Platform Security Invariants**:
  Core platform services (such as system update verification, crash reporting, and
  hardware attestation) represent fundamental security boundaries. Allowing
  unvetted or arbitrary root certificates to be injected into platform services
  would create a mechanism to bypass platform security controls and compromise
  system integrity.

* **Audited Platform Baselines**:
  The platform maintains and audits supported trust store profiles centrally.
  Products configure their behavior by selecting among audited platform profiles
  (such as `restricted` vs `public`) rather than introducing unverified root sets.

* **Application vs. Platform Boundary**:
  Applications, user-space runtimes, or guest environments (such as web runtimes or
  container runtimes) that require custom or private enterprise PKI must manage
  those certificates within their own isolated sandboxes. Custom application trust
  must not be mixed with or propagated to platform-level system services.
