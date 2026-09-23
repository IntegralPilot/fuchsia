# Additional Inputs for Build-Time Verification

## Pre-signing Policies

These policy files are intended for use by Scrutiny's pre-signing verifier. They enforce platform-level constraints based on build type (userdebug and user), following the strategy from https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0115_build_types.

These checks are run downstream by the signing server, which maintains an equivalent of the policy as a GCL configuration. The GCL is the source of truth for the policies defined here. Updating the policy files or golden files here are not sufficient to prevent a signing failure.

In order to deduplicate the policy, the signing server should run the Scrutiny verifiers at signing time. The effort to enable this is tracked at b/311208337.

Please consult with the security team if any checks need to be updated or removed.

## Pre-signing Golden Files

sshd_config_userdebug - this golden file acts a change detector for third_party/openssh-portable/fuchsia/sshd_config. This is currently how the security team ensures a secure sshd configuration for userdebug builds.

To change this file, a migration strategy is recommended as follows:
1. Notify the security team so back-end changes can be prepared.
2. Add a copy of the updated sshd_config to the pre_signing/ folder in this dir.
3. Update the pre_signing_userdebug.json5 policy file to add an entry for the updated golden in the matches_golden check.
4. Land the CL for steps (2) and (3)
5. Land the sshd_config change to openssh-portable.
6. Remove the previous sshd_config_userdebug golden file.
7. Rename the updated sshd_config to sshd_config_userdebug.
8. Update pre_signing_userdebug.json5 to only use the new sshd_config_userdebug file in the matches_golden check.

## Component Tree Config

This platform-level file exists to teach Scrutiny about dynamic route information so that it can construct a component model representative of the running system. This assumes that enough information can be inferred to represent the set of possible capabilities in this config file, which may or may not be practical depending on what kind of dynamic routing strategy is employed.