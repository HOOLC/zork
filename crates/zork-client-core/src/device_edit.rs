//! Public validation contract used by native and offline device editors.
pub fn validate_name(name: &str) -> Result<String, String> {
    zork_client_types::device::validate_name(name)
}

/// A Mesh display name: short, not empty, and not used by another device of
/// the Mesh (compared without case or surrounding spaces).
pub fn validate_display_name(name: &str, others: &[&str]) -> Result<String, String> {
    let name = zork_client_types::device::validate_display_name(name)?;
    if others
        .iter()
        .any(|other| other.trim().to_lowercase() == name.to_lowercase())
    {
        return Err(format!(
            "名称“{name}”已被 Mesh 中的另一台设备使用，请换一个名称"
        ));
    }
    Ok(name)
}

/// The fixture Mesh of design stories: devices A and C already exist.
pub fn validate_story_display_name(name: &str) -> Result<String, String> {
    validate_display_name(name, &["A", "C"])
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
