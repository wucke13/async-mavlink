[![Workflow Status](https://github.com/wucke13/async-mavlink/workflows/main/badge.svg)](https://github.com/wucke13/async-mavlink/actions?query=workflow%3A%22main%22)
[![Percentage of issues still open](https://isitmaintained.com/badge/open/wucke13/async-mavlink.svg)](https://isitmaintained.com/project/wucke13/async-mavlink "Percentage of issues still open")
![Maintenance](https://img.shields.io/badge/maintenance-activly--developed-brightgreen.svg)

# async-mavlink

Async adapter for the [mavlink](https://docs.rs/mavlink/) crate

The mavlink crate offers a low level API for MAVLink connections. This crate adds a thin
async API on top of that. The most important feature is a subscribe based communication model.
It allows the user to subscribe to a certain type of message. All subsequent messages of that
type then can be received in a async stream. In order for this to function, it is necessary to
execute the event loop of the connector as a task. This crate aims to be executor independent,
e.g. it should work with all async executors.

This so far is only a proof of concept. While it might serve your usecase well, expect things
to break. Contributions and suggestions are wellcome!

## Example: Pulling all Parameters from a Vehicle

In this example the subscribe method is utilized to collect all parameters of the vehicle.

```rust
use std::collections::HashMap;
use async_mavlink::{AsyncMavConn, MavMessageType, to_string};
use mavlink::common::*;
use smol::prelude::*;

smol::block_on(async {
    // connect
    let (conn, future) = AsyncMavConn::new("udpin:127.0.0.1:14551")?;

    // start event loop
    smol::spawn(async move { future.await }).detach();

    // we want to subscribe to PARAM_VALUE messages
    let msg_type = MavMessageType::new(&MavMessage::PARAM_VALUE(Default::default()));

    // subscribe to PARAM_VALUE message
    let mut stream = conn.subscribe(msg_type).await;

    // and send one PARAM_REQUEST_LIST message
    let msg_request = MavMessage::PARAM_REQUEST_LIST(Default::default());
    conn.send_default(&msg_request).await?;
    let mut parameters = HashMap::new();

    // receive all parameters and store them in a HashMap
    while let Some(MavMessage::PARAM_VALUE(data)) = (stream.next()).await {
        parameters.insert(to_string(&data.param_id), data.param_value);
        if data.param_count as usize == parameters.len(){
            break;
        }
    }

    // do something with parameters
})
```

License: MIT OR Apache-2.0
