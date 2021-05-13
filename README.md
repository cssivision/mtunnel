# mtunnel 
[![Build](https://github.com/cssivision/mtunnel/workflows/build/badge.svg)](
https://github.com/cssivision/mtunnel/actions)
[![crate](https://img.shields.io/crates/v/mtunnel.svg)](https://crates.io/crates/mtunnel)
[![License](http://img.shields.io/badge/license-mit-blue.svg)](https://github.com/cssivision/mtunnel/blob/master/LICENSE)

A tcp over http2 + tls proxy.

# Usage 
```sh
# start the server
RUST_LOG=debug cargo run --bin mtunnel-server -- -c mtunnel-server.json 
# start the client
RUST_LOG=debug cargo run --bin mtunnel-client -- -c mtunnel-client.json 
```

# Licenses

All source code is licensed under the [MIT License](https://github.com/cssivision/mtunnel/blob/master/LICENSE).
