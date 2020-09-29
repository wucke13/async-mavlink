use std::collections::HashMap;
use std::io::{Error, ErrorKind, Result};

use async_mavlink::{to_string, AsyncMavConn, MavMessageType};
use mavlink::common::*;
use smol::prelude::*;

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 2 {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            "Usage: (tcpout|tcpin|udpout|udpin|udpbcast|serial|file):(ip|dev|path):(port|baud)",
        ));
    }

    smol::block_on(async {
        println!("connecting");
        let (conn, future) = AsyncMavConn::new(&args[1])?;

        println!("starting event loop");
        smol::spawn(async move { future.await }).detach();

        println!("subscribing to PARAM_VALUE");
        let msg_type = MavMessageType::new(&MavMessage::PARAM_VALUE(Default::default()));
        let mut stream = conn.subscribe(msg_type).await;

        println!("requesting PARAM_VALUE by sending PARAM_REQUEST_LIST");
        let msg_request = MavMessage::PARAM_REQUEST_LIST(Default::default());
        conn.send_default(&msg_request).await?;
        let mut parameters = HashMap::new();

        println!("beginning to collect the parameters into a HashMap");
        while let Some(MavMessage::PARAM_VALUE(data)) = (stream.next()).await {
            let name = to_string(&data.param_id);
            print!("storing parameter {:>16}\r", name);
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
