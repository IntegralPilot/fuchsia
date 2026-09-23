# google_root_ssl_certificates

This directory defines the `google_root_ssl_certificates` package, which provides
a scoped TLS trust store containing Google Trust Services (GTS) root CAs.

## Overview

* **Source**: The certificate bundle (`cert.pem`) contains root certificates from
  **Google Trust Services (GTS)**, the public Certificate Authority operated by Google.
* **Synchronization**: Automatically updated on a regular schedule via an automated
  synchronization pipeline. Manual edits to `cert.pem` should not be made directly.
* **Usage**:
  * **Statically Routed Services**: Used by platform services that communicate
    with Google backend endpoints (such as `feedback`, `cobalt`, and `timekeeper`).
  * **Configurable Subsystems**: Selected for the Software Delivery (SWD) stack
    (`omaha-client`, `system-updater`) when products configure
    `software_delivery.trust_store = "restricted"` in product assembly.
* **Verification**:
  * Build-time Scrutiny verification can be configured to assert that target components
    are strictly routed to this store.

For more details on Fuchsia's trust store architecture and product configuration
policy, see [TLS trust stores](/docs/concepts/security/trust_stores.md).



