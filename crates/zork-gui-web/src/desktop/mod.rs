#[path = "../../../zork-gui/src/desktop/agents.rs"]
mod agents;
pub use zork_ui::navigation;
#[path = "../../../zork-gui/src/desktop/profile_quota.rs"]
mod profile_quota;
#[path = "../../../zork-gui/src/desktop/profiles.rs"]
mod profiles;
#[path = "../../../zork-gui/src/desktop/stories.rs"]
pub mod stories;
pub use zork_ui::controls as ui;

#[path = "../../../zork-gui/src/desktop/interaction_story.rs"]
mod interaction_story;
#[path = "../../../zork-gui/src/desktop/interaction_view.rs"]
mod interaction_view;

#[path = "../../../zork-gui/src/desktop/notification_story.rs"]
mod notification_story;
