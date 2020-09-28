[![Workflow Status](https://github.com/wucke13/async-mavlink/workflows/main/badge.svg)](https://github.com/wucke13/async-mavlink/actions?query=workflow%3A%22main%22)

# async-mavlink

Async adapter for the [mavlink](https://docs.rs/mavlink/) lib

The mavlink lib offers a low level API for MAVLink connections. This crate offers a thin async
API on top of that. The main addition is a subscribe based communication model, where the user
can subscribe to a certain type of message. In order for this to function, it is necessary to
execute the even loop of the connector as task. This library however aims to be executor
independent, e.g. it should work with all executors.

This library so far is a proof of concept. While it might serve your usecase well, expect
things to break. Contributions and suggestions are wellcome!

## Example: Pulling all Parameters from a Vehicle

In this example we will utilize the subscribe method to collect all parameters of the vehicle.

```rust
use std::collections::HashMap;
use async_mavlink::{AsyncMavConn, MavMessageType, to_string};
use mavlink::common::*;
use smol::prelude::*;

smol::block_on(async {
#
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
