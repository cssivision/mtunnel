use std::fs;
use std::io;
use std::net::SocketAddr;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    pub client: Option<Client>,
    pub server: Option<Server>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Client {
    pub local_addr: SocketAddr,
    pub remote_addr: String,
    pub domain_name: String,
    pub ca_certificate: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Server {
    pub local_addr: SocketAddr,
    pub remote_addr: String,
    pub server_cert: String,
    pub server_key: String,
}

impl Server {
    pub fn remote_socket_addrs(&self) -> Vec<SocketAddr> {
        self.remote_addr
            .split(',')
            .map(|v| v.parse())
            .filter(|v| {
                if let Err(e) = v {
                    log::error!("invalid addr: {}", e);
                    false
                } else {
                    true
                }
            })
            .map(|v| v.unwrap())
            .collect()
    }
}

impl Config {
    pub fn new<P: AsRef<Path>>(path: P) -> io::Result<Config> {
        if path.as_ref().exists() {
            let contents = fs::read_to_string(path)?;
            let config = match toml::from_str(&contents) {
                Ok(c) => c,
                Err(e) => {
                    log::error!("{}", e);
                    return Err(io::Error::other(e));
                }
            };

            return Ok(config);
        }
        Err(io::Error::other(format!(
            "{:?} not exist",
            path.as_ref().to_str()
        )))
    }
}
