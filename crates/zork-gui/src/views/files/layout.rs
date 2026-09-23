//! Draft file previews follow the current core snapshot without shape transitions.
use zork_client_core::files::FileRef;
use zork_ui::components::attachment_fan as geometry;

#[derive(Clone)]
pub(in crate::views) struct VisualFile {
    pub file: FileRef,
}
#[derive(Clone)]
pub(in crate::views) struct Frame {
    pub files: Vec<VisualFile>,
    pub expanded: bool,
    pub width: f32,
    pub height: f32,
}
impl Frame {
    pub fn settled(files: &[FileRef], expanded: bool, available: f32) -> Self {
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
