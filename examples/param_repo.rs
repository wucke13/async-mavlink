use std::time::Duration;

use async_mavlink::{parameter_repo::ParameterRepo, prelude::*};
use futures::future::FutureExt;
use mavlink::common::*;
use simple_logger::SimpleLogger;

fn main() -> Result<(), AsyncMavlinkError> {
    SimpleLogger::new()
        .with_level(log::LevelFilter::Debug)
        .init()
        .unwrap();

    let args: Vec<_> = std::env::args().collect();
    if args.len() < 2 {
        println!(
            "Usage: (tcpout|tcpin|udpout|udpin|udpbcast|serial|file):(ip|dev|path):(port|baud)"
        );
        return Ok(());
    }

    smol::block_on(async {
        println!("connecting");
        let (conn, future) = AsyncMavConn::new(&args[1], mavlink::MavlinkVersion::V1)
            .map_err(AsyncMavlinkError::from)?;

        println!("starting event loop");
        smol::spawn(async move { future.await }).detach();

        println!("starting heartbeat task");
        smol::spawn({
            let conn = conn.clone();
            async move {
                let heartbeat = MavMessage::HEARTBEAT(HEARTBEAT_DATA::default());
                loop {
                    conn.send_default(&heartbeat)
                        .await
                        .expect("unable to send heartbeat");
                    smol::Timer::after(Duration::from_secs(1)).await;
                }
            }
        })
        .detach();

        println!("initializing the parameter repo");
        let timeout_fn = |d| Box::new(smol::Timer::after(d).map(|_| ())) as _;
        let mut repo = ParameterRepo::new(conn, 1, 1, timeout_fn).await?;

        for (k, v) in repo.get_all().await? {
            println!("K: {} = {}", k, v);
        }

        Ok(())
    })
}
