//! Shared desktop design. Contract: docs/design/interface.md.
//! Hallmark · Workbench · modern-minimal · native GPUI tokens.
//!
//! Rendering stays in `views`, but the durable palette, hierarchy, and default
//! geometry live here so regressions do not silently turn the app back into a
//! generic dashboard.

use std::sync::atomic::{AtomicBool, Ordering};

/// The brand persimmon of the mark. Interface accents read `INTERACTION.accent`,
/// which each theme tunes for its surfaces.
pub const BRAND_ACCENT: u32 = 0xE9643B;
/// Keyboard focus is an independent ring outside the outline (`INTERACTION.focus_ring`).
pub const FOCUS_RING_ALPHA: f32 = 0.6;

/// Light or dark rendering. Colors are read through the themed statics below,
/// so ordinary reads such as `ZORK_UI.palette.text` follow the current theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Light,
    Dark,
}
static DARK: AtomicBool = AtomicBool::new(false);
pub fn theme() -> Theme {
    if DARK.load(Ordering::Relaxed) {
        Theme::Dark
    } else {
        Theme::Light
    }
}
pub fn set_theme(theme: Theme) {
    DARK.store(theme == Theme::Dark, Ordering::Relaxed);
}
impl Theme {
    pub fn appearance(self) -> gpui::WindowAppearance {
        match self {
            Theme::Light => gpui::WindowAppearance::Light,
            Theme::Dark => gpui::WindowAppearance::Dark,
        }
    }
    pub fn for_appearance(appearance: gpui::WindowAppearance) -> Theme {
        match appearance {
            gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark => Theme::Dark,
            _ => Theme::Light,
        }
    }
}
/// `ZORK_THEME=light|dark` pins the theme; otherwise it follows the system.
pub fn pinned_theme() -> Option<Theme> {
    match std::env::var("ZORK_THEME").ok()?.to_ascii_lowercase().as_str() {
        "light" => Some(Theme::Light),
        "dark" => Some(Theme::Dark),
        _ => None,
    }
}
/// Switches palettes and the component library to `theme`, then repaints.
pub fn apply_theme(theme: Theme, cx: &mut gpui::App) {
    if self::theme() == theme {
        return;
    }
    set_theme(theme);
    init_component_theme(cx);
    crate::components::region::invalidate_every(cx);
    cx.refresh_windows();
}
/// Keeps a window's theme in step with the system appearance, unless pinned.
pub fn follow_appearance(window: &gpui::Window) -> gpui::Subscription {
    window.observe_window_appearance(|window, cx| {
        if pinned_theme().is_none() {
            apply_theme(Theme::for_appearance(window.appearance()), cx);
        }
    })
}

/// A value with a light and a dark variant; dereferences to the current one.
pub struct Themed<T: 'static> {
    light: T,
    dark: T,
}
impl<T> Themed<T> {
    pub const fn new(light: T, dark: T) -> Self {
        Self { light, dark }
    }
    pub fn get(&self, theme: Theme) -> &T {
        match theme {
            Theme::Light => &self.light,
            Theme::Dark => &self.dark,
        }
    }
}
impl<T> std::ops::Deref for Themed<T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.get(theme())
    }
}

/// Corner radii by role. Controls are capsules; containers grow so a capsule
/// sits concentric inside them (inner radius = outer radius - padding).
/// Corners render as continuous-curvature curves in the GPUI renderer.
pub struct Radii {
    /// Inline code, key caps and skeleton lines.
    pub inline: f32,
    /// Buttons, fields, rows, menu items: a capsule at the 32 px control height.
    pub control: f32,
    /// Embedded blocks that can grow: code blocks, attachments, multi-line fields.
    pub block: f32,
    /// Cards, menus, popovers and banners.
    pub container: f32,
    /// Dialogs, the composer and other large surfaces.
    pub surface: f32,
    /// Message bubbles; the folded corner echoes the brand mark.
    pub bubble: f32,
    pub fold: f32,
}
pub const RADIUS: Radii = Radii {
    inline: 6.,
    control: 16.,
    block: 16.,
    container: 24.,
    surface: 32.,
    bubble: 24.,
    fold: 8.,
};

