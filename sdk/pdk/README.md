# Product Development Kit

The Product Development Kit (PDK) is a sibling to the Fuchsia Software
Development Kit (SDK). Whereas the SDK's primary audience is _component
developers_, the PDK's primary audience is _product owners_.

The SDK includes everything a developer needs to write Fuchsia components like
applications or drivers. This includes the tools necessary to assemble Fuchsia
product images[^1], but also includes code and static libraries that are not
useful for product owners (unless those product owners are also component
developers).

The PDK is a lighter-weight alternative for those product owners.

[^1]: Well, in theory. Some artifacts, like Assembly Input Bundles (AIBs) and
    companion images are distributed via separate channels for technical and
    historical reasons. This is also true with the PDK. But _conceptually_,
    these artifacts are part of the SDK/PDK, even if they're not literally
    distributed in the same archive.

## Policy

The PDK is a subset of the SDK. Therefore, any policies that apply to inclusion
in the SDK also apply to inclusion in the PDK.

SDK atoms should be included in the PDK if PDK customers need them.

## Support

As with the SDK, support and compatibility guarantees are only extended to our
partners. Do not depend on it without approval from Fuchsia SDK leads.

## Future work

Eventually, we may well want to enable SDK consumers to obtain parts of the SDK
à la carte (for example, only downloading static libraries for certain
architectures or API levels). In that world, a separate PDK is potentially less
valuable, and we may choose to absorb it back into the SDK.
