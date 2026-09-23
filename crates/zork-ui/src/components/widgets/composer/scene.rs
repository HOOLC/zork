//! Static composer bounds shared by product and design fixtures.
use super::super::Pose;

#[derive(Default)]
pub struct Scene {
    body: Option<Pose>,
}

impl Scene {
    pub fn moving(&self) -> bool {
        false
    }
    pub fn body(&self) -> Pose {
        self.body.expect("composer frame must precede render")
    }
    pub fn inspect(&self) -> serde_json::Value {
        serde_json::json!({"moving":false,"body":self.body})
    }
    pub fn frame(&mut self, body: Pose) {
        self.body = Some(body);
    }
}
