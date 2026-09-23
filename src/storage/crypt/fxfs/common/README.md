# fxfs-crypt-common

This library provides shared cryptographic logic for Fxfs, specifically for key
management and wrapping/unwrapping operations.

## Key Components

**`CryptBase`**: A helper for managing wrapping keys and performing
cryptographic operations. It handles the storage of wrapping keys and the
encryption/decryption of data and metadata keys.