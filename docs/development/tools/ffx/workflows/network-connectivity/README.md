# Network connectivity troubleshoot playbook

This playbook is intended to help diagnose network connectivity issues
between your local Linux host machine and a connected Fuchsia device.

The CDC mechanisms behind `ffx` are explored in depth. Other non-CDC
mechanisms are out of scope in this playbook.

The playbook is split into the following three parts, where each part
represents a category of related issues:

* **Part 1**: [Interface configuration][part-1]
* **Part 2**: [Device discovery][part-2]
* **Part 3**: [SSH daemon][part-3]

When diagnosing network connectivity issues, it’s often one of these
three categories that is impeding connection. The approaches in this
playbook are explored using available Linux command line tools without
going through any `ffx` workflows.

Lastly, many of the command arguments and example outputs (for instance,
interface names, IP addresses, etc.) will be different on your machine.
Be sure to substitute the command arguments with the actual values on
your machine when necessary.

<!-- Reference links -->

[part-1]: interface-configuration.md
[part-2]: device-discovery.md
[part-3]: ssh-daemon.md
