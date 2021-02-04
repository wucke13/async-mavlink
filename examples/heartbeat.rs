use std::time::Duration;

use async_mavlink::prelude::*;
use mavlink::common::*;
use smol::prelude::*;

fn main() -> Result<(), AsyncMavlinkError> {
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
                let mut data = HEARTBEAT_DATA::default();
                data.mavtype = MavType::MAV_TYPE_GCS;
                data.autopilot = MavAutopilot::MAV_AUTOPILOT_INVALID;

                let heartbeat = MavMessage::HEARTBEAT(data);
                loop {
                    println!("Heartbeat sent"); // ring the bell on hearbeat
                    conn.send_default(&heartbeat)
                        .await
                        .expect("unable to send heartbeat");
                    smol::Timer::after(Duration::from_secs(1)).await;
                }
            }
        })
        .detach();

        let msg_type = MavMessageType::new(&MavMessage::HEARTBEAT(Default::default()));
        let mut stream = conn.subscribe(msg_type).await?;

        while let Some(MavMessage::HEARTBEAT(data)) = (stream.next()).await {
            println!("Heatbeat received:\n{:?}", data);
        }

        Ok(())
    })
}
