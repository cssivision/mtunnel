# mtunnel 
[![Build](https://github.com/cssivision/mtunnel/workflows/build/badge.svg)](
https://github.com/cssivision/mtunnel/actions)
[![crate](https://img.shields.io/crates/v/mtunnel.svg)](https://crates.io/crates/mtunnel)
[![License](http://img.shields.io/badge/license-mit-blue.svg)](https://github.com/cssivision/mtunnel/blob/master/LICENSE)

A tcp over http2 + tls proxy.

# Usage 
### get certificates, following [steps](https://github.com/cssivision/mtunnel/tree/main/tls_config).

### make your config
client config:
```json
{
    "local_addr": "127.0.0.1:8080",
    "remote_addr": "127.0.0.1:8081",
    "domain_name": "mydomain.com",
    "ca_certificate": "./tls_config/rootCA.crt"
}
```

server config:
```json
{
    "local_addr": "127.0.0.1:8081",
    "remote_addr": "127.0.0.1:8082",
    "server_cert": "./tls_config/mydomain.com.crt",
    "server_key": "./tls_config/mydomain.com.key"
}
```

### start client and server
```sh
# start the server
RUST_LOG=debug cargo run --bin mtunnel-server -- -c mtunnel-server.json 
# start the client
RUST_LOG=debug cargo run --bin mtunnel-client -- -c mtunnel-client.json 
```

# Licenses

All source code is licensed under the [MIT License](https://github.com/cssivision/mtunnel/blob/master/LICENSE).
