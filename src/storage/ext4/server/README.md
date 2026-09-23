# Ext4 Server

This directory contains the implementation of the Ext4 filesystem server.

It provides two component variants:
1.  **ext4_readonly**: This component reads and exposes an ext4 filesystem with
    full read functionality.
2.  **ext4_server**: This component reads and exposes an ext4 filesystem with
    full read functionality and limited write support.

Write support is limited and persisting changes to metadata is not supported.
Only truncating files to zero and overwriting file contents are supported.
Other operations will fail with a "not supported" error.
