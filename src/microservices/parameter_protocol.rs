use crate::util::*;
use mavlink::common::{
    MavMessage, MavParamType, PARAM_REQUEST_LIST_DATA, PARAM_REQUEST_READ_DATA, PARAM_VALUE_DATA,
};
use mavlink::MavHeader;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::{future::Either, prelude::*, StreamExt};

use crate::prelude::*;
use std::pin::Pin;

/// An implementation of the [parameter protocol](https://mavlink.io/en/services/parameter.html)
/// with included parameter cache
///
/// It implements robust communication, that is it just works™ even if some packet loss occurs.
/// This however results in ugly runtime characteristics - byzantine faults can drastically
/// increase the time until a dead connection is detected.
///
/// The algorithm in use assumes that `PARAM_VALUE` messages from the MAV can be consumed faster
/// than it can produce them, otherwise some of the loops will never terminate if the MAV spams an
/// endless amount of `PARAM_VALUE` messages. Under normal circumstances this is a safe assumption
/// to make --- parameters should not change frequently _all the time_.
///
/// # Generic Arguments
///
/// * `F`: Timeout function. The function is expected to asynchronously wait for the desired time
///   span before a re-request is sent
/// * `T`: The return value of the timeout function. Can be literally anything, it will be
///   discarded.
/// * `N`: Threshold for re-requesting all or specific parameters. When after a timeout more than
///   `N` parameters are known to be missing, all parameters are re-requested from the target
///   system. If only `N` or less parameters are missing, only the missing parameters are
///   requested.
/// * The maximum number of retries to be attempted before accepting defeat. If a timeout occurs,
///   up to `R` re-requests are made. Only after `R` timeouts occured in a row with no
///   `PARAM_VALUE` message received in between, an `AsyncMavError::MaxRetriesReached` is returned.
pub struct ParameterProtocol<
    F: FnMut() -> Box<dyn Future<Output = T> + Unpin>,
    T,
    const N: usize,
    const R: u16,
> {
    conn: Arc<AsyncMavConn<MavMessage>>,
    params: HashMap<String, (f32, MavParamType)>,
    missing_indeces: HashSet<u16>,
    new_params: Pin<Box<dyn Stream<Item = MavMessage>>>,
    param_total_count: u16,
    target_system_id: u8,
    target_component_id: u8,
    retries_attempted: u16,
    timeout_fn: F,
}

