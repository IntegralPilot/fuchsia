# Fuchsia Security: Binaries
## Overview
This directory contains all source code that results in a component, package
or other binary that is intended to be included in some assembled version
of Fuchsia.

- Host only tools should instead be placed in [//src/security/tools](//src/security/tools)
- Integration tests should be placed in [//src/security/tests](//src/security/tests)

## Project Descriptions
* [root\_ssl\_certificates](//src/security/bin/root_ssl_certificates): Public Chrome
  Root Store CA certificates. Serves as the general-purpose TLS trust store for
  outbound web traffic and third-party services.
* [google\_root\_ssl\_certificates](//src/security/bin/google_root_ssl_certificates):
  Google Trust Services (GTS) root CA certificates. Restricts trust to the
  publicly operated Google Trust Services authorities used by services connecting
  to Google backends (Feedback, Cobalt, Timekeeper, and SWD in restricted mode).


* [kms](//src/security/bin/kms): Key Management Service for hardware-backed key
  storage and cryptographic operations.
* [tee\_manager](//src/security/bin/tee_manager): Fuchsia - TEE communication
  stack. Marshals trusted application invocations; handles secure storage RPCs.
* [syscall-check](//src/security/bin/syscall-check): Diagnostic utility to check
  whether specific security-sensitive system calls are enabled or disabled.

