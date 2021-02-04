//! This module contains a repository for parameters.
//!
//! It implements robust communication, that is it should work just fine even if some packet loss
//! appears. Per design this is a singleton: if you crate more than one `ParameterRepo`, changes
//! from one repo will not automagically be propagated to the other
//!
//!
//! # Examples
//!
//! ```
//! //let (conn, future) = AsyncMavConn::new("udpin:127.0.0.1:14551")?;
//! // start event loop
//! //smol::spawn(async move { future.await }).detach();
//! //let repo = ParameterRepo::new(&conn);
//!
//!
//! //let param = repo.get("AHR_ORIENTATION").await;
//!
//! //println!("paramater name = {}", param.name());
//! //println!("parameter value = {}", param.value().await);
//! //println!("parameter new_value = {}", param.set(23.0).await)
//! ```

use crate::util::*;
use mavlink::common::{MavMessage, PARAM_VALUE_DATA};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use futures::{future::Either, prelude::*, StreamExt};

use crate::prelude::*;
use std::pin::Pin;

/// TODO
pub struct ParameterRepo<F: FnMut(Duration) -> T, T: Future<Output = U> + Unpin, U> {
    conn: Arc<AsyncMavConn<MavMessage>>,
    params: HashMap<String, f32>,
    missing_indexes: HashSet<u16>,
    new_params: Pin<Box<dyn Stream<Item = MavMessage>>>,
    param_count: u16,
    target_system: u8,
    target_component: u8,
    timeout_fn: F,
}

impl<F: FnMut(Duration) -> T, T: Future<Output = U> + Unpin, U> ParameterRepo<F, T, U> {
    /// Initialize a new parameter repo
    pub async fn new(
        conn: Arc<AsyncMavConn<MavMessage>>,
        target_system: u8,
        target_component: u8,
        timeout_fn: F,
    ) -> Result<Self, AsyncMavlinkError> {
        let msg_type = MavMessageType::new(&MavMessage::PARAM_VALUE(Default::default()));
        let new_params = conn.subscribe(msg_type).await?;

        let mut repo = Self {
            conn,
            params: HashMap::new(),
            missing_indexes: HashSet::new(),
            new_params,
            param_count: 1,
            target_system,
            target_component,
            timeout_fn,
        };

        // request all parameters to get going
        repo.conn.send_default(&repo.mav_request_list()).await?;

        // wait for first param_value to happen
        // the repo will only work as expected once `update()`
        // did actually process one PARAM_VALUE message.
        while !repo.update().await? {
            (repo.timeout_fn)(Duration::from_millis(10)).await;
        }

        Ok(repo)
    }

    /// get a parameters value
    pub async fn get(&mut self, param_name: &str) -> Result<Option<f32>, AsyncMavlinkError> {
        self.update().await?;

        loop {
            match self.params.get(param_name) {
                Some(value) => return Ok(Some(*value)),
                None if self.all_synced() => {
                    return Ok(None);
                }
                None => {
                    self.update().await?;
                }
            }
        }
    }

    /// Get all parameters
    pub async fn get_all(&mut self) -> Result<HashMap<String, f32>, AsyncMavlinkError> {
        while !self.all_synced() {
            self.update().await?;
        }

        Ok(self.params.clone())
    }

    /*
    pub async fn set(&mut self, param_name: &str, param_value:f32 ){
        let msg = PARAM_SET_DATA {
            target_system: 1, // TODO handle this better
            target_component: 1, // TODO handle this better
            param_id: to_char_arr(param_name),
            param_value: param_value,
            param_type:

        };

        self.conn.send_default(&MavMessage::PARAM_SET(msg));
    }
     */

    fn all_synced(&self) -> bool {
        self.missing_indexes.is_empty() && (self.param_count as usize == self.params.len())
    }

    async fn process_message(&mut self, msg: MavMessage) -> Result<(), AsyncMavlinkError> {
        match msg {
            MavMessage::PARAM_VALUE(PARAM_VALUE_DATA {
                param_count,
                param_id,
                param_index,
                param_value,
                ..
            }) => {
                if param_count != self.param_count {
                    // oh, the param_count changed. A new MAV? Better safe than sorry, let's invalidate the cache.
                    // let's also request all parameters again.
                    self.conn.send_default(&self.mav_request_list()).await?;
                    self.param_count = param_count;
                    self.params.clear();
                    self.missing_indexes = (0..self.param_count).collect();
                }

                // We now have the param with this value in cache
                self.missing_indexes.remove(&param_index);
                self.params.insert(to_string(&param_id), param_value);
            }
            _ => {
                panic!("Oh no");
            }
        }
        Ok(())
    }

    /// refresh the repo
    ///
    /// The bool tells us, if something happened
    async fn update(&mut self) -> Result<bool, AsyncMavlinkError> {
        let mut processed_a_message = false;

        match self.all_synced() {
            true => {
                if let Some(msg) = self.new_params.next().now_or_never().flatten() {
                    processed_a_message = true;
                    self.process_message(msg).await?
                }
            } // TODO handle error
            false => {
                match future::select(
                    (self.timeout_fn)(Duration::from_millis(100)),
                    self.new_params.next(),
                )
                .await
                {
                    Either::Left((_, _)) if self.missing_indexes.is_empty() => {
                        // we are still missing indexes and quite some time passed since we receive the last PARAM_VALUE message
                        if let Some(index) = self.missing_indexes.iter().next() {
                            self.conn
                                .send_default(&self.mav_param_read("", *index as i16))
                                .await?;
                        }
                    }
                    Either::Right((Some(msg), _)) => {
                        self.process_message(msg).await?;
                        processed_a_message = true;
                    }
                    _ => {}
                }
            }
        }

        Ok(processed_a_message)
    }

    fn mav_param_read(&self, param_id: &str, param_index: i16) -> MavMessage {
        use mavlink::common::*;
        MavMessage::PARAM_REQUEST_READ(PARAM_REQUEST_READ_DATA {
            target_system: self.target_system,
            target_component: self.target_component,
            param_id: to_char_arr(param_id),
            param_index,
        })
    }

    fn mav_request_list(&self) -> MavMessage {
        use mavlink::common::*;
        MavMessage::PARAM_REQUEST_LIST(PARAM_REQUEST_LIST_DATA {
            target_system: self.target_system,
            target_component: self.target_component,
        })
    }
}
