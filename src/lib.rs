//! Async adapter for the [mavlink](https://docs.rs/mavlink/) crate
//!
//! The mavlink crate offers a low level API for MAVLink connections. This crate adds a thin
//! async API on top of that. The most important feature is a subscribe based communication model.
//! It allows the user to subscribe to a certain type of message. All subsequent messages of that
//! type then can be received in a async stream. In order for this to function, it is necessary to
//! execute the event loop of the connector as a task. This crate aims to be executor independent,
//! e.g. it should work with all async executors.
//!
//! This so far is only a proof of concept. While it might serve your usecase well, expect things
//! to break. Contributions and suggestions are wellcome!
//!
//! # Example: Pulling all Parameters from a Vehicle
//!
//! In this example the subscribe method is utilized to collect all parameters of the vehicle.
//!
//! ```
//! use std::collections::HashMap;
//! use async_mavlink::prelude::*;
//! use mavlink::{MavlinkVersion, common::*};
//! use smol::prelude::*;
//!
//! # fn main()->Result<(),AsyncMavlinkError>{
//! smol::block_on(async {
//! #   smol::spawn(async {
//! #       let mut conn = mavlink::connect("udpout:127.0.0.1:14551").unwrap();
//! #       conn.set_protocol_version(mavlink::MavlinkVersion::V1);
//! #       loop {
//! #           let mut value = PARAM_VALUE_DATA::default();
//! #           value.param_id = to_char_arr("param_1");
//! #           value.param_value = 13.0;
//! #           value.param_count = 1;
//! #           conn.send_default(&MavMessage::PARAM_VALUE(value));
//! #           smol::Timer::after(std::time::Duration::from_millis(10)).await;
//! #       }
//! #   }).detach();
//!     // connect
//!     let (conn, future) = AsyncMavConn::new("udpin:127.0.0.1:14551", MavlinkVersion::V1)?;
//!     
//!     // start event loop
//!     smol::spawn(async move { future.await }).detach();
//!
//!     // we want to subscribe to PARAM_VALUE messages
//!     let msg_type = MavMessageType::new(&MavMessage::PARAM_VALUE(Default::default()));
//!
//!     // subscribe to PARAM_VALUE message
//!     let mut stream = conn.subscribe(msg_type).await?;
//!
//!     // and send one PARAM_REQUEST_LIST message
//!     let msg_request = MavMessage::PARAM_REQUEST_LIST(Default::default());
//!     conn.send_default(&msg_request).await?;
//!     let mut parameters = HashMap::new();
//!
//!     // receive all parameters and store them in a HashMap
//!     while let Some(MavMessage::PARAM_VALUE(data)) = (stream.next()).await {
//!         parameters.insert(to_string(&data.param_id), data.param_value);
//!         if data.param_count as usize == parameters.len(){
//!             break;
//!         }
//!     }
//!
//!     // do something with parameters
//! # Ok(())    
//! })
//! # }
//! ```
#![deny(missing_docs)]
#![deny(unsafe_code)]

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use arc_swap::ArcSwapOption;
use blocking::Unblock;
use futures::{
    channel::{
        mpsc::{self, UnboundedSender},
        oneshot,
    },
    future::Either,
    prelude::*,
    StreamExt,
};

use mavlink::{MavHeader, Message};

pub mod prelude;
mod types;
mod util;

pub use types::{AsyncMavlinkError, MavMessageType};
pub use util::{to_char_arr, to_string};

/// An async adapter for a MAVLink connection
///
/// Offers high level functionality to interact with a MAVLink vehicle in an async fashion.
pub struct AsyncMavConn<M: mavlink::Message> {
    task_dispatcher: UnboundedSender<Task<M>>,
    last_heartbeat: Arc<ArcSwapOption<Instant>>,
}

enum Task<M: mavlink::Message> {
    Emit {
        header: MavHeader,
        message: M,
        backchannel: oneshot::Sender<Result<(), AsyncMavlinkError>>,
    },
    Subscribe {
        message_type: MavMessageType<M>,
        backchannel: UnboundedSender<M>,
    },
}

