//! Device-local appearance shared by native client adapters.
use crate::store::ClientStore;
use anyhow::Result;
use serde::Serialize;

pub const MESSAGE_PREVIEW_HEIGHT_KEY: &str = "message-preview-height";
pub const MESSAGE_PREVIEW_MIN: u32 = 80;
pub const MESSAGE_PREVIEW_MAX: u32 = 720;

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ClientPreferences {
    /// Zero selects the platform's automatic preview height.
    pub message_preview_height: u32,
}

pub enum ViewState {
    LastSession,
    Reading(String),
    SidebarWidth,
}
impl ViewState {
    fn key(&self) -> Result<String> {
        Ok(match self {
            Self::LastSession => "last-session".into(),
            Self::Reading(id) => {
                crate::valid_session(id)?;
                format!("reading:{id}")
            }
            Self::SidebarWidth => "sidebar-width".into(),
        })
    }
}
pub fn save_view_state<T: Serialize>(
    store: &ClientStore,
    scope: &str,
    state: ViewState,
    value: &T,
) -> Result<()> {
    store.put(scope, &state.key()?, value)
}
pub fn read_view_state<T: serde::de::DeserializeOwned>(
    store: &ClientStore,
    scope: &str,
    state: ViewState,
) -> Result<Option<T>> {
    store.get(scope, &state.key()?)
}

fn valid_height(height: u32) -> bool {
    height == 0 || (MESSAGE_PREVIEW_MIN..=MESSAGE_PREVIEW_MAX).contains(&height)
}

pub fn read(store: &ClientStore) -> ClientPreferences {
    ClientPreferences {
        message_preview_height: store
            .get::<u32>("device", MESSAGE_PREVIEW_HEIGHT_KEY)
            .ok()
            .flatten()
            .filter(|height| valid_height(*height))
            .unwrap_or(0),
    }
}

pub fn save_message_preview_height(store: &ClientStore, height: u32) -> Result<ClientPreferences> {
    anyhow::ensure!(valid_height(height), "消息折叠高度须为 80–720，或选择自动");
    store.put("device", MESSAGE_PREVIEW_HEIGHT_KEY, &height)?;
    Ok(read(store))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_survives_reopening_and_invalid_updates_preserve_the_value() {
        let directory = tempfile::tempdir().unwrap();
        {
            let store = ClientStore::open(directory.path()).unwrap();
            assert_eq!(read(&store).message_preview_height, 0);
            save_message_preview_height(&store, 360).unwrap();
            assert!(save_message_preview_height(&store, 79).is_err());
            assert!(save_message_preview_height(&store, 721).is_err());
        }
        let store = ClientStore::open(directory.path()).unwrap();
        assert_eq!(read(&store).message_preview_height, 360);
        save_message_preview_height(&store, 0).unwrap();
        assert_eq!(read(&store).message_preview_height, 0);
        store
            .put("device", MESSAGE_PREVIEW_HEIGHT_KEY, &"invalid")
            .unwrap();
        assert_eq!(read(&store).message_preview_height, 0);
    }
}
