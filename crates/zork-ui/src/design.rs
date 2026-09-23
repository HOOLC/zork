//! Shared desktop design. Contract: docs/design/interface.md.
//! Hallmark · Workbench · modern-minimal · native GPUI tokens.
//!
//! Rendering stays in `views`, but the durable palette, hierarchy, and default
//! geometry live here so regressions do not silently turn the app back into a
//! generic dashboard.

pub const BRAND_ACCENT: u32 = 0xE9643B;
pub const UI_OUTLINE: u32 = 0xCCC7BD;
/// Keyboard focus is an independent persimmon ring outside the outline.
pub const FOCUS_RING: u32 = 0xE9643B;
pub const FOCUS_RING_ALPHA: f32 = 0.6;
/// Text and glyphs of disabled controls; disabled surfaces use `palette.prompt`.
pub const DISABLED_TEXT: u32 = 0xB9B5AD;
/// A selected row keeps its identity under the pointer: hover deepens it.
pub const SELECTED_HOVER: u32 = 0xDCD6CA;

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
pub const DEVICE_HUES: [u32; 5] = [0x5E8B6B, 0x56759A, 0xA27A2B, 0x87618F, 0x3E8787];
pub fn device_hue(key: &str) -> u32 {
    let hash = key
        .bytes()
        .fold(0x811C9DC5u32, |hash, byte| (hash ^ byte as u32).wrapping_mul(0x0100_0193));
    DEVICE_HUES[hash as usize % DEVICE_HUES.len()]
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
}
pub const INTERACTION: InteractionPalette = InteractionPalette {
    neutral_hover: 0xEDEAE3,
    neutral_pressed: 0xE5E1D8,
    primary_hover: 0x3B3F44,
    primary_pressed: 0x4C5157,
    accent_hover: 0xDB572F,
    accent_pressed: 0xC84A27,
    danger_hover: 0x9E252C,
    danger_pressed: 0x8A1D23,
    focus_border: 0x24272B,
};

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
    theme.border = rgb(UI_OUTLINE).into();
    theme.input = rgb(UI_OUTLINE).into();
    theme.ring = rgb(FOCUS_RING).into();
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
    theme.popover = rgb(p.canvas).into();
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
}
pub const FORM: FormPalette = FormPalette {
    hover_border: 0xA9A398,
    focus_border: INTERACTION.focus_border,
    error_border: 0xC9837E,
    error_focus_border: ZORK_UI.palette.danger,
    error_surface: 0xFFFAFA,
    success_surface: 0xE6F2EA,
    warning_surface: 0xFBF1DE,
    switch_off: 0xCCC7BD,
};

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
            Self::Metadata => (11., 17., 400),
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

pub const ZORK_UI: ZorkUiSpec = ZorkUiSpec {
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
        user_padding_x: 16.0,
        user_padding_y: 10.0,
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

pub const LEADER_SIDEBAR_WIDTH: f32 = 264.;

/// Unified desktop device navigation; the legacy station shell keeps its rail.
pub const DEVICE_SIDEBAR_WIDTH: f32 = 240.;