// TODO make this failable if no heartbeat is received
impl<M: 'static + mavlink::Message + Clone + Send + Sync> AsyncMavConn<M> {
    /// Construct a new MavlinkConnectionHandler
    ///
    /// # Arguments
    ///
    /// * `address` - MAVLink connection `&str`. Equivalent to the `address` argument in
    /// [mavlink::connect](https://docs.rs/mavlink/*/mavlink/fn.connect.html)
    ///
    /// # Examples
    ///
    /// ```
    /// use async_mavlink::prelude::*;
    /// use mavlink::{MavlinkVersion, common::MavMessage};
    /// use smol::prelude::*;
    ///
    /// # fn main() -> Result<(),AsyncMavlinkError> {
    /// smol::block_on(async {
    ///     let (conn, future) = AsyncMavConn::new("udpbcast:127.0.0.2:14551", MavlinkVersion::V1)?;
    ///     smol::spawn(async move { future.await }).detach();
    ///     // ...
    /// #   conn.send_default(&MavMessage::HEARTBEAT(Default::default())).await?;
    /// # Ok(())
    /// })
    /// # }
    /// ```
    pub fn new(
        address: &str,
        mavlink_version: mavlink::MavlinkVersion,
    ) -> Result<(Arc<Self>, impl Future<Output = impl Send> + Send), AsyncMavlinkError> {
        let mut conn = mavlink::connect::<M>(address)?;
        conn.set_protocol_version(mavlink_version);
        let (task_dispatcher, incoming_tasks) = mpsc::unbounded();
        let last_heartbeat = Arc::new(ArcSwapOption::from(None));

        let f = {
            let last_heartbeat = last_heartbeat.clone();
            async move {
                let mut subscriptions: HashMap<_, Vec<UnboundedSender<M>>> = HashMap::new();
                let conn = Arc::new(conn);

                let messages_iter = std::iter::repeat_with({
                    let conn = conn.clone();
                    move || conn.recv()
                });
                let messages = Unblock::new(messages_iter).map(Either::Right);
                let operations = incoming_tasks.map(Either::Left);

                let mut combined = stream::select(operations, messages);

                let heartbeat_id =
                    mavlink::common::MavMessage::HEARTBEAT(Default::default()).message_id();

                // Event loop
                //
                // In here we process everything. By making this not concurrent itself, we can
                // afford not to use locks at all!
                loop {
                    // TODO Evaluate whether this unwrap is fallible
                    match combined.next().await.unwrap() {
                        // A new subscription
                        Either::Left(Task::Subscribe {
                            message_type,
                            backchannel,
                        }) => {
                            // find existing subscriptions for this message
                            let message_subs = subscriptions
                                .entry(message_type)
                                .or_insert_with(|| Vec::with_capacity(1));
                            // and add this channel to it.
                            message_subs.push(backchannel);
                        }
                        // A new message to be emitted
                        Either::Left(Task::Emit {
                            header,
                            message,
                            backchannel,
                        }) => {
                            // Send the message and send back any error which occurred on sending
                            let result = conn.send(&header, &message).map_err(|e| e.into());
                            let _ = backchannel.send(result); // TODO handle this
                        }
                        Either::Right(Ok((_header, msg))) => {
                            if msg.message_id() == heartbeat_id {
                                last_heartbeat.rcu(|_| Some(Arc::new(Instant::now())));
                            }
                            subscriptions
                                .entry(MavMessageType::new(&msg))
                                .or_insert_with(Vec::new)
                                .retain(|mut backchannel| match backchannel.is_closed() {
                                    true => false,
                                    false => {
                                        futures::executor::block_on(backchannel.send(msg.clone()))
                                            .expect("unable to do this");
                                        true
                                    }
                                });
                        }
                        _ => {}
                    }
                }
            }
        };

        Ok((
            Arc::new(Self {
                task_dispatcher,
                last_heartbeat,
            }),
            Box::pin(f),
        ))
    }

    /// Subscribe to all new MavMessages of the given MavMessageType
    ///
    /// This returns a never-ending Stream of MavMessages.
    ///
    /// # Arguments
    ///
    /// * `message_type` - `MavMessageType` of the desired messages
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::time::Duration;
    /// # use futures::prelude::*;
    /// # use async_mavlink::prelude::*;
    /// # use mavlink::{MavlinkVersion, common::MavMessage};
    /// # fn main() -> Result<(),AsyncMavlinkError> {
    /// # smol::block_on(async {
    /// # let (conn, future) = AsyncMavConn::new("udpin:127.0.0.3:14551", MavlinkVersion::V1)?;
    /// # smol::spawn(async move { future.await; }).detach();
    /// # smol::spawn(async {
    /// #   let mut conn = mavlink::connect("udpout:127.0.0.3:14551").unwrap();
    /// #   conn.set_protocol_version(mavlink::MavlinkVersion::V1);
    /// #   loop {
    /// #       conn.send_default(&MavMessage::PARAM_VALUE(Default::default()));
    /// #       smol::Timer::after(Duration::from_millis(10)).await;
    /// #   }
    /// # }).detach();
    /// let message_type = MavMessageType::new(&MavMessage::PARAM_VALUE(Default::default()));
    /// let mut stream = conn.subscribe(message_type).await?;
    /// while let Some(MavMessage::PARAM_VALUE(data)) = (stream.next()).await {
    ///     // do something with `data`
    /// #   break;
    /// }
    /// # Ok(()) })}
    /// ```
    pub async fn subscribe(
        &self,
        message_type: MavMessageType<M>,
    ) -> Result<Pin<Box<dyn Stream<Item = M>>>, AsyncMavlinkError> {
        let (backchannel, rx) = mpsc::unbounded();

        self.task_dispatcher
            .clone()
            .send(Task::Subscribe {
                message_type,
                backchannel,
            })
            .await?; // infailable
        Ok(Box::pin(rx))
    }

    /// Awaits the next MavMessage of the given MavMessageType
    ///
    /// # Arguments
    ///
    /// * `message_type` - `MavMessageType` of the desired messages
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::time::Duration;
    /// # use futures::prelude::*;
    /// # use async_mavlink::prelude::*;
    /// # use mavlink::{MavlinkVersion, common::MavMessage};
    /// # fn main() -> Result<(),AsyncMavlinkError> {
    /// # smol::block_on(async {
    /// # let (conn, future) = AsyncMavConn::new("udpin:127.0.0.4:14551", MavlinkVersion::V1)?;
    /// # smol::spawn(async move { future.await; }).detach();
    /// # smol::spawn(async {
    /// #   let mut conn = mavlink::connect("udpout:127.0.0.4:14551").unwrap();
    /// #   conn.set_protocol_version(mavlink::MavlinkVersion::V1);
    /// #   loop {
    /// #       let mut header = mavlink::MavHeader::default();
    /// #       header.system_id = 0;    
    /// #       conn.send(&header, &MavMessage::PARAM_VALUE(Default::default()));
    /// #       smol::Timer::after(Duration::from_millis(10)).await;
    /// #   }
    /// # }).detach();
    /// let message_type = MavMessageType::new(&MavMessage::PARAM_VALUE(Default::default()));
    ///
    /// if let MavMessage::PARAM_VALUE(data) = conn.request(message_type).await {
    ///     // do something with `data`
    /// }
    /// # Ok(()) })}
    /// ```
    pub async fn request(&self, message_type: MavMessageType<M>) -> M {
        let (backchannel, mut rx) = mpsc::unbounded();
        self.task_dispatcher
            .clone()
            .send(Task::Subscribe {
                message_type,
                backchannel,
            })
            .await
            .unwrap(); //this may never fail
        rx.next().map(|m| m.expect("Oh no!")).await
    }

    /// Send a `MavMessage` to the vehicle
    ///
    /// # Arguments
    ///
    /// * `message` - `MavMessage` to send
    ///
    /// # Examples
    ///
    /// ```
    /// # use std::time::Duration;
    /// # use futures::prelude::*;
    /// # use async_mavlink::AsyncMavConn;
    /// # use async_mavlink::prelude::*;
    /// # use mavlink::{MavHeader, MavlinkVersion, common::*};
    /// # fn main() -> Result<(),AsyncMavlinkError> {
    /// # smol::block_on(async {
    /// # let (conn, future) = AsyncMavConn::new("udpbcast:127.0.0.5:14551", MavlinkVersion::V1)?;
    /// # smol::spawn(async move { future.await; }).detach();
    /// let header = MavHeader::default();
    /// let message = MavMessage::PARAM_REQUEST_LIST(PARAM_REQUEST_LIST_DATA {
    ///     target_component: 0,
    ///     target_system: 0,
    /// });
    ///
    /// conn.send(&header, &message).await?;
    /// # Ok(())
    /// # })}
    /// ```
    pub async fn send(&self, header: &MavHeader, message: &M) -> Result<(), AsyncMavlinkError> {
        let (backchannel, receiver) = oneshot::channel();
        self.task_dispatcher
            .clone() // TODO get rid of clone
            .send(Task::Emit {
                header: *header,
                message: message.clone(),
                backchannel,
            })
            .await
            .map_err(AsyncMavlinkError::from)?;
        receiver.await.map_err(AsyncMavlinkError::from)?
    }

    /// Send a `MavMessage` to the vehicle
    ///
    /// # Arguments
    ///
    /// * `message` - `MavMessage` to send
    ///
    /// # Examples
    ///
    /// ```
    /// # use async_mavlink::prelude::*;
    /// # use mavlink::{MavlinkVersion,common::*};
    /// # fn main() -> Result<(), AsyncMavlinkError> {
    /// # use async_mavlink::prelude::AsyncMavlinkError;
    /// # smol::block_on(async {
    /// # let (conn, future) = AsyncMavConn::new("udpbcast:127.0.0.6:14551", MavlinkVersion::V1)?;
    /// # smol::spawn(async move { future.await; }).detach();
    /// let message = MavMessage::PARAM_REQUEST_LIST(PARAM_REQUEST_LIST_DATA {
    ///     target_component: 0,
    ///     target_system: 0,
    /// });
    ///
    /// conn.send_default(&message).await?;
    /// # Ok(())
    /// # })}
    /// ```
    pub async fn send_default(&self, message: &M) -> Result<(), AsyncMavlinkError> {
        Ok(self.send(&MavHeader::default(), message).await?)
    }

    /// Returns the `Instant` from the last received HEARTBEAT
    pub async fn last_heartbeat(&self) -> Option<Instant> {
        self.last_heartbeat.load_full().map(|arc| *arc)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use mavlink::common::*;
    use std::time::Duration;
    async fn new_conn<M: 'static + mavlink::Message + Clone + Send + Sync>(
        arg: &str,
    ) -> Arc<AsyncMavConn<M>> {
        let (conn, future) = AsyncMavConn::new(arg, mavlink::MavlinkVersion::V1).unwrap();
        smol::spawn(async move { future.await }).detach();
        conn
    }

    #[test]
    fn subscribe() -> Result<(), AsyncMavlinkError> {
        smol::block_on(async {
            let conn = new_conn("udpin:127.0.0.7:14551").await;

            smol::spawn(async move {
                let mut conn = mavlink::connect("udpout:127.0.0.7:14551").unwrap();
                conn.set_protocol_version(mavlink::MavlinkVersion::V1);
                loop {
                    conn.send_default(&MavMessage::HEARTBEAT(Default::default()))
                        .unwrap();
                    smol::Timer::after(Duration::from_millis(10)).await;
                }
            })
            .detach();

            let message_type = MavMessageType::new(&MavMessage::HEARTBEAT(Default::default()));
            let mut stream = conn.subscribe(message_type).await?;

            let mut i = 0;

            while let Some(MavMessage::HEARTBEAT(_data)) = (stream.next()).await {
                i += 1;
                if i > 5 {
                    break;
                }
            }

            Ok(())
        })
    }

    #[test]
    fn send() -> Result<(), AsyncMavlinkError> {
        smol::block_on(async {
            let conn = new_conn("udpout:127.0.0.8:14551").await;

            let mut raw_conn = mavlink::connect("udpin:127.0.0.8:14551").unwrap();
            raw_conn.set_protocol_version(mavlink::MavlinkVersion::V1);
            let received = blocking::unblock(move || raw_conn.recv());

            smol::spawn(async move {
                let header = mavlink::MavHeader::default();
                let message = MavMessage::HEARTBEAT(HEARTBEAT_DATA::default());
                for _ in 0usize..100 {
                    conn.send(&header, &message).await.unwrap();
                }
            })
            .detach();

            let received = received.await;
            assert!(received.is_ok());

            if let Ok((_, MavMessage::HEARTBEAT(_))) = received {
            } else {
                panic!("received wrong message");
            }

            Ok(())
        })
    }

    #[test]
    fn send_default() -> std::io::Result<()> {
        smol::block_on(async {
            let conn = new_conn("udpout:127.0.0.9:14551").await;

            let mut raw_conn = mavlink::connect("udpin:127.0.0.9:14551").unwrap();
            raw_conn.set_protocol_version(mavlink::MavlinkVersion::V1);
            let received = blocking::unblock(move || raw_conn.recv());

            smol::spawn(async move {
                let message = MavMessage::HEARTBEAT(HEARTBEAT_DATA::default());
                for _ in 0usize..100 {
                    conn.send_default(&message).await.unwrap();
                }
            })
            .detach();

            let received = received.await;
            assert!(received.is_ok());

            if let Ok((_, MavMessage::HEARTBEAT(_))) = received {
            } else {
                panic!("received wrong message");
            }

            Ok(())
        })
    }
}