impl<F: FnMut() -> Box<dyn Future<Output = T> + Unpin>, T, const N: usize, const R: u16>
    ParameterProtocol<F, T, N, R>
{
    /// Initialize a new instance of the parameter protocol
    ///
    /// # Arguments
    ///
    /// * `conn`: an async mavlink connection
    /// * `target_system`: the id of the target system
    /// * `target_component`: the id of the target component
    /// * `timeout_fn`: An implementation of the timeout function
    ///
    /// # Example
    ///
    /// ```
    /// use async_mavlink::{microservices::ParameterProtocol, prelude::*};
    /// use mavlink::{MavlinkVersion, common::*};
    /// use std::time::Duration;
    /// # fn main()->Result<(),AsyncMavlinkError>{ smol::block_on(async {
    /// # smol::spawn(async {
    /// #     let mut conn = mavlink::connect("udpout:127.0.0.1:14551").unwrap();
    /// #     conn.set_protocol_version(mavlink::MavlinkVersion::V1);
    /// #     loop {
    /// #         let mut value = PARAM_VALUE_DATA::default();
    /// #         value.param_id = to_char_arr("param_1");
    /// #         value.param_value = 13.0;
    /// #         value.param_count = 1;
    /// #         conn.send_default(&MavMessage::PARAM_VALUE(value));
    /// #         smol::Timer::after(std::time::Duration::from_millis(500)).await;
    /// #     }
    /// # }).detach();
    ///
    /// // establish connection
    /// let (conn, future) = AsyncMavConn::new("udpin:127.0.0.1:14551", MavlinkVersion::V1)?;
    ///  
    /// // start event loop
    /// smol::spawn(async move { future.await }).detach();
    ///
    /// // define timeout function utilizing the current async reactor
    /// let timeout = Duration::from_millis(500);
    /// let timeout_fn = || Box::new(smol::Timer::after(timeout)) as _;
    ///
    /// // create instance of param_prot. It will re-request all parameters when more than 64
    /// // parameters are missing at the moment of timeout, and it will declare the connection
    /// // defunct when 50 retries fail to yield any result.
    /// let mut param_prot: ParameterProtocol<_, _, 64, 50> =
    ///        ParameterProtocol::new(conn, 1, 1, timeout_fn).await?;
    ///
    /// // get all parameters
    /// let all_params = param_prot.get_all().await?;
    ///
    /// # Ok(())    
    /// # })
    /// # }
    /// ```
    pub async fn new(
        conn: Arc<AsyncMavConn<MavMessage>>,
        target_system: u8,
        target_component: u8,
        timeout_fn: F,
    ) -> Result<Self, AsyncMavlinkError> {
        let msg_type = MavMessageType::new(&MavMessage::PARAM_VALUE(Default::default()));
        let new_params = conn.subscribe(msg_type).await?;

        let repo = Self {
            conn,
            params: HashMap::new(),
            missing_indeces: HashSet::new(),
            new_params,
            param_total_count: u16::MAX,
            target_system_id: target_system,
            target_component_id: target_component,
            retries_attempted: 0,
            timeout_fn,
        };

        // request all parameters to get going
        repo.conn
            .send(&repo.mav_header(), &repo.mav_request_list())
            .await?;

        Ok(repo)
    }

    /// Get a parameter's value
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

    /// Set a parameter's value
    pub async fn set(
        &mut self,
        param_name: &str,
        param_value: f32,
    ) -> Result<bool, AsyncMavlinkError> {
        // process all enqueued messages and keep going until we got a full sync
        while !self.all_synced() {
            self.update().await?;
        }

        // do we know this param?
        Ok(match self.params.get(param_name) {
            Some((_old_value, param_type)) => {
                let msg = MavMessage::PARAM_SET(mavlink::common::PARAM_SET_DATA {
                    target_system: self.target_system_id,
                    target_component: self.target_component_id,
                    param_id: encode_param_id(param_name),
                    param_value,
                    param_type: *param_type,
                });
                // request change of parameter
                self.conn.send(&self.mav_header(), &msg).await?;
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
                            println!("sent again\n\n\n");
                            if last_send.elapsed() > Duration::from_millis(100) {
                                println!("sent again\n\n\n");
                                self.conn.send(&self.mav_header(), &msg).await?;
                                last_send = Instant::now();
                            }
                        }
                    }
                }
            }
            None => false,
        })
    }

    /// Determines if a full sync was achieved
    fn all_synced(&self) -> bool {
        self.missing_indeces.is_empty() && (self.param_total_count as usize == self.params.len())
    }

    /// Process a message, updates or invalidates the cache according to new information derived
    /// from the message. Expects `msg` to contain an instance of
    /// [`PARAM_VALUE`](mavlink::common::PARAM_VALUE_DATA).
    async fn process_message(&mut self, msg: &MavMessage) -> Result<(), AsyncMavlinkError> {
        // we expect a PARAM_VALUE mav message
        if let MavMessage::PARAM_VALUE(PARAM_VALUE_DATA {
            param_count,
            param_id,
            param_index,
            param_value,
            param_type,
        }) = msg
        {
            #[cfg(feature = "log")]
            log::trace!(
                "processing param #{param_index}, {} in total",
                self.param_total_count
            );

            if *param_count != self.param_total_count {
                #[cfg(feature = "log")]
                log::debug!(
                    "number of parameters changed, from {} to {param_count}",
                    self.param_total_count
                );

                // oh, the param_count changed. A new MAV? Better safe than sorry, let's invalidate the cache.
                // let's also request all parameters again.
                self.conn.send_default(&self.mav_request_list()).await?;
                self.param_total_count = *param_count;
                self.params.clear();
                self.missing_indeces = (0..self.param_total_count).collect();
            }

            // We now have the param with this value in cache
            self.missing_indeces.remove(param_index);
            self.params
                .insert(decode_param_id(param_id), (*param_value, *param_type));
        } else {
            panic!("this is impossible");
        }
        Ok(())
    }

    /// Process up to one enqueued message
    /// If a message was processed, it is returned in the Option
    async fn update(&mut self) -> Result<Option<MavMessage>, AsyncMavlinkError> {
        if self.retries_attempted > R {
            return Err(AsyncMavlinkError::MaxRetriesReached);
        }

        //let timeout = (self.timeout_fn)(REPEAT_REQUEST_TIMEOUT);

        Ok(
            // wait for new messages, up to REPEAT_REQUEST_TIMEOUT
            match future::select(self.new_params.next(), (self.timeout_fn)()).await {
                // we received either an update for an already known param or a new param
                Either::Left((Some(msg), _)) => {
                    self.retries_attempted = 0;
                    self.process_message(&msg).await?;
                    Some(msg)
                }
                // we ran in a timeout, and are still missing many params
                Either::Right(_) if self.missing_indeces.len() > N => {
                    self.retries_attempted += 1;

                    #[cfg(feature = "log")]
                    log::debug!(
                        "re-requesting all parameters, {} are missing. {} retries left.",
                        self.missing_indeces.len(),
                        R - self.retries_attempted,
                    );

                    self.conn
                        .send(&self.mav_header(), &self.mav_request_list())
                        .await?;
                    None
                }
                // we ran in a timeout, but only a few params are still missing
                Either::Right(_) => {
                    self.retries_attempted += 1;

                    #[cfg(feature = "log")]
                    log::debug!(
                        "re-requesting the {} missing parameters. {} retries left.",
                        self.missing_indeces.len(),
                        R - self.retries_attempted,
                    );

                    for index in self.missing_indeces.iter() {
                        self.conn
                            .send(&self.mav_header(), &self.mav_param_read("", *index as i16))
                            .await?;
                    }
                    None
                }
                // the stream ended, this should never happen?!
                Either::Left((None, _)) => {
                    panic!("this should never happen");
                }
            },
        )
    }

    /// generate a new PARAM_READ message
    fn mav_param_read(&self, param_id: &str, param_index: i16) -> MavMessage {
        MavMessage::PARAM_REQUEST_READ(PARAM_REQUEST_READ_DATA {
            target_system: self.target_system_id,
            target_component: self.target_component_id,
            param_id: encode_param_id(param_id),
            param_index,
        })
    }

    /// generate a new REQUEST_LIST message
    fn mav_request_list(&self) -> MavMessage {
        MavMessage::PARAM_REQUEST_LIST(PARAM_REQUEST_LIST_DATA {
            target_system: self.target_system_id,
            target_component: self.target_component_id,
        })
    }

    /// generate an appropiate [`MavHeader`](mavlink::MavHeader)
    fn mav_header(&self) -> MavHeader {
        MavHeader {
            system_id: self.target_system_id,
            component_id: self.target_component_id,
            ..Default::default()
        }
    }
}
