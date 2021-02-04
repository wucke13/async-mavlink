use std::collections::HashMap;
use std::time::Duration;

use async_mavlink::prelude::*;
use log::*;
use mavlink::common::*;
use simple_logger::SimpleLogger;
use smol::prelude::*;

fn main() -> Result<(), AsyncMavlinkError> {
    SimpleLogger::new()
        .with_level(log::LevelFilter::Debug)
        .init()
        .unwrap();

    let args: Vec<_> = std::env::args().collect();
    if args.len() < 2 {
        warn!("Usage: (tcpout|tcpin|udpout|udpin|udpbcast|serial|file):(ip|dev|path):(port|baud)");
        return Ok(());
    }

    smol::block_on(async {
        info!("connecting");
        let (conn, future) = AsyncMavConn::new(&args[1], mavlink::MavlinkVersion::V1)
            .map_err(AsyncMavlinkError::from)?;

        // start the event loop
        smol::spawn(async move { future.await }).detach();

        info!("starting heartbeat task");
        smol::spawn({
            let conn = conn.clone();
            async move {
                let mut data = HEARTBEAT_DATA::default();
                data.mavtype = MavType::MAV_TYPE_GCS;
                data.autopilot = MavAutopilot::MAV_AUTOPILOT_INVALID;

                let heartbeat = MavMessage::HEARTBEAT(data);
                loop {
                    conn.send_default(&heartbeat)
                        .await
                        .expect("unable to send heartbeat");
                    smol::Timer::after(Duration::from_secs(1)).await;
                }
            }
        })
        .detach();

        info!("subscribing to PARAM_VALUE");
        let msg_type = MavMessageType::new(&MavMessage::PARAM_VALUE(Default::default()));
        let mut stream = conn.subscribe(msg_type).await?;

        info!("sending request for all parameters");
        let msg_request = MavMessage::PARAM_REQUEST_LIST(Default::default());
        conn.send_default(&msg_request).await.unwrap();
        smol::Timer::after(Duration::from_millis(100)).await;

        let mut parameters = HashMap::new();
        info!("beginning to collect the parameters into a HashMap");
        while let Some(MavMessage::PARAM_VALUE(data)) = (stream.next()).await {
            let name = to_string(&data.param_id);
            trace!(
                "storing parameter {:>4}/{:<4} {:>16}\r",
                parameters.len(),
                data.param_count,
                name
            );
            parameters.insert(name, data.param_value);
            if data.param_count as usize == parameters.len() {
                break;
            }
        }

        // do something with parameters
        info!("\nParameters found:\n{:#?}", parameters);

        Ok(())
    })
}
