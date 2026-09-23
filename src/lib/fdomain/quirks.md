FDomain Handle Quirks
=====================

This file catalogues some of the unusual behaviors you might see if you use
handles through the FDomain client and are expecting the behaviors one would see
using handles directly.

* If you drop futures from socket or channel reads, read calls at the client
  will no longer correlate to actual read calls on the handle. E.g. :

  ```
  let _dropped_fut = socket.read(32);
  let data = socket.read(5).await;
  ```

  The first call may actually cause 32-bytes to be read from the socket, despite
  the future never being polled, in which case the second call will return the
  first 5 bytes from *that* read while never issuing a 5-byte read of its own.
  Subsequent calls will return more bytes from the original 32-byte read without
  initiating a new read to the handle itself until all 32 bytes of the original
  read are consumed.

* Sockets must have the INSPECT right to exist within an FDomain. This is so
  FDomain can know whether a socket is a datagram or stream socket, which is
  necessary for correct semantics.
