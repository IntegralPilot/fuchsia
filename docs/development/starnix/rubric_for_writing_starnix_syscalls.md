# Rubric for writing Starnix syscalls

## Processing arguments

Syscalls should process arguments in the order they are provided from userspace.
For example, arguments should be validated in the order they are provided to the
syscall. Validation order is especially important if different the syscall
produces different Errno values for different validation errors because we need
to return the correct Errno value to userspace if there are multiple ways in
which the arguments are invalid.

If the syscall argument needs to read userspace memory (e.g., if the argument
is a `UserAddress` or a `UserRef`), the syscall should read userspace memory
using arguments in the order those arguments are provided. The order in which we
read from userspace memory is visible to userspace because those reads can
generate faults, which are reported to userspace.

## Error handling

Userspace should not be able to use syscalls to crash the Starnix kernel. For
example, syscalls should not use Rust APIs that panic (e.g., `unwrap` or
`expect`) when they encounter invalid or unexpected parameters.

## Flags or options

Many syscalls take a bitfield argument (often a `u32`) that carries the flags
or options for the syscall. A good practice is to validate this bitfield by
checking whether the argument contains an unknown bits before doing more
detailed processing of the argument.

Example:

```
if flags & !(AT_EMPTY_PATH | AT_SYMLINK_NOFOLLOW) != 0 {
    track_stub!(...)
    return error!(EINVAL);
}
```

Use the `track_stub` macro to track the unknown flag. Tracking unsupported
features helps people who are debugging userspace code notice that the issue
they are seeing might be caused by missing functionality in Starnix.

Starnix often uses the `bitflags` macro to define a Starnix-internal type for
bitfields passes as arguments to syscalls. In most cases, syscalls should use
the `from_bits` method to convert the raw syscall argument into the
Starnix-internal type because `from_bits` lets the caller explicitly handle the
case where the raw value contains unknown bits.

## Arithmetic on user-space provided values

Syscalls should avoid using arithmetic on values provided from userspace that
can cause numerical overflow. For example, if a syscall receives two numerical
arguments from userspace, adding those values (without validation) can cause a
numerical overflow if userspace provides excessively large values. In Rust,
numerical overflow causes a panic, which is not the correct way of handling
invalid arguments.

Instead, either validate the range of the numerical values or use a function
like `checked_add`, which lets the calling code handle overflow explicitly.
Consider using the `UserValue` instead of a raw numerical type to make it easier
to validate values from userspace.

## User addresses

When working with userspace addresses, do not use raw numerical values or raw
pointers. Instead, use the `UserAddress` type for addresses. If the address is a
pointer to a specific struct in userspace, use the `UserRef` type, which has the
specific struct as a type parameter. If the address is a pointer to a struct
that has a different layout on different architectures, use the
`MultiArchUserRef` type. The `UserRef` and `MultiArchUserRef` types are also
useful for processing arrays of objects.

Syscalls should not read the same location from userspace memory more than once
because another thread in userspace could modify userspace memory while the
syscall is executing. Instead, copy from userspace into a variable inside the
syscall implementation.

Syscalls that write to userspace memory need to be careful because userspace
might provide addresses that overlap each other. For this reason, syscalls
should not interleave reading and writing userspace memory. Instead, syscalls
should perform all their reads from userspace before performing their writes.
