//! Public validation contract used by native and offline device editors.
pub fn validate_name(name: &str) -> Result<String, String> {
    zork_client_types::device::validate_name(name)
}

pub fn validate_peer(
    name: &str,
    origin: &str,
    address: &str,
) -> Result<zork_client_types::device::PeerInput, String> {
    let name = validate_name(name)?;
    let origin = origin.trim();
    if origin.is_empty() {
        return Err("请填写设备身份".into());
    }
    let address = address.trim();
    Ok(zork_client_types::device::PeerInput {
        name,
        origin: origin.into(),
        address: (!address.is_empty()).then(|| address.into()),
    })
}
