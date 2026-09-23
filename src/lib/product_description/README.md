# Product Description lib

A description of a Fuchsia Product Bundle stored as json.

## Overview

A json file to look up the URL of the transfer manifest for a product bundle.

The URL for a transfer.json is not easy to guess, so some way of looking up
the URL from product attributes is desired.

The description file contains a dictionary of key:value pairs in json which are
used to find a "transfer_url" (which is the value or payload of the file).

E.g. The following would match a search for board=x64 and
product_name=workstation:

```
{
  "schema_version": "1",
  "abi": "0ABC1DEF",
  "board": "x64",
  "platform_version": "0.20221213.2.1",
  "product_name": "workstation",
  "product_version": "2",
  "sdk_version": "10.23434342",
  "transfer_url": "gs://fuchsia-artifacts-release/builds/8794953117502721265/transfer.json"
}
```
