//! Device-local appearance shared by native client adapters.
use crate::store::ClientStore;
use anyhow::Result;
use serde::Serialize;

pub const THEME_KEY: &str = "theme";

/// The client theme. `System` follows the platform's light or dark appearance.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ClientPreferences {
    pub theme: Theme,
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

pub fn read(store: &ClientStore) -> ClientPreferences {
    ClientPreferences {
        theme: store
            .get::<Theme>("device", THEME_KEY)
            .ok()
            .flatten()
            .unwrap_or_default(),
    }
}

pub fn save_theme(store: &ClientStore, theme: Theme) -> Result<ClientPreferences> {
    store.put("device", THEME_KEY, &theme)?;
    Ok(read(store))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_survives_reopening_and_unknown_values_follow_the_system() {
        let directory = tempfile::tempdir().unwrap();
        {
            let store = ClientStore::open(directory.path()).unwrap();
            assert_eq!(read(&store).theme, Theme::System);
            save_theme(&store, Theme::Dark).unwrap();
        }
        let store = ClientStore::open(directory.path()).unwrap();
        assert_eq!(read(&store).theme, Theme::Dark);
        save_theme(&store, Theme::Light).unwrap();
        assert_eq!(read(&store).theme, Theme::Light);
        store.put("device", THEME_KEY, &"sepia").unwrap();
        assert_eq!(read(&store).theme, Theme::System);
    }
}
