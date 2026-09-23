//! Draft file previews follow the current core snapshot without shape transitions.
use super::fan::FanState;
use std::time::Instant;
use zork_client_core::files::FileRef;
use zork_ui::components::attachment_fan as geometry;

#[derive(Clone)]
pub(in crate::views) struct VisualFile {
    pub file: FileRef,
}
#[derive(Clone)]
pub(in crate::views) struct Frame {
    pub files: Vec<VisualFile>,
    pub expanded: f32,
    pub width: f32,
    pub height: f32,
}
impl Frame {
    pub fn settled(files: &[FileRef], expanded: f32, available: f32) -> Self {
        let (width, height) = geometry::dimensions(files.len(), available);
        Self {
            files: files
                .iter()
                .cloned()
                .map(|file| VisualFile { file })
                .collect(),
            expanded,
            width,
            height,
        }
    }
}

#[derive(Default)]
pub(in crate::views) struct DraftFiles {
    current: Vec<FileRef>,
}
impl DraftFiles {
    pub fn reset(&mut self, files: &[FileRef]) {
        self.current = files.to_vec();
    }
    pub fn frame(&self, expanded: f32, available: f32) -> Frame {
        Frame::settled(&self.current, expanded, available)
    }
    pub fn advance(
        &mut self,
        desired: &[FileRef],
        fan: &mut FanState,
        _: Instant,
        _: bool,
        _: f32,
    ) -> bool {
        if self.current != desired {
            self.current = desired.to_vec();
            if self.current.is_empty() {
                *fan = FanState::default();
            }
        }
        false
    }
}
