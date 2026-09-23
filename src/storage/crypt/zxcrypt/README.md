# zxcrypt-crypt

This module contains a local implementation of the Crypt service, running as an in-process task.
It is coupled to the logic in the FVM component (//src/storage/fvm), which uses the Crypt protocol
but expects keys to be formatted in a particular way to pass zxcrypt metadata.
