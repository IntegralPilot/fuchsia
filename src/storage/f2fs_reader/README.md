# F2fs reader library

This is a bare bones library for read-only access to F2fs metadata and files.

## Filesystem Architecture

F2fs has two 1kb superblocks at offset 1kb and 2kb. These point at the
checkpoint segment.

There are two checkpoints. These contain the NAT bitmap which is the primary
means by which on-disk consistency is preserved. Checkpoints have a
monotonically increasing version number. The most recent valid checkpoint is
always used.

The "Node Address Table" (NAT) is a mapping from logical (inode/node id) to
physical (block on disk). Again, pairs of segments are used here. The NAT bitmap
(stored in the checkpoint) dictates which segment to read a given entry from for
a given nid (node id). If the bitmap indicates the first copy, updates will be
written to the second copy and the in-memory bitmap will be updated. After that
checkpoint has been written out successfully, the other slot will be used.
This ping-pong behaviour makes consistency guarantees easier to manage.

To avoid excessive flushing of the NAT to disk, there is a small cache of
'dirty' NAT entries stored in RAM. The size of this is bounded based on the
number of entries that will fit in a single block (the so-called "summary block").

F2fs divides devices into two regions -- 'node' and 'data'.
Internally F2fs has 3 write heads for each area named hot, warm and cold.
Each of these write heads works on a segment at a time and calls them "current"
segments. Summary blocks exist as the last block in "current" segments. The NAT
summary block exists only in the "hot node" current segment. Because we are not
writing to the filesystem, we can avoid but otherwise largely ignore the other
"current" segments.

Lastly, a filesystem feature called 'compact summaries' allows for the NAT
and SIT (Segment Information Table) along with per-segment summary information
to be written out in a compact form together in one block, further reducing the
cost of checkpointing. It seems this feature is used by our implementation of
F2fs so support for this has been added.

### Extended attributes

These are stored partially inline and partially in a separate 'nid' to the inode
itself. xattr are 4-byte aligned records consisting of an "index" which is
really a label used to identify the type of xattr (security, verity, user,
encryption, etc), a name and a value.

### "default-key" encryption

The device mapper default-key encryption uses aes256xts where the tweak is the
physical partition block number and assumes a 4k block size.

### File-based encryption

f2fs stores fscrypt encryption contexts in an extended attribute with index 9
and name "c". Critically, this context contains a hash of the main key required
to read the encrypted data and a nonce that must be mixed with the main key to
derive a 64-byte per-file key.

### Filename encryption

When a directory is marked as encrypted, entries in the directory are encrypted
with aes256cts using an fscrypt HKDF that creates a key derived from the
first 32 bytes of the per-file key.

Filenames shorter than 16 bytes are padded to 16 bytes with nulls
before being encrypted. If the filenames are still shorter than 32 bytes (the
minimum required for cypher-text-stealing), then aes256cbc is used instead.

### Symlinks

These are stored as data. For encrypted symlinks, the encryption scheme used is
the same as that used for filename encryption.

### Content encryption

Files are encrypted with aes256xts using the per file key.
The tweak used for content encryption is the logical block address.

### Casefold + direntry hash.

A TEA-based hash is used for direntry hashing most of the time with a SipHash2-4
hash used in the case of casefold + encryption. The latter is seeded using a
HKDF derived from the main key and nonce.

## Testing

A custom, compact f2fs image will be submitted alongside this code as a
preliminary validation. Longer term, we should look at something like
the code at `src/storage/f2fs/test/compatibility/compatibility.h` for
end-to-end interoperability testing. See [this](http://b/399726386).
