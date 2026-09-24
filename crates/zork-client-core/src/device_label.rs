//! Human names for devices and the sessions that run on them.
//!
//! Every client shows the same device name, so the core owns it: the name the
//! user or the device gave itself, never a peer key, node id or session id.
//! A name that is missing or only an identifier becomes "未命名设备"; two devices
//! with the same name get a short suffix from their id so they stay apart.

const UNNAMED: &str = "未命名设备";
const ID_PREFIXES: [&str; 6] = ["key:", "session:", "agent:", "node:", "user:", "peer:"];

/// Whether `value` is an identifier rather than something a person named.
pub fn is_id_like(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("session") {
        return true;
    }
    if ID_PREFIXES.iter().any(|prefix| value.starts_with(prefix)) {
        return true;
    }
    let alnum = value.chars().all(|c| c.is_ascii_alphanumeric());
    let digits = value.chars().any(|c| c.is_ascii_digit());
    let letters = value.chars().any(|c| c.is_ascii_alphabetic());
    // ULID, base32 keys and similar opaque tokens.
    if alnum && value.len() >= 20 && digits && letters {
        return true;
    }
    // Hex digests and UUIDs.
    let hex = value.chars().filter(|c| *c != '-').collect::<String>();
    hex.len() >= 12 && hex.chars().all(|c| c.is_ascii_hexdigit()) && digits
}

/// The display name for a device whose stored name is `name`.
pub fn device_label(name: &str) -> String {
    if is_id_like(name) {
        UNNAMED.into()
    } else {
        name.trim().into()
    }
}

/// Short, stable suffix taken from the end of a device id.
fn suffix(id: &str) -> String {
    let tail = id
        .chars()
        .rev()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(4)
        .collect::<Vec<_>>();
    tail.into_iter().rev().collect::<String>().to_uppercase()
}

/// Replaces each `(id, name)` name with its label, adding an id suffix to
/// labels that more than one device would share.
pub fn label_devices<'a>(devices: impl IntoIterator<Item = (&'a str, &'a mut String)>) {
    let mut devices = devices.into_iter().collect::<Vec<_>>();
    for (_, name) in &mut devices {
        **name = device_label(name);
    }
    let mut counts = std::collections::HashMap::<String, usize>::new();
    for (_, name) in &devices {
        *counts.entry((**name).clone()).or_default() += 1;
    }
    for (id, name) in &mut devices {
        if counts[&**name] > 1 {
            let tail = suffix(id);
            if !tail.is_empty() {
                **name = format!("{} · {tail}", name);
            }
        }
    }
}

/// The name a Chat member shows. A Session member runs on the Chat's device,
/// so it takes the device's name instead of an id or the generic "Session".
pub fn member_label(member_id: &str, name: &str, device: Option<&str>) -> String {
    let unnamed = is_id_like(name) || name == member_id || member_id.starts_with("session:");
    match (unnamed, device) {
        (true, Some(device)) => device_label(device),
        (true, None) => UNNAMED.into(),
        (false, _) => name.trim().into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_never_names() {
        for id in [
            "",
            "  ",
            "Session",
            &format!("key:{}", "a".repeat(52)),
            "session:chat-1",
            "01J9Z3XK8Q2M4N6P8R0T2V4W6Y",
            "3f2a9c0b7d1e4f56",
            "550e8400-e29b-41d4-a716-446655440000",
            "agent:leader",
        ] {
            assert!(is_id_like(id), "{id}");
            assert_eq!(device_label(id), "未命名设备");
        }
        for name in ["Studio", "mini1", "zuozijiandeMacBook-Air", "工作室的 MacBook Air", "DESKTOP-4F2K9QZ", "本机设备"] {
            assert!(!is_id_like(name), "{name}");
            assert_eq!(device_label(name), name);
        }
    }

    #[test]
    fn colliding_names_get_a_short_suffix() {
        let mut a = "mini".to_string();
        let mut b = "mini".to_string();
        let mut c = "Studio".to_string();
        let mut d = String::new();
        label_devices([
            ("key:aaaa1111", &mut a),
            ("key:bbbb2222", &mut b),
            ("key:cccc", &mut c),
            ("key:dddd9999", &mut d),
        ]);
        assert_eq!(a, "mini · 1111");
        assert_eq!(b, "mini · 2222");
        assert_eq!(c, "Studio");
        assert_eq!(d, "未命名设备");
    }

    #[test]
    fn session_members_take_the_device_name() {
        assert_eq!(member_label("session:abc", "Session", Some("Studio")), "Studio");
        assert_eq!(member_label("01J9Z3XK8Q2M4N6P8R0T2V4W6Y", "01J9Z3XK8Q2M4N6P8R0T2V4W6Y", Some("MBA")), "MBA");
        assert_eq!(member_label("leader", "产品领队", Some("Studio")), "产品领队");
        assert_eq!(member_label("session:abc", "Session", None), "未命名设备");
    }
}
