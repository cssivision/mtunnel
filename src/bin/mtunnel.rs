use std::future::pending;
use std::io;

use mtunnel::args::parse_args;
use mtunnel::client;
use mtunnel::server;

fn main() -> io::Result<()> {
    env_logger::init();

    awak::block_on(async {
        let cfg = parse_args("mtunnel").expect("invalid config");
        log::info!("config: \n{}", toml::ser::to_string_pretty(&cfg).unwrap());

        if let Some(cfg) = cfg.client {
            awak::spawn(async {
                if let Err(err) = client::run(cfg).await {
                    log::error!("client fail: {:?}", err);
                }
            })
            .detach();
        }
        if let Some(cfg) = cfg.server {
            awak::spawn(async {
                if let Err(err) = server::run(cfg).await {
                    log::error!("client fail: {:?}", err);
                }
            })
            .detach();
        }
        let () = pending().await;
        Ok(())
    })
}
