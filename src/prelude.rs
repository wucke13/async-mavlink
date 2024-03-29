//! The prelude is a collection of all traits and commonly used types in this crate
//!
//! For normal use of this crate it is sufficient to glob import only this module, e.g. `use
//! async_mavlink::prelude::*`.

pub use crate::{
    types::{AsyncMavlinkError, MavMessageType},
    util::*,
    AsyncMavConn,
};
