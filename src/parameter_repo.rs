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

use std::collections::HashMap;
use std::io;

use futures::{
    channel::mpsc::{self as channel, UnboundedSender},
    future::Either,
    prelude::*,
    StreamExt,
};

use crate::AsyncMavConn;

pub enum Error {
    /// The parameter does not exist
    DoesNotExist,
}

pub struct ParameterRepo<'a, M: mavlink::Message> {
    conn: &'a AsyncMavConn<M>,
    map: HashMap<String, f32>,
}

impl<'a, M: mavlink::Message> ParameterRepo<'a, M> {
    pub fn new(conn: &'a AsyncMavConn<M>) -> Self {
        Self {
            conn,
            map: HashMap::new(),
        }
    }

    /// refresh the repo __after all pending operations where processed__
    pub async fn refresh(&self) {}

    pub async fn get(&self, id: &str) -> Result<f32, Error> {
        todo!()
    }
}

/*
impl IntoIterator for &ParameterRepo {
    type Item=ParameterRepo;
    type IntoIter = std::vec::IntoIter<Self::Item>;


    fn into_iter(self) -> Self::IntoIter {
        Vec::new().into_iter()
    }
}
*/
