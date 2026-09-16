//! Transient send-intent visuals. Hosts decide whether an intent was accepted.
//! This is an animation queue, not a message cache or delivery state.
use super::{Pose, Simulation, Surface};
use gpui::SharedString;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub enum Origin {
    #[default]
    Button,
    Composer,
}
struct Flight {
    id: u64,
    origin: Origin,
    text: SharedString,
    age: f64,
    exiting: bool,
    accent: Option<Surface>,
    color_revision: u64,
}
#[derive(Default)]
pub struct Departures {
    flights: Vec<Flight>,
    base: usize,
    sequence: u64,
    render_errors: usize,
}
pub struct Bubble<'a> {
    pub id: u64,
    pub pose: Pose,
    pub text: SharedString,
    pub opacity: f32,
    pub text_width: f32,
    pub accent: Option<&'a Surface>,
    pub accent_opacity: f32,
}
impl Departures {
    pub(super) fn rebase(&mut self, base: usize) { self.base = base; }
    pub fn emit(&mut self, origin: Origin, text: &str, body: Pose, sim: &mut Simulation) {
        // Bound transient effects independently of accepted business operations.
        if self.flights.len() == 4 {
            sim.remove_compound_group(self.base);
            self.flights.remove(0);
        }
        let source = match origin {
            Origin::Button => Pose::rect(
                body.left() + body.w - 30.,
                body.top() + body.h - 30.,
                24.,
                24.,
                12.,
            ),
            Origin::Composer => body,
        };
        let index = sim.append_compound_group(source);
        if self.flights.is_empty() {
            self.base = index;
        }
        self.sequence += 1;
        self.flights.push(Flight {
            id: self.sequence,
            origin,
            text: if text.trim().is_empty() {
                "附件".into()
            } else {
                text.chars().take(160).collect::<String>().into()
            },
            age: 0.,
            exiting: false,
            accent: None,
            color_revision: u64::MAX,
        });
    }
    pub fn targets(&self, body: Pose) -> impl Iterator<Item = Pose> + '_ {
        self.flights.iter().map(move |f| {
            let width = (body.w - 20.).clamp(120., 210.);
            Pose::rect(
                body.left() + body.w - width,
                body.top() - if f.exiting { 260. } else { 130. },
                width,
                52.,
                18.,
            )
        })
    }
    pub fn advance(&mut self, elapsed: f64, reduced: bool, sim: &mut Simulation) -> bool {
        let mut changed = false;
        for i in (0..self.flights.len()).rev() {
            let f = &mut self.flights[i];
            f.age += elapsed;
            if reduced || (f.exiting && f.age >= 1.5) {
                sim.remove_compound_group(self.base + i);
                self.flights.remove(i);
                changed = true;
            } else if !f.exiting && f.age >= 0.85 {
                // Briefly leave the submitted text readable before it exits
                // the preview. Travel itself remains the material spring.
                f.exiting = true;
                changed = true;
            }
        }
        changed
    }
    pub fn active(&self) -> bool {
        !self.flights.is_empty()
    }
    /// Cache the colored projection for a material revision. A temporary trace
    /// failure retains the prior color path (or the base fill), never aborts the
    /// application from a render pass. The authoritative contour is unchanged.
    pub fn prepare(&mut self, sim: &Simulation) {
        for (i, flight) in self.flights.iter_mut().enumerate() {
            if flight.origin != Origin::Button || flight.age >= 0.32 {
                flight.accent = None;
                continue;
            }
            if flight.color_revision == sim.revision {
                continue;
            }
            flight.color_revision = sim.revision;
            let projection = sim.group_snapshot(self.base + i);
            if let Some(surface) = &mut flight.accent {
                surface.simulation = projection;
                surface.prepare();
                self.render_errors += usize::from(surface.last_error.is_some());
            } else {
                match Surface::new(projection) {
                    Ok(surface) => flight.accent = Some(surface),
                    Err(_) => self.render_errors += 1,
                }
            }
        }
    }
    pub fn render_errors(&self) -> usize {
        self.render_errors
    }
    pub fn bubbles(&self, body: Pose, sim: &Simulation) -> Vec<Bubble<'_>> {
        self.flights
            .iter()
            .enumerate()
            .map(|(i, f)| {
                let pose = sim.group_pose(self.base + i);
                Bubble {
                    id: f.id,
                    pose,
                    text: f.text.clone(),
                    opacity: (((body.top() - pose.top()) / 36.).clamp(0., 1.)
                        * ((pose.w - 40.) / 80.).clamp(0., 1.)) as f32,
                    // Shape text for its final line width; the current material
                    // clips/reveals it instead of rewrapping a long string for
                    // every subpixel change of the spring width.
                    text_width: ((sim.target_pose().w - 20.).clamp(120., 210.) - 24.) as f32,
                    accent: f.accent.as_ref(),
                    accent_opacity: (1. - f.age / 0.32).clamp(0., 1.) as f32,
                }
            })
            .collect()
    }
    #[cfg(feature = "stories")]
    pub fn inspect(&self, sim: &Simulation) -> serde_json::Value {
        serde_json::json!({"renderErrors":self.render_errors,"emitted":self.sequence,"flights":self.flights.iter().enumerate().map(|(i,f)|
            serde_json::json!({"id":f.id,"origin":f.origin,"age":f.age,"exiting":f.exiting,"pose":sim.group_pose(self.base+i)})).collect::<Vec<_>>()})
    }
}
