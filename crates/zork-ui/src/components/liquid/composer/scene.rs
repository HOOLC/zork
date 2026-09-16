//! Stable material parcels for a production-sized composer. The host supplies
//! sampled presentation poses; accepted sends are explicit transient events.
use super::*;
use super::super::{departure::{Departures, Origin}, Material, Options, Simulation};

#[derive(Default)]
pub struct Scene {
    pub surface: Option<Surface>,
    members: Vec<String>,
    departures: Departures,
    pending: Vec<(Origin, String)>,
    layout_anchor: Option<[f64; 2]>,
}
impl Scene {
    pub fn moving(&self) -> bool {
        !self.pending.is_empty() || self.departures.active()
            || self.surface.as_ref().is_some_and(|surface| surface.simulation.moving())
    }
    pub fn inspect(&self) -> serde_json::Value {
        self.surface.as_ref().map_or(serde_json::Value::Null, |surface| {
            let sim = &surface.simulation;
            serde_json::json!({"moving":sim.moving(),"revision":sim.revision,"members":self.members,
                "groups":(0..self.members.len()+1).map(|i| {
                    let group = sim.group_snapshot(i);
                    serde_json::json!({"pose":group.pose(),"target":group.target_pose(),"moving":group.moving(),
                        "max_velocity":group.particles().map(|p| p.vx.hypot(p.vy)).fold(0.,f64::max)})
                }).collect::<Vec<_>>()})
        })
    }
    pub fn accepted(&mut self, origin: Origin, text: &str) {
        if self.pending.len() == 4 { self.pending.remove(0); }
        self.pending.push((origin, text.chars().take(160).collect()));
    }
    pub fn indices(&self, ids: impl IntoIterator<Item = impl AsRef<str>>) -> Vec<usize> {
        ids.into_iter().map(|id| self.members.iter().position(|member| member == id.as_ref()).unwrap() + 1).collect()
    }
    pub fn bubbles(&self) -> Vec<super::super::departure::Bubble<'_>> {
        self.surface.as_ref().map_or_else(Vec::new, |surface|
            self.departures.bubbles(surface.simulation.pose(), &surface.simulation))
    }
    pub fn frame(&mut self, body: Pose, members: &[(String, Pose)], opening: Option<fan_geometry::Opening>, elapsed: f64, reduced: bool) -> bool {
        let anchor = [body.cx, body.top() + body.h];
        if self.surface.is_none() {
            self.layout_anchor = Some(anchor);
            self.members = members.iter().map(|m| m.0.clone()).collect();
            let poses: Vec<_> = std::iter::once(body).chain(members.iter().map(|m| m.1)).collect();
            self.surface = Surface::new(Simulation::compound(&poses, 4., Material::default(), Options { anchor: [0., 1.], ..Default::default() })).ok();
            if let Some(surface) = &mut self.surface { surface.simulation.finish(); surface.prepare(); }
        }
        let Some(surface) = &mut self.surface else { return false; };
        if let Some(previous) = self.layout_anchor.replace(anchor) {
            surface.simulation.translate([anchor[0] - previous[0], anchor[1] - previous[1]]);
        }
        // Deletion and insertion preserve every surviving parcel's state.
        for i in (0..self.members.len()).rev() {
            if !members.iter().any(|m| m.0 == self.members[i]) {
                surface.simulation.remove_compound_group(i + 1);
                self.members.remove(i);
            }
        }
        for (id, _) in members {
            if !self.members.contains(id) {
                let index = self.members.len() + 1;
                surface.simulation.insert_compound_group(index, Pose::rect(body.left() + 24., body.top() + 8., 32., 32., 16.));
                self.members.push(id.clone());
            }
        }
        self.departures.rebase(self.members.len() + 1);
        if !reduced {
            for (origin, text) in self.pending.drain(..) { self.departures.emit(origin, &text, body, &mut surface.simulation); }
        } else { self.pending.clear(); }
        self.departures.advance(elapsed, reduced, &mut surface.simulation);
        let poses: Vec<_> = std::iter::once(body)
            .chain(self.members.iter().map(|id| members.iter().find(|m| &m.0 == id).unwrap().1))
            .chain(self.departures.targets(body)).collect();
        surface.simulation.set_compound_targets(&poses);
        if surface.simulation.moving() { surface.simulation.advance(elapsed.min(0.05), reduced); }
        surface.set_cutouts(opening.into_iter().map(|opening| opening.hole.into_iter().map(|curve| super::super::Cubic {
            from: [curve[0].x as f64, curve[0].y as f64], c1: [curve[1].x as f64, curve[1].y as f64],
            c2: [curve[2].x as f64, curve[2].y as f64], to: [curve[3].x as f64, curve[3].y as f64],
        }).collect()).collect());
        surface.prepare();
        self.departures.prepare(&surface.simulation);
        surface.simulation.moving() || self.departures.active()
    }
}