/// Stable identity hues for devices. They mark which device a Chat or message
/// belongs to and never express status; status keeps its own shape and text.
/// Both themes share lightness and chroma within the set; dark lifts it a step.
pub static DEVICE_HUES: Themed<[u32; 5]> = Themed::new(
    [0x5E8B6B, 0x56759A, 0xA27A2B, 0x87618F, 0x3E8787],
    [0x6E9D7B, 0x6C8BB0, 0xB8903E, 0x9D78A6, 0x52A0A0],
);
/// Selected text in inputs and messages (RGBA).
pub static TEXT_SELECTION: Themed<u32> = Themed::new(0xC9DCF5CC, 0x35507ACC);
/// Inline code ink inside prose.
pub static CODE_INK: Themed<u32> = Themed::new(0x7C3FA0, 0xC9A2E8);
pub fn device_hue(key: &str) -> u32 {
    let hash = key
        .bytes()
        .fold(0x811C9DC5u32, |hash, byte| (hash ^ byte as u32).wrapping_mul(0x0100_0193));
    let hues = *DEVICE_HUES;
    hues[hash as usize % hues.len()]
}
pub const BORDER_WIDTH: f32 = 0.5;

pub struct InteractionPalette {
    pub neutral_hover: u32,
    pub neutral_pressed: u32,
    pub primary_hover: u32,
    pub primary_pressed: u32,
    pub accent_hover: u32,
    pub accent_pressed: u32,
    pub danger_hover: u32,
    pub danger_pressed: u32,
    pub focus_border: u32,
    /// Persimmon for work in progress and sending; never failure, unread or offline.
    pub accent: u32,
    /// A selected row keeps its identity under the pointer: hover deepens it.
    pub selected_hover: u32,
    pub focus_ring: u32,
}
pub static INTERACTION: Themed<InteractionPalette> = Themed::new(
    InteractionPalette {
        neutral_hover: 0xEDEAE3,
        neutral_pressed: 0xE5E1D8,
        primary_hover: 0x3B3F44,
        primary_pressed: 0x4C5157,
        accent_hover: 0xDB572F,
        accent_pressed: 0xC84A27,
        danger_hover: 0x9E252C,
        danger_pressed: 0x8A1D23,
        focus_border: 0x24272B,
        accent: 0xE9643B,
        selected_hover: 0xDCD6CA,
        focus_ring: 0xE9643B,
    },
    // Dark states move away from the surface in lightness, never to pure black or white.
    InteractionPalette {
        neutral_hover: 0x2A2C2F,
        neutral_pressed: 0x323539,
        primary_hover: 0xDAD6CE,
        primary_pressed: 0xC8C3BA,
        accent_hover: 0xF48A66,
        accent_pressed: 0xF59C7C,
        danger_hover: 0xF59599,
        danger_pressed: 0xF8ADB0,
        focus_border: 0xECE9E3,
        accent: 0xF0764E,
        selected_hover: 0x3A3D41,
        focus_ring: 0xF0764E,
    },
);

/// Apply the same palette to complete library components and our Base wrappers.
/// Initialize after gpui-component, before constructing any product windows.
pub(crate) fn init_component_theme(cx: &mut gpui::App) {
    use gpui::{px, rgb};
    let p = ZORK_UI.palette;
    let theme = gpui_component::Theme::global_mut(cx);
    theme.font_family = "Inter Variable".into();
    theme.font_size = px(13.);
    theme.mono_font_family = crate::assets::CODE_FONT_FAMILY.into();
    theme.radius = px(crate::controls::FIELD_RADIUS);
    theme.radius_lg = px(crate::controls::CARD_RADIUS);
    theme.background = rgb(p.canvas).into();
    theme.foreground = rgb(p.text).into();
    theme.border = rgb(FORM.outline).into();
    theme.input = rgb(FORM.outline).into();
    theme.ring = rgb(INTERACTION.focus_ring).into();
    theme.caret = rgb(p.text).into();
    theme.muted = rgb(p.selected).into();
    theme.muted_foreground = rgb(p.muted).into();
    theme.accent = rgb(INTERACTION.neutral_hover).into();
    theme.accent_foreground = rgb(p.text).into();
    theme.primary = rgb(p.text).into();
    theme.primary_foreground = rgb(p.canvas).into();
    theme.primary_hover = rgb(INTERACTION.primary_hover).into();
    theme.primary_active = rgb(INTERACTION.primary_pressed).into();
    theme.secondary = rgb(p.selected).into();
    theme.secondary_foreground = rgb(p.text).into();
    theme.popover = rgb(p.elevated).into();
    theme.popover_foreground = rgb(p.text).into();
    theme.colors.list = rgb(p.canvas).into();
    theme.list_hover = rgb(INTERACTION.neutral_hover).into();
    theme.list_active = rgb(p.selected).into();
    theme.list_active_border = rgb(FORM.focus_border).into();
    theme.slider_bar = rgb(p.text).into();
    theme.slider_thumb = rgb(p.canvas).into();
    theme.switch = rgb(FORM.switch_off).into();
    theme.switch_thumb = rgb(p.canvas).into();
    theme.button = rgb(p.canvas).into();
    theme.button_foreground = rgb(p.text).into();
    theme.button_hover = rgb(INTERACTION.neutral_hover).into();
    theme.button_active = rgb(INTERACTION.neutral_pressed).into();
    theme.button_primary = theme.primary;
    theme.button_primary_foreground = theme.primary_foreground;
    theme.button_primary_hover = theme.primary_hover;
    theme.button_primary_active = theme.primary_active;
    theme.danger = rgb(p.danger).into();
    theme.danger_foreground = rgb(p.canvas).into();
    theme.scrollbar_thumb = rgb(p.muted).into();
    theme.scrollbar_thumb_hover = rgb(p.text).into();
    // This pinned library still reads resolved component tokens in its widgets.
    theme.tokens = (&theme.colors).into();
    gpui_component::Theme::sync_base(cx);
}

