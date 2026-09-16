//! A user-started local script carried by an ordinary Chat message.
//! It has no remote request, result or device binding.
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const VERSION: u32 = 1;
pub const MAX_SOURCE_BYTES: usize = 64 * 1024;

/// The tool's versioned host API help is generated from the same source as the card contract.
pub const API_HELP: &str = r#"Publish an Android-local JavaScript ES module. The user clicks Run on Android; PC displays it read-only. This is an ordinary send: do not wait for execution or expect a result message. Use JavaScript variables, loops, functions and top-level await. Host methods return Promises and throw on native failure; all values stay local. APIs (v1): android.info() -> {sdk,packageName,manufacturer,model}; android.startActivity(intent), android.startActivityForResult(intent) -> {resultCode,data,uris,extras}; intent={action,data?,mimeType?,package?,categories?:string[],extras?:object,chooserTitle?,flags?:number}. Extras accept strings, booleans, numbers, string arrays or {uri:"content://..."}; use full Android action/extra names. A launch only confirms the jump, not the user's later action. android.requestPermissions(string[]) -> object mapping Android permission names to booleans; android.hasPermission(name) -> boolean. android.clipboard.write(text), .read() -> string|null, .clear(); android.torch(enabled,cameraId?); android.volume.get(stream?) -> {index,min,max}, .set(index,stream?,showUi?); streams: music (default), alarm, ring, notification. android.vibrate(milliseconds); android.settings.get(namespace,key) -> string|null (system,secure,global), .canWrite() -> boolean, .put(key,value) (only screen_brightness,screen_brightness_mode,accelerometer_rotation,user_rotation,screen_off_timeout; requires user-granted WRITE_SETTINGS). android.content.readText(uri) -> string, .writeText(uri,text) -> byte count: only content:// URIs explicitly returned by this run's document picker, never app-private files. android.location() -> {latitude,longitude,accuracy,time}; request coarse/fine location first. android.sleep(milliseconds); console.log/warn/error(...values) show bounded local output. No shell, Node, filesystem imports, automatic execution or Agent callbacks. Ordinary message text never executes. Runtime permissions available: CAMERA, ACCESS_COARSE_LOCATION, ACCESS_FINE_LOCATION, CALL_PHONE, POST_NOTIFICATIONS (prefix android.permission.). Request CAMERA before delegated capture because Zork declares it. Files: await android.startActivityForResult({action:"android.intent.action.OPEN_DOCUMENT",mimeType:"text/plain",categories:["android.intent.category.OPENABLE"],flags:1}); use returned data URI with content.readText. Developer options: await android.startActivity({action:"android.settings.APPLICATION_DEVELOPMENT_SETTINGS"}); Copy then open: await android.clipboard.write("command"); await android.startActivity({action:"android.settings.APPLICATION_DEVELOPMENT_SETTINGS"}); Scripts have bounded memory, CPU time, native calls and a 5-minute deadline; long system dialogs can time out. Cancellation stops subsequent steps; native actions already started may still complete."#;

fn version() -> u32 {
    VERSION
}
fn platform() -> String {
    "android".into()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[default]
    LocalScript,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Card {
    #[serde(default)]
    pub kind: Kind,
    #[serde(default = "version")]
    pub version: u32,
    #[serde(default = "platform")]
    pub platform: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub source: String,
}

impl Card {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != VERSION || self.platform != "android" {
            return Err("unsupported_local_script");
        }
        if self.title.trim().is_empty()
            || self.title.len() > 512
            || self.description.as_ref().is_some_and(|s| s.len() > 4096)
            || self.source.trim().is_empty()
            || self.source.len() > MAX_SOURCE_BYTES
            || self.source.contains('\0')
        {
            return Err("invalid_local_script");
        }
        Ok(())
    }

    pub fn parse(value: &Value) -> Option<Self> {
        // Ordinary message text and other card payloads are never scripts.
        if value["kind"] != "local_script"
            || value["version"] != VERSION
            || value["platform"] != "android"
        {
            return None;
        }
        let card: Self = serde_json::from_value(value.clone()).ok()?;
        card.validate().ok()?;
        Some(card)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn only_supported_explicit_script_cards_are_executable() {
        let card: Card = serde_json::from_value(json!({"title":"Open settings","source":"await android.startActivity({action: 'android.settings.SETTINGS'});"})).unwrap();
        let value = serde_json::to_value(card).unwrap();
        assert!(Card::parse(&value).is_some());
        for (field, bad) in [
            ("version", json!(99)),
            ("platform", json!("desktop")),
            ("kind", json!("request")),
            ("source", json!("")),
        ] {
            let mut value = value.clone();
            value[field] = bad;
            assert!(Card::parse(&value).is_none());
        }
        assert!(Card::parse(&json!({"text":"await android.startActivity({});"})).is_none());
    }
}
