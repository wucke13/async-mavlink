//! A module for the various [microservices](https://mavlink.io/en/services/) present in MAVLink
//!
//! Currently, only the [parameter protocol](https://mavlink.io/en/services/parameter.html) is
//! implemented. It is feature gated behind the `parameter_protocol` feature, which is enabled per
//! default.

mod parameter_protocol;

#[cfg(feature = "parameter_protocol")]
pub use parameter_protocol::ParameterProtocol;
