use std::hash::{Hash, Hasher};
use std::mem::{discriminant, Discriminant};

use mavlink::Message;

use thiserror::Error;

// TODO find better names for all of this
/// Error type
#[derive(Error, Debug)]
pub enum AsyncMavlinkError {
    /// IO Error encountered when trying to communicate with the MAV
    #[error("connection to MAV lost")]
    ConnectionLost(#[from] std::io::Error),

    /// The event loop does not take our call
    #[error("unable to emit task to event loop")]
    TaskEmit(#[from] futures::channel::mpsc::SendError),

    /// The event loop canceled our send request
    #[error("the event loop canceled our send ack channel")]
    SendAck(#[from] futures::channel::oneshot::Canceled),
}

/// Representation of the type of a specific MavMessage
pub struct MavMessageType<M: Message>(Discriminant<M>);
impl<M: mavlink::Message> Eq for MavMessageType<M> {}
impl<M: mavlink::Message> PartialEq for MavMessageType<M> {
    fn eq(&self, rhs: &Self) -> bool {
        self.0.eq(&rhs.0)
    }
}
impl<M: mavlink::Message> Hash for MavMessageType<M> {
    fn hash<H>(&self, hasher: &mut H)
    where
        H: Hasher,
    {
        self.0.hash(hasher)
    }
}

impl<M: Message> MavMessageType<M> {
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
        #[allow(clippy::mem_discriminant_non_enum)]
        Self(discriminant(message))
    }
}
