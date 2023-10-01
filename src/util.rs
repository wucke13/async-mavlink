/// Decode param_id from mavlink's PARAM_VALUE_DATA struct
pub fn decode_param_id(bytes: &[u8; 16]) -> String {
    std::str::from_utf8(bytes)
        .unwrap()
        .trim_end_matches('\0')
        .to_string()
}

/// Encode param_id from mavlink's PARAM_VALUE_DATA struct
pub fn encode_param_id(name: &str) -> [u8; 16] {
    let mut bytes = [0u8; 16];
    bytes[..name.len()].copy_from_slice(name.as_bytes());
    bytes
}
