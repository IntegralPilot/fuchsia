# Linux Security Modules for Starnix

Linux Security Modules (LSM) is a framework supporting Mandatory Access Control (MAC) systems, alongside the more commonly used Discretionary Access Control (DAC) model (user/group/other).

At time of writing, Starnix is hard-wired to an implementation of SELinux.

## The LSM interface

LSM is designed to decouple the kernel from the details of any individual MAC. The `security` directory exposes only:
* Some opaque data types, for Starnix kernel structures to use as "placeholders" for LSM state.
* Hook functions, which are invoked when performing operations to allow the LSM to perform access-checks, or update security state.

While LSM hooks are typically called with borrowed references to Starnix kernel structures (e.g. `Task`, `FsNode`, etc.), the rest of the Starnix kernel should not interface directly with any SELinux-specific data types, methods, etc.

### Security data-types

For each Starnix kernel type that requires associated security state (e.g. `Task`, `FsNode`) there is an opaque placeholder type defined using the Rust "newtype" pattern.  These types need to be fairly compact, since for simplicity they are declared as inline members in kernel structures, and therefore take up space even when no MAC is enabled.

### Hooks

Hook functions generally take the form:
```
security::do_stuff(current_task: &CurrentTask, ...) -> Result<..., Errno>
```

In the LSM layer (`hooks.rs`) the hook functions determine whether SELinux is enabled, and call on to the relevant SELinux-specific hook implementation if so. The LSM hooks are responsible for returning appropriate values when SELinux is not enabled.

## SELinux integration

A subdirectory holds the SELinux-specific versions of each LSM hook, implemented using the primitives provided by Starnix' SELinux library.

### Data-types

The SELinux integration defines data types to hold security state for each kernel structure that requires it. These are wrapped by the LSM layer, to guard against the kernel implementation using them directly.

### Hooks

Hook functions in the SELinux integration have similar signatures to those of the LSM hooks called by the kernel.

These hooks (or the SELinux library with which they integrate) are responsible for ensuring appropriate behaviour when no policy has yet been loaded, for supporting permissive vs. enforcing, and "fake" mode, etc.
