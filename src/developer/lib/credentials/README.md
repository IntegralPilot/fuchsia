# Credentials lib

Store and retrieve user (developer) credentials.

When creating a tool which needs access to the developer's OAuth2 refresh token,
or when storing a new such token, use this lib to perform that operation.

The goal is having all of the Fuchsia developer host tools accessing the same
user credentials.

## Testing

Run unit tests with `fx test host_x64/credentials_lib_test`.
