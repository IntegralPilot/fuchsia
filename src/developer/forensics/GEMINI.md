# Forensics

This directory contains source code for the feedback.cm and exceptions.cm components.

## Testing

When a change is ready:
- You MUST run `fx test //src/developer/forensics` to verify that there are no unintended breakages.
- If the contents of a Fuchsia snapshot are expected to change because of your modifications, you
  MUST include the `verify-snapshot` skill in your Verification Plan and use it during the
  execution phase to verify that the contents of the snapshot match the code changes being made.
