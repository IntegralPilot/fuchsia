This is a compatibility library to provide a local /dev/urandom implementation.
This can be used in processes that load libraries expecting to read random bytes
from this path. This implementation only supports opening and reading the
contents. It always fills the provided buffer with data from zx_cprng_draw().