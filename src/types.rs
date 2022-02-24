use std::fmt::Debug;
use std::hash::{Hash, Hasher};
use std::mem::{discriminant, Discriminant};

use mavlink::Message;

use thiserror::Error;

/// Error type for the [`async_mavlink` crate](crate)
#[derive(Error, Debug)]
#[allow(missing_docs)]
pub enum AsyncMavlinkError {
    #[error("error on opening connection to the target system")]
    ConnectionRefused(#[from] std::io::Error),

    #[error("connection to the target system broke")]
    ConnectionLost(#[from] mavlink::error::MessageWriteError),

    #[error("unable to emit task to event loop")]
    TaskEmit(#[from] futures::channel::mpsc::SendError),

    #[error("the event loop canceled a send ack channel")]
    SendAck(#[from] futures::channel::oneshot::Canceled),

    #[error("the maximum number of retries was reached")]
    MaxRetriesReached,
}

/// Representation of the type of a specific MavMessage
pub struct MavMessageType<M: Message> {
    name: String,
    discriminant: Discriminant<M>,
}
impl<M: mavlink::Message> Eq for MavMessageType<M> {}
impl<M: mavlink::Message> PartialEq for MavMessageType<M> {
    fn eq(&self, rhs: &Self) -> bool {
        self.discriminant.eq(&rhs.discriminant)
    }
}
impl<M: mavlink::Message> Hash for MavMessageType<M> {
    fn hash<H>(&self, hasher: &mut H)
    where
        H: Hasher,
    {
        self.discriminant.hash(hasher)
    }
}

impl<M: Message + Debug> MavMessageType<M> {
    /// Returns the `MavMessageType` of a `MavMessage`
    ///
    /// # Arguments
    ///
    /// * `message` - The message whose type shall be represented
    /// # Examples
    ///
    /// ```
    /// use mavlink::common::MavMessage;
    /// use async_mavlink::prelude::*;
    ///
    /// let message_type = MavMessageType::new(&MavMessage::PARAM_VALUE(Default::default()));
    /// ```
    pub fn new(message: &M) -> MavMessageType<M> {
        let name = format!("{:?}", message)
            .split('(')
            .take(1)
            .next()
            .unwrap_or("")
            .to_string();

        #[allow(enum_intrinsics_non_enums)]
        Self {
            name,
            discriminant: discriminant(message),
        }
    }
}

impl<M: Message> std::fmt::Display for MavMessageType<M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}
