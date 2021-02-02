use std::collections::HashMap;
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
            .map_err(|e| AsyncMavlinkError::from(e))?;

        println!("starting event loop");
        smol::spawn(async move { future.await }).detach();

        println!("starting heartbeat task");
        smol::spawn({
            let conn = conn.clone();
            async move {
                let heartbeat = MavMessage::HEARTBEAT(HEARTBEAT_DATA::default());
                loop {
                    println!("\x07"); // ring the bell on hearbeat
                    conn.send_default(&heartbeat)
                        .await
                        .expect("unable to send heartbeat");
                    smol::Timer::after(Duration::from_secs(1)).await;
                }
            }
        })
        .detach();

        println!("subscribing to PARAM_VALUE");
        let msg_type = MavMessageType::new(&MavMessage::PARAM_VALUE(Default::default()));
        let mut stream = conn.subscribe(msg_type).await?;

        println!("sending request for all parameters");
        let msg_request = MavMessage::PARAM_REQUEST_LIST(Default::default());
        conn.send_default(&msg_request).await.unwrap();
        smol::Timer::after(Duration::from_millis(100)).await;

        let mut parameters = HashMap::new();
        println!("beginning to collect the parameters into a HashMap");
        while let Some(MavMessage::PARAM_VALUE(data)) = (stream.next()).await {
            let name = to_string(&data.param_id);
            print!(
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
        println!("\nParameters found:\n{:#?}", parameters);

        Ok(())
    })
}
