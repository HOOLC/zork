//! Static rectangle geometry for layouts that place GPUI children absolutely.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize)]
pub struct Pose {
    pub cx: f64,
    pub cy: f64,
    pub w: f64,
    pub h: f64,
    pub r: f64,
}
impl Pose {
    pub fn rect(x: f64, y: f64, w: f64, h: f64, r: f64) -> Self {
        Self {
            cx: x + w / 2.,
            cy: y + h / 2.,
            w,
            h,
            r,
        }
    }
    pub fn left(self) -> f64 {
        self.cx - self.w / 2.
    }
    pub fn top(self) -> f64 {
        self.cy - self.h / 2.
    }
}
