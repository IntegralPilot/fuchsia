# root_ssl_certificates

This directory defines the `root_ssl_certificates` package, which provides
Fuchsia's general-purpose public TLS trust store.

## Overview

* **Source**: The certificate bundle (`cert.pem`) represents the **Chrome Root Store**.
* **Synchronization**: Automatically updated on a regular schedule via an automated
  synchronization pipeline. Manual edits to `cert.pem` should not be made directly.
* **Usage**:
  * Offered to components via the `root-ssl-certificates` directory capability (mounted at `/config/ssl`).
  * Used by web engines, general HTTP clients, and non-Google network services.
* **Alternative (Scoped Trust)**: Services communicating specifically with Google
  backends (such as `feedback`, `cobalt`, `timekeeper`, and SWD when configured with
  `software_delivery.trust_store = "restricted"`) use
  [google_root_ssl_certificates](//src/security/bin/google_root_ssl_certificates)
  to scope trust exclusively to Google Trust Services roots.

For more details on Fuchsia's trust store architecture and product configuration
policy, see [TLS trust stores](/docs/concepts/security/trust_stores.md).