/// Form and feedback colors extend the existing approved warm-white palette.
pub struct FormPalette {
    pub hover_border: u32,
    pub focus_border: u32,
    pub error_border: u32,
    pub error_focus_border: u32,
    pub error_surface: u32,
    pub success_surface: u32,
    pub warning_surface: u32,
    pub switch_off: u32,
    /// Resting outline of fields, outlined buttons and popovers.
    pub outline: u32,
    /// Text and glyphs of disabled controls; disabled surfaces use `palette.prompt`.
    pub disabled_text: u32,
}
pub static FORM: Themed<FormPalette> = Themed::new(
    FormPalette {
        hover_border: 0xA9A398,
        focus_border: 0x24272B,
        error_border: 0xC9837E,
        error_focus_border: 0xB42E35,
        error_surface: 0xFFFAFA,
        success_surface: 0xE6F2EA,
        warning_surface: 0xFBF1DE,
        switch_off: 0xCCC7BD,
        outline: 0xCCC7BD,
        disabled_text: 0xB9B5AD,
    },
    FormPalette {
        hover_border: 0x6A6E75,
        focus_border: 0xECE9E3,
        error_border: 0x9E4E52,
        error_focus_border: 0xF27E83,
        error_surface: 0x2A1E20,
        success_surface: 0x1B2D23,
        warning_surface: 0x332919,
        switch_off: 0x4A4E54,
        outline: 0x4A4E54,
        disabled_text: 0x5E6166,
    },
);

