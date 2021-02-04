//! This module contains a repository for parameters.
//!
//! It implements robust communication, that is it should work just fine even if some packet loss
//! appears. This however results in weird runtime characteristics, many of the algorithms in use
//! try endlessly to achieve their goal. If the MAV does not respond to our messages, these will
//! never terminate.
//!
//!
//! # Examples
//!
//! ```
//! // let (conn, future) = AsyncMavConn::new("udpin:127.0.0.1:14551")?;
//! // start event loop
//! // smol::spawn(async move { future.await }).detach();
//! // let repo = ParameterRepo::new(&conn);
//!
//!
//! // let param = repo.get("AHR_ORIENTATION").await;
//!
//! // println!("parameter name = {}", param.name());
//! // println!("parameter value = {}", param.value().await);
//! // println!("parameter new_value = {}", param.set(23.0).await)
//! ```

use crate::util::*;
use mavlink::common::{MavMessage, MavParamType, PARAM_VALUE_DATA};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::{future::Either, prelude::*, StreamExt};

use crate::prelude::*;
use std::pin::Pin;

/// A parameter repository with included cache
///
/// The repo processes new parameters with potential new values only on method calls. The time for
/// a call to finish can vary widly, depending for example on the ammount of enqueued PARAM_VALUE
/// messages which need to be processed.
///
/// The algorithm in sync also assumes that we can consume PARAM_VALUE messages from the MAV faster
/// than it can produce them - otherwise some of the loops will never terminate. Under normal
/// circumstances this is safe assumption to make - parameters should not change frequently all the
/// time.
pub struct ParameterRepo {
    conn: Arc<AsyncMavConn<MavMessage>>,
    params: HashMap<String, (f32, MavParamType)>,
    missing_indexes: HashSet<u16>,
    new_params: Pin<Box<dyn Stream<Item = MavMessage>>>,
    param_count: u16,
    target_system: u8,
    target_component: u8,
    timeout_fn: Box<(dyn FnMut(Duration) -> Box<dyn Future<Output = ()> + Unpin>)>,
}

impl ParameterRepo {
    /// Initialize a new parameter repo
    ///
    ///
    pub async fn new<F: 'static + FnMut(Duration) -> Box<dyn Future<Output = ()> + Unpin>>(
        conn: Arc<AsyncMavConn<MavMessage>>,
        target_system: u8,
        target_component: u8,
        timeout_fn: F,
    ) -> Result<Self, AsyncMavlinkError> {
        let msg_type = MavMessageType::new(&MavMessage::PARAM_VALUE(Default::default()));
        let new_params = conn.subscribe(msg_type).await?;
        let timeout_fn = Box::new(timeout_fn);

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
        while repo.update().await?.is_none() {
            (repo.timeout_fn)(Duration::from_millis(10)).await;
            panic!("not blocked");
        }

        Ok(repo)
    }

    /// get a parameters value
    pub async fn get(&mut self, param_name: &str) -> Result<Option<f32>, AsyncMavlinkError> {
        // process all enqueued messages
        while self.update().await?.is_some() {}

        loop {
            match self.params.get(param_name) {
                Some((value, _)) => return Ok(Some(*value)),
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
        // process all enqueued messages and keep going until we got a full sync
        while self.update().await?.is_some() || !self.all_synced() {}

        Ok(self
            .params
            .iter()
            .map(|(k, (v, _))| (k.clone(), *v))
            .collect())
    }

    /// Set a parameter
    ///
    /// For this to work, a full sync needs to be done
    pub async fn set(
        &mut self,
        param_name: &str,
        param_value: f32,
    ) -> Result<bool, AsyncMavlinkError> {
        // process all enqueued messages and keep going until we got a full sync
        while !self.all_synced() || self.update().await?.is_some() {}

        // do we know this param?
        Ok(match self.params.get(param_name) {
            Some((_old_value, param_type)) => {
                let msg = MavMessage::PARAM_SET(mavlink::common::PARAM_SET_DATA {
                    target_system: self.target_system,
                    target_component: self.target_component,
                    param_id: to_char_arr(param_name),
                    param_value,
                    param_type: *param_type,
                });
                // request change of parameter
                self.conn.send_default(&msg).await?;
                let mut last_send = Instant::now();

                // while change was not acknowledged
                loop {
                    match self.update().await? {
                        Some(_) => {
                            if self
                                .params
                                .get(param_name)
                                .map(|(value, _)| *value == param_value)
                                .unwrap_or(false)
                            {
                                return Ok(true);
                            }
                        }
                        None => {
                            if last_send.elapsed() > Duration::from_millis(100) {
                                self.conn.send_default(&msg).await?;
                                last_send = Instant::now();
                            }
                        }
                    }
                }
            }
            None => false,
        })
    }

    /// Determines if we are fully synced
    ///
    /// For this to work at least one PARAM_VALUE message has to have been processed already.
    fn all_synced(&self) -> bool {
        self.missing_indexes.is_empty() && (self.param_count as usize == self.params.len())
    }

    /// Process a message, updates or invalidates the cache according to new intel derived from the
    /// message
    async fn process_message(&mut self, msg: &MavMessage) -> Result<(), AsyncMavlinkError> {
        match msg {
            MavMessage::PARAM_VALUE(PARAM_VALUE_DATA {
                param_count,
                param_id,
                param_index,
                param_value,
                param_type,
            }) => {
                if *param_count != self.param_count {
                    // oh, the param_count changed. A new MAV? Better safe than sorry, let's invalidate the cache.
                    // let's also request all parameters again.
                    self.conn.send_default(&self.mav_request_list()).await?;
                    self.param_count = *param_count;
                    self.params.clear();
                    self.missing_indexes = (0..self.param_count).collect();
                }

                // We now have the param with this value in cache
                self.missing_indexes.remove(&param_index);
                self.params
                    .insert(to_string(param_id), (*param_value, *param_type));
            }
            _ => {
                panic!("this is impossible");
            }
        }
        Ok(())
    }

    /// Process up to one enqueued message
    /// If a message was processed, it is returned in the Option
    async fn update(&mut self) -> Result<Option<MavMessage>, AsyncMavlinkError> {
        Ok(match self.all_synced() {
            true => {
                if let Some(msg) = self.new_params.next().now_or_never().flatten() {
                    self.process_message(&msg).await?;
                    Some(msg)
                } else {
                    None
                }
            } // TODO
            false => {
                let timeout = (self.timeout_fn)(Duration::from_millis(100));
                match future::select(self.new_params.next(), timeout).await {
                    Either::Left((Some(msg), _)) => {
                        self.process_message(&msg).await?;
                        Some(msg)
                    }
                    Either::Right(_) => {
                        // ask for all missing ones
                        for index in self.missing_indexes.iter() {
                            self.conn
                                .send_default(&self.mav_param_read("", *index as i16))
                                .await?;
                        }
                        None
                    }
                    _ => None,
                }
            }
        })
    }

    /// generate a new PARAM_READ message
    fn mav_param_read(&self, param_id: &str, param_index: i16) -> MavMessage {
        use mavlink::common::*;
        MavMessage::PARAM_REQUEST_READ(PARAM_REQUEST_READ_DATA {
            target_system: self.target_system,
            target_component: self.target_component,
            param_id: to_char_arr(param_id),
            param_index,
        })
    }

    /// generate a new REQUEST_LIST message
    fn mav_request_list(&self) -> MavMessage {
        use mavlink::common::*;
        MavMessage::PARAM_REQUEST_LIST(PARAM_REQUEST_LIST_DATA {
            target_system: self.target_system,
            target_component: self.target_component,
        })
    }
}
