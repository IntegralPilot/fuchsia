# crypt

This module contains implementations of the Crypt service, which manages wrapping and unwrapping
cryptographic keys for Fxfs and FVM instances.

Generally, one crypt instance will be running per unlocked volume in Fxfs or FVM.  A handle to this
crypt instance will be passed as part of unlocking the volume.  The creator of the crypt instance
can use the CryptManagement protocol to control the state of the crypt service (adding new keys,
switching active keys, and removing old keys).

The algorithm used for key wrapping is [AES-GCM-SIV](https://en.wikipedia.org/wiki/AES-GCM-SIV).