#[derive(Clone, Copy)]
pub enum TextRole {
    PageTitle,
    SectionTitle,
    Body,
    Label,
    Description,
    Metadata,
}
impl TextRole {
    pub fn metrics(self) -> (f32, f32, u16) {
        match self {
            Self::PageTitle => (19., 28., 600),
            Self::SectionTitle => (14., 22., 600),
            Self::Body => (13., 20., 400),
            Self::Label => (12., 18., 500),
            Self::Description => (12., 20., 400),
            Self::Metadata => (12., 16., 400),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TranscriptTreatment {
    PromptPill,
    PlainProse,
    InlineActivity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub window: u32,
    pub canvas: u32,
    pub sidebar: u32,
    pub sidebar_hover: u32,
    pub selected: u32,
    pub elevated: u32,
    pub prompt: u32,
    pub border: u32,
    pub border_strong: u32,
    pub text: u32,
    pub muted: u32,
    pub subtle: u32,
    pub accent: u32,
    pub success: u32,
    pub warning: u32,
    pub danger: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutSpec {
    pub rail_width: f32,
    pub window_bar_height: f32,
    pub window_inset: f32,
    pub panel_radius: f32,
    pub sidebar_width: f32,
    pub header_height: f32,
    pub transcript_max_width: f32,
    pub composer_width: f32,
    pub composer_height: f32,
    pub composer_bottom_inset: f32,
    pub has_global_status_bar: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ResolvedGeometry {
    pub rail: Rect,
    pub sidebar: Rect,
    pub main: Rect,
    pub header: Rect,
    pub transcript: Rect,
    pub composer: Rect,
}

impl LayoutSpec {
    pub fn resolve(self, width: f32, height: f32) -> Option<ResolvedGeometry> {
        self.resolve_with_sidebar(width, height, false)
    }

    pub fn resolve_with_sidebar(
        self,
        width: f32,
        height: f32,
        sidebar_open: bool,
    ) -> Option<ResolvedGeometry> {
        if width < 900.0 || height < 600.0 {
            return None;
        }

        let main = Rect {
            left: self.rail_width,
            top: self.window_bar_height,
            right: width
                - self.window_inset
                - if sidebar_open {
                    self.sidebar_width
                } else {
                    0.0
                },
            bottom: height - self.window_inset,
        };
        let rail = Rect {
            left: 0.0,
            top: main.top,
            right: self.rail_width,
            bottom: main.bottom,
        };
        let sidebar = Rect {
            left: main.right,
            top: main.top,
            right: width - self.window_inset,
            bottom: main.bottom,
        };
        let header = Rect {
            left: main.left,
            top: main.top,
            right: main.right,
            bottom: main.top + self.header_height,
        };
        let main_width = main.right - main.left;
        let composer_width = self.composer_width.min(main_width - 48.0);
        let composer_left = main.left + (main_width - composer_width) / 2.0;
        let composer = Rect {
            left: composer_left,
            top: main.bottom - self.composer_bottom_inset - self.composer_height,
            right: composer_left + composer_width,
            bottom: main.bottom - self.composer_bottom_inset,
        };
        let transcript_width = self.transcript_max_width.min(main_width - 48.0);
        let transcript_left = main.left + (main_width - transcript_width) / 2.0;
        let transcript = Rect {
            left: transcript_left,
            top: header.bottom + 24.0,
            right: transcript_left + transcript_width,
            bottom: composer.top - 20.0,
        };

        if transcript.bottom <= transcript.top {
            return None;
        }

        Some(ResolvedGeometry {
            rail,
            sidebar,
            main,
            header,
            transcript,
            composer,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TranscriptSpec {
    pub user: TranscriptTreatment,
    pub assistant: TranscriptTreatment,
    pub activity: TranscriptTreatment,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ComposerSpec {
    pub surface_radius: f32,
    pub minimum_editor_height: f32,
    pub home_max_editor_height: f32,
    pub thread_max_editor_height: f32,
    pub home_chrome_height: f32,
    pub thread_chrome_height: f32,
    pub action_size: f32,
    pub menu_width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SidebarSpec {
    pub toolbar_height: f32,
    pub footer_height: f32,
    pub inline_inset: f32,
    pub row_height: f32,
    pub row_radius: f32,
    pub item_font_size: f32,
    pub item_line_height: f32,
    pub section_label_font_size: f32,
    pub section_label_line_height: f32,
    pub section_label_weight: u16,
    pub selected_fill: u32,
    pub has_hard_divider: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HomeSpec {
    pub recommendations_fold_width: f32,
    pub tasks_fold_width: f32,
    pub compact_composer_height: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ThreadSpec {
    pub header_height: f32,
    pub title_font_size: f32,
    pub title_line_height: f32,
    pub title_weight: u16,
    pub assistant_font_size: f32,
    pub assistant_line_height: f32,
    pub content_left_inset: f32,
    pub aligns_followup_composer: bool,
    pub user_font_size: f32,
    pub user_line_height: f32,
    pub user_max_width: f32,
    pub user_left_clearance: f32,
    pub user_padding_x: f32,
    pub user_padding_y: f32,
    pub user_radius: f32,
    pub user_fill: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskRowSpec {
    pub group_by_workspace: bool,
    pub selected_uses_fill: bool,
    pub draw_card_borders: bool,
    pub show_model_suffix: bool,
    pub show_leading_status_dot: bool,
    pub workspace_uses_icon_asset: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ZorkUiSpec {
    pub force_light_window_chrome: bool,
    pub palette: Palette,
    pub layout: LayoutSpec,
    pub transcript: TranscriptSpec,
    pub composer: ComposerSpec,
    pub sidebar: SidebarSpec,
    pub home: HomeSpec,
    pub thread: ThreadSpec,
    pub task_rows: TaskRowSpec,
}

pub const LIGHT_UI: ZorkUiSpec = ZorkUiSpec {
    force_light_window_chrome: true,
    // sRGB values sampled from the approved HTML design tokens in a browser.
    palette: Palette {
        window: 0xF4F2ED,
        canvas: 0xFFFFFF,
        sidebar: 0xF4F2ED,
        sidebar_hover: 0xEDEAE3,
        selected: 0xE4DFD4,
        elevated: 0xFFFFFF,
        prompt: 0xF1EEE8,
        border: 0xE7E3DB,
        border_strong: 0xD8D3C9,
        text: 0x24272B,
        muted: 0x575C62,
        subtle: 0x676C72,
        accent: 0x24272B,
        success: 0x1F7A4D,
        warning: 0x935800,
        danger: 0xB42E35,
    },
    layout: LayoutSpec {
        rail_width: 75.0,
        window_bar_height: 42.0,
        window_inset: 8.0,
        panel_radius: 12.0,
        sidebar_width: 300.0,
        header_height: 44.0,
        transcript_max_width: 744.0,
        composer_width: 744.0,
        composer_height: 102.0,
        composer_bottom_inset: 16.0,
        has_global_status_bar: false,
    },
    transcript: TranscriptSpec {
        user: TranscriptTreatment::PromptPill,
        assistant: TranscriptTreatment::PlainProse,
        activity: TranscriptTreatment::InlineActivity,
    },
    composer: ComposerSpec {
        surface_radius: RADIUS.container,
        minimum_editor_height: 40.0,
        home_max_editor_height: 120.0,
        thread_max_editor_height: 60.0,
        home_chrome_height: 74.0,
        thread_chrome_height: 74.0,
        action_size: crate::controls::IconButtonSize::Small.extent(),
        menu_width: 353.0,
    },
    sidebar: SidebarSpec {
        toolbar_height: 48.0,
        footer_height: 46.0,
        inline_inset: 8.0,
        row_height: 32.0,
        row_radius: RADIUS.control,
        item_font_size: 13.0,
        item_line_height: 20.0,
        section_label_font_size: 12.0,
        section_label_line_height: 18.0,
        section_label_weight: 500,
        selected_fill: 0xE4DFD4,
        has_hard_divider: false,
    },
    home: HomeSpec {
        recommendations_fold_width: 621.0,
        tasks_fold_width: 917.0,
        compact_composer_height: 50.0,
    },
    thread: ThreadSpec {
        header_height: 44.0,
        title_font_size: 13.0,
        title_line_height: 20.0,
        title_weight: 400,
        assistant_font_size: 13.0,
        assistant_line_height: 20.0,
        content_left_inset: 24.0,
        aligns_followup_composer: true,
        user_font_size: 13.0,
        user_line_height: 20.0,
        user_max_width: 500.0,
        user_left_clearance: 42.0,
        user_padding_x: 18.0,
        user_padding_y: 11.0,
        user_radius: RADIUS.bubble,
        user_fill: 0xF1EEE8,
    },
    task_rows: TaskRowSpec {
        group_by_workspace: true,
        selected_uses_fill: true,
        draw_card_borders: false,
        show_model_suffix: false,
        show_leading_status_dot: false,
        workspace_uses_icon_asset: true,
    },
};

/// Dark shares every geometry of light; only colors change.
pub const DARK_UI: ZorkUiSpec = ZorkUiSpec {
    force_light_window_chrome: false,
    palette: Palette {
        window: 0x18191B,
        canvas: 0x1F2023,
        sidebar: 0x18191B,
        sidebar_hover: 0x27292C,
        selected: 0x303236,
        elevated: 0x26282B,
        prompt: 0x2B2D31,
        border: 0x2E3034,
        border_strong: 0x3A3D42,
        text: 0xECE9E3,
        muted: 0xB7B3AB,
        subtle: 0x9C988F,
        accent: 0xECE9E3,
        success: 0x5FC08C,
        warning: 0xE3A84A,
        danger: 0xF27E83,
    },
    sidebar: SidebarSpec {
        selected_fill: 0x303236,
        ..LIGHT_UI.sidebar
    },
    thread: ThreadSpec {
        user_fill: 0x2B2D31,
        ..LIGHT_UI.thread
    },
    ..LIGHT_UI
};
/// The current theme's specification.
pub static ZORK_UI: Themed<ZorkUiSpec> = Themed::new(LIGHT_UI, DARK_UI);

pub const LEADER_SIDEBAR_WIDTH: f32 = 264.;

/// Unified desktop device navigation; the legacy station shell keeps its rail.
pub const DEVICE_SIDEBAR_WIDTH: f32 = 240.;
