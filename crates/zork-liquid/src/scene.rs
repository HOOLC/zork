//! Batched visual transport. Native-endian 32-bit words, versioned explicitly.
//! A scene belongs to one visible platform host. No business state or clocks.
//!
//! Commands (24 words): id, kind, flags, parent; from(x,y,w,h,r),
//! to(x,y,w,h,r), anchor(x,y), count, selected, smoothing, border; reserved×4.
//! Flags: visible=1, active=2, snap=4, completed tap=8, resend geometry=16.
//! Kind 7 removes an id. Kind 9 is a compound body; kind 10 retargets a
//! stable member of parent, spawning at from and moving to to. Member output
//! uses the same record header without paths, preserving identity on reorder.
//! Frame: version, record count, moving count, error count. Each record starts
//! with id, flags, byteLength, fillLoops, borderLoops, activeLoops, trackLoops,
//! x, y, width, height, progress.
//! Record flags: geometryChanged=1, moving=2, pressure feedback=4, hidden=8.
//! Fill loops: cubic count, start(x,y), then c1/c2/end. Border loops: point
//! count then x/y. Paths are local to the returned x/y placement.
//!
//! Version 3: four words follow progress, before any paths: content opacity,
//! backdrop reveal, actual expansion (all f32), and presentation flags (alive=1).
//! These values apply to Morph/Pair nodes; other records contain zeroes. The
//! backdrop value is normalized, so a host applies its modal scrim color once.
//! Material progress must not substitute for either independent reveal value.
//!
//! IDs remain stable through retargeting and reversal. Output is a delta: retain
//! omitted nodes and paths, update placement without rebuilding unchanged paths,
//! and discard a node when its remove command is submitted. Hidden records end
//! presentation without transmitting paths. Hosts stop requesting frames when
//! moving is zero and there are no new commands; background time is not replayed.
//! `pending()` borrows the last encoded result for buffer-growth retries, without
//! applying commands or advancing time again. Protocols are process-local,
//! native-endian transports, not persistent or network serialization formats.
use crate::{
    border,
    motion::{DelayedReveal, Press, Reveal, Transition},
    recipes, Contour, Material, Options, Pose, Simulation, Surface,
};
use rustc_hash::{FxHashMap, FxHashSet};
use std::rc::Rc;

pub const VERSION: u32 = 3;
pub const COMMAND_BYTES: usize = 96;
pub const MAX_NODES: usize = 256;
pub const MAX_COMMANDS: usize = 2048;
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Command,
    Capacity,
    Geometry,
}

#[derive(Clone, Copy)]
struct Update {
    id: u32,
    kind: u32,
    flags: u32,
    parent: u32,
    from: Pose,
    to: Pose,
    anchor: [f64; 2],
    count: usize,
    selected: usize,
    smoothing: f64,
    border: f32,
}
impl Update {
    fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let word = |i: usize| u32::from_ne_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap());
        let number = |i| f32::from_bits(word(i)) as f64;
        let pose = |i| {
            Pose::rect(
                number(i),
                number(i + 1),
                number(i + 2),
                number(i + 3),
                number(i + 4),
            )
        };
        let kind = word(1);
        if word(0) == 0
            || kind > 10
            || word(2) & !31 != 0
            || kind == 10 && (word(3) == 0 || word(3) == word(0))
        {
            return Err(Error::Command);
        }
        if kind != 7
            && ((4..16).any(|i| !number(i).is_finite()) || number(8) < 0. || number(13) < 0.)
        {
            return Err(Error::Command);
        }
        let update = Self {
            id: word(0),
            kind,
            flags: word(2),
            parent: word(3),
            from: pose(4),
            to: pose(9),
            anchor: [number(14), number(15)],
            count: word(16) as usize,
            selected: word(17) as usize,
            smoothing: number(18),
            border: number(19) as f32,
        };
        if kind == 7 {
            return Ok(update);
        }
        // Bound sizes before any simulation or tracing allocation.
        if [4, 9].into_iter().any(|i| {
            !(2. ..=4096.).contains(&number(i + 2)) || !(2. ..=4096.).contains(&number(i + 3))
        }) || !update.from.is_valid()
            || !update.to.is_valid()
            || [number(4), number(5), number(9), number(10)]
                .into_iter()
                .any(|v| v.abs() > 16384.)
            || !(0. ..=1.).contains(&update.smoothing)
            || !(0. ..=8.).contains(&update.border)
            || update.anchor.into_iter().any(|v| !(0. ..=1.).contains(&v))
            || kind == 3 && (!(1..=64).contains(&update.count) || update.selected >= update.count)
        {
            return Err(Error::Command);
        }
        Ok(update)
    }
    fn active(self) -> bool {
        self.flags & 2 != 0
    }
    fn visible(self) -> bool {
        self.flags & 1 != 0
    }
    fn material(self) -> Material {
        Material {
            smoothing: self.smoothing,
            ..Material::default()
        }
    }
    fn target(self) -> Pose {
        match self.kind {
            3 => {
                let mut pose =
                    recipes::segment_pose(self.from.w, self.from.h, self.count, self.selected);
                pose.cx += self.from.left();
                pose.cy += self.from.top();
                pose
            }
            4 => {
                let mut pose = recipes::toggle_pose(self.active());
                pose.cx += self.from.left();
                pose.cy += self.from.top();
                pose
            }
            8 => recipes::slider_target(self.from, self.anchor[0], false, self.active()),
            _ => self.from,
        }
    }
}

struct Node {
    update: Update,
    surface: Option<Surface>,
    press: Press,
    transition: Transition,
    presentation: Option<OverlayPresentation>,
    cached: Option<(Rc<Contour>, [f64; 2], f32)>,
    static_contour: Option<Rc<Contour>>,
    dirty: bool,
    moving: bool,
    track_cached: Option<Pose>,
    members: Vec<u32>,
}

#[derive(Default)]
struct OverlayPresentation {
    content: Reveal,
    backdrop: DelayedReveal,
    expansion: f32,
    alive: bool,
}
impl OverlayPresentation {
    fn values(&self) -> [u32; 4] {
        [
            self.content.opacity().to_bits(),
            self.backdrop.opacity().to_bits(),
            self.expansion.to_bits(),
            u32::from(self.alive),
        ]
    }
}
impl Node {
    fn new(update: Update) -> Result<Self, Error> {
        let simulation = match update.kind {
            0 | 1 => None,
            5 | 6 => Some(Transition::simulation(
                update.from,
                update.to,
                update.kind == 6,
                update.material(),
                update.anchor,
            )),
            9 => {
                let mut sim =
                    Simulation::compound(&[update.from], 4., update.material(), Options::default());
                sim.finish();
                Some(sim)
            }
            _ => {
                let mut sim =
                    Simulation::new(update.target(), update.material(), Options::default());
                sim.finish();
                Some(sim)
            }
        };
        let surface = simulation
            .map(Surface::new)
            .transpose()
            .map_err(|_| Error::Geometry)?;
        let mut node = Self {
            update,
            surface,
            press: Press::default(),
            transition: Transition::default(),
            presentation: matches!(update.kind, 5 | 6).then(OverlayPresentation::default),
            cached: None,
            static_contour: None,
            dirty: true,
            moving: false,
            track_cached: None,
            members: Vec::new(),
        };
        node.apply(update);
        Ok(node)
    }
    fn apply(&mut self, update: Update) {
        if update.flags & 16 != 0 {
            self.cached = None;
            self.track_cached = None;
        }
        if update.kind == 1 && self.surface.is_none() && (update.active() || update.flags & 8 != 0)
        {
            let mut simulation =
                Simulation::new(update.from, update.material(), Options::default());
            simulation.finish();
            // Validated single rectangles have an exact contour fast path.
            self.surface = Surface::new(simulation).ok();
        }
        if let Some(surface) = &mut self.surface {
            if surface.simulation.material() != update.material() {
                surface.simulation.configure(update.material());
            }
            match update.kind {
                1 => {
                    if self.update.from != update.from {
                        self.press.reset();
                        surface.simulation.set_target(update.from);
                        surface.simulation.finish();
                    }
                    if update.flags & 8 != 0 && !self.press.down {
                        self.press.set(
                            &mut surface.simulation,
                            update.from,
                            true,
                            Some(update.anchor),
                            false,
                        );
                    }
                    self.press.set(
                        &mut surface.simulation,
                        update.from,
                        update.active(),
                        Some(update.anchor),
                        update.flags & 8 != 0,
                    );
                }
                5 | 6 => self.transition.target(
                    &mut surface.simulation,
                    update.from,
                    update.to,
                    update.kind == 6,
                    update.active(),
                    update.material(),
                    update.visible(),
                ),
                9 => surface.simulation.set_compound_group_target(0, update.from),
                _ => {
                    if surface.simulation.target_pose() != update.target() {
                        surface.simulation.set_target(update.target());
                    }
                }
            }
            if update.flags & 4 != 0 || !update.visible() {
                if update.kind == 1 {
                    self.press.reset();
                    surface.simulation.set_target(update.from);
                }
                if matches!(update.kind, 5 | 6) {
                    self.transition.advance(&mut surface.simulation, 0., true);
                } else if surface.simulation.moving() {
                    surface.simulation.finish();
                }
            }
        } else if self.update.from != update.from || self.update.smoothing != update.smoothing {
            self.static_contour = None;
        }
        self.dirty = true;
        self.update = update;
    }
    fn advance(&mut self, elapsed: f64, reduced: bool) -> bool {
        let was_moving = self.moving;
        self.moving = false;
        if !self.update.visible() {
            if let Some(presentation) = &mut self.presentation {
                *presentation = OverlayPresentation::default();
            }
            return false;
        }
        let Some(surface) = &mut self.surface else {
            return false;
        };
        let progress_before = self.transition.progress();
        let mut moving = match self.update.kind {
            1 => self
                .press
                .advance(&mut surface.simulation, self.update.from, elapsed, reduced),
            5 | 6 => self
                .transition
                .advance(&mut surface.simulation, elapsed, reduced),
            8 => {
                if surface.simulation.moving() {
                    surface
                        .simulation
                        .advance(elapsed * recipes::SLIDER_PLAYBACK_RATE, reduced);
                }
                surface.simulation.moving()
            }
            _ => {
                if surface.simulation.moving() {
                    surface.simulation.advance(elapsed, reduced);
                }
                surface.simulation.moving()
            }
        };
        if let Some(presentation) = &mut self.presentation {
            let before = presentation.values();
            let open = self.update.active();
            let reduced = reduced || self.update.flags & 4 != 0;
            presentation.expansion = self.transition.expansion(&surface.simulation) as f32;
            moving |= presentation.content.advance(open, elapsed, reduced);
            moving |= presentation.backdrop.advance(
                open,
                presentation.expansion >= 0.5,
                elapsed,
                reduced,
            );
            presentation.alive = open
                || self.transition.alive(&surface.simulation)
                || presentation.content.opacity() > 0.
                || presentation.backdrop.opacity() > 0.;
            self.dirty |= before != presentation.values() || was_moving != moving;
        }
        self.dirty |= surface.prepare() || progress_before != self.transition.progress();
        self.moving = moving;
        moving
    }
    fn contour(&mut self) -> Rc<Contour> {
        if let Some(surface) = &self.surface {
            surface.contour()
        } else {
            self.static_contour
                .get_or_insert_with(|| {
                    Rc::new(Contour {
                        loops: vec![crate::rounded_rectangle(
                            self.update.from,
                            self.update.smoothing,
                        )],
                        sampled_points: 0,
                        full_grid_points: 0,
                        used_fallback: false,
                        revision: 0,
                    })
                })
                .clone()
        }
    }
    fn member(&mut self, update: Update) {
        let simulation = &mut self.surface.as_mut().unwrap().simulation;
        let index = if let Some(index) = self.members.iter().position(|id| *id == update.id) {
            index + 1
        } else {
            self.members.push(update.id);
            simulation.append_compound_group(update.from)
        };
        simulation.set_compound_group_target(index, update.to);
        if update.flags & 4 != 0 || !self.update.visible() {
            simulation.finish();
        }
        self.dirty = true;
    }
    fn remove_member(&mut self, id: u32) {
        if let Some(index) = self.members.iter().position(|member| *member == id) {
            self.members.remove(index);
            self.surface
                .as_mut()
                .unwrap()
                .simulation
                .remove_compound_group(index + 1);
            self.dirty = true;
        }
    }
    fn write(&mut self, output: &mut Vec<u8>) -> Result<u32, Error> {
        if !self.dirty {
            return Ok(0);
        }
        if !self.update.visible() {
            // A hidden terminal update releases platform exit/focus lifetimes
            // without tracing or transmitting geometry that will not be drawn.
            for value in [self.update.id, 8, 64, 0, 0, 0, 0] {
                word(output, value);
            }
            for _ in 0..5 {
                scalar(output, 0.);
            }
            for _ in 0..4 {
                word(output, 0);
            }
            self.dirty = false;
            return Ok(1);
        }
        let contour = self.contour();
        let pose = self
            .surface
            .as_ref()
            .map_or(self.update.from, |s| s.simulation.pose());
        let offset = [pose.left(), pose.top()];
        let changed = self.cached.as_ref().is_none_or(|(old, origin, width)| {
            *width != self.update.border || !same_geometry(old, *origin, &contour, offset)
        });
        let border = if changed && self.update.border > 0. {
            border::outline(&contour, self.update.border).ok_or(Error::Geometry)?
        } else {
            Vec::new()
        };
        let start = output.len();
        let flags = u32::from(changed)
            | (u32::from(self.moving) << 1)
            | (u32::from(self.press.down || self.press.release_pending) << 2);
        let slider = self.update.kind == 8;
        let track = slider && self.track_cached != Some(self.update.from);
        for value in [
            self.update.id,
            flags,
            0,
            if changed {
                contour.loops.len() as u32
            } else {
                0
            },
            border.len() as u32,
            u32::from(slider),
            u32::from(track),
        ] {
            word(output, value);
        }
        scalar(output, offset[0]);
        scalar(output, offset[1]);
        scalar(output, pose.w);
        scalar(output, pose.h);
        scalar(output, self.transition.progress());
        for value in self
            .presentation
            .as_ref()
            .map_or([0; 4], OverlayPresentation::values)
        {
            word(output, value);
        }
        if changed {
            for curves in &contour.loops {
                word(output, curves.len() as u32);
                let from = curves.first().ok_or(Error::Geometry)?.from;
                scalar(output, from[0] - offset[0]);
                scalar(output, from[1] - offset[1]);
                for c in curves {
                    for p in [c.c1, c.c2, c.to] {
                        scalar(output, p[0] - offset[0]);
                        scalar(output, p[1] - offset[1]);
                    }
                }
            }
            for points in border {
                word(output, points.len() as u32);
                for p in points {
                    scalar(output, p[0] as f64 - offset[0]);
                    scalar(output, p[1] as f64 - offset[1]);
                }
            }
        }
        for shape in [
            slider.then(|| recipes::slider_fill(self.update.from, pose)),
            track.then(|| recipes::slider_track(self.update.from)),
        ]
        .into_iter()
        .flatten()
        {
            let curves = crate::rounded_rectangle(shape, self.update.smoothing);
            word(output, curves.len() as u32);
            scalar(output, curves[0].from[0]);
            scalar(output, curves[0].from[1]);
            for c in curves {
                for p in [c.c1, c.c2, c.to] {
                    scalar(output, p[0]);
                    scalar(output, p[1]);
                }
            }
        }
        if track {
            self.track_cached = Some(self.update.from);
        }
        let bytes = (output.len() - start) as u32;
        output[start + 8..start + 12].copy_from_slice(&bytes.to_ne_bytes());
        for (index, id) in self.members.iter().enumerate() {
            let p = self
                .surface
                .as_ref()
                .unwrap()
                .simulation
                .group_pose(index + 1);
            for value in [*id, u32::from(self.moving) << 1, 64, 0, 0, 0, 0] {
                word(output, value);
            }
            for value in [p.left(), p.top(), p.w, p.h, recipes::member_reveal(p.w)] {
                scalar(output, value);
            }
            for _ in 0..4 {
                word(output, 0);
            }
        }
        self.cached = Some((contour, offset, self.update.border));
        self.dirty = false;
        Ok(1 + self.members.len() as u32)
    }
}
fn same_geometry(a: &Contour, ao: [f64; 2], b: &Contour, bo: [f64; 2]) -> bool {
    if std::ptr::eq(a, b) && ao == bo {
        return true;
    }
    a.loops.len() == b.loops.len()
        && a.loops.iter().zip(&b.loops).all(|(a, b)| {
            a.len() == b.len()
                && a.iter().zip(b).all(|(a, b)| {
                    [a.from, a.c1, a.c2, a.to]
                        .into_iter()
                        .zip([b.from, b.c1, b.c2, b.to])
                        .all(|(a, b)| {
                            (0..2).all(|i| ((a[i] - ao[i]) - (b[i] - bo[i])).abs() < 1e-5)
                        })
                })
        })
}
fn word(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_ne_bytes());
}
fn scalar(out: &mut Vec<u8>, value: f64) {
    word(out, (value as f32).to_bits());
}

#[derive(Default)]
pub struct Scene {
    nodes: FxHashMap<u32, Node>,
    updates: Vec<Update>,
    sizing: FxHashSet<u32>,
    parents: FxHashMap<u32, u32>,
    output: Vec<u8>,
}
impl Scene {
    /// Reject malformed commands and node-limit violations before changing
    /// nodes. The last encoded frame remains available after such rejection.
    pub fn frame(&mut self, input: &[u8], elapsed: f64, reduced: bool) -> Result<&[u8], Error> {
        if input.len() % COMMAND_BYTES != 0
            || input.len() / COMMAND_BYTES > MAX_COMMANDS
            || !elapsed.is_finite()
            || elapsed < 0.
        {
            return Err(Error::Command);
        }
        self.updates.clear();
        for bytes in input.chunks_exact(COMMAND_BYTES) {
            self.updates.push(Update::parse(bytes)?);
        }
        if !self.updates.is_empty() {
            self.sizing.clear();
            self.sizing
                .extend(self.nodes.keys().chain(self.parents.keys()).copied());
            for update in &self.updates {
                if update.kind == 7 {
                    self.sizing.remove(&update.id);
                } else {
                    self.sizing.insert(update.id);
                }
                if self.sizing.len() > MAX_NODES {
                    return Err(Error::Capacity);
                }
            }
            for update in &self.updates {
                if update.kind == 10 {
                    let parent = self
                        .updates
                        .iter()
                        .rev()
                        .find(|u| u.id == update.parent)
                        .map(|u| u.kind)
                        .or_else(|| self.nodes.get(&update.parent).map(|n| n.update.kind));
                    let conflicting = self
                        .updates
                        .iter()
                        .any(|u| u.id == update.id && !matches!(u.kind, 7 | 10));
                    if parent != Some(9)
                        || conflicting
                        || self.nodes.contains_key(&update.id)
                        || self
                            .parents
                            .get(&update.id)
                            .is_some_and(|p| *p != update.parent)
                    {
                        return Err(Error::Command);
                    }
                } else if update.kind != 7 && self.parents.contains_key(&update.id) {
                    return Err(Error::Command);
                }
            }
        }
        for update in &self.updates {
            if update.kind == 10 {
                continue;
            }
            if update.kind == 7 {
                if let Some(parent) = self.parents.remove(&update.id) {
                    if let Some(node) = self.nodes.get_mut(&parent) {
                        node.remove_member(update.id);
                    }
                }
                self.nodes.remove(&update.id);
                self.parents.retain(|_, parent| *parent != update.id);
                continue;
            }
            if let Some(node) = self
                .nodes
                .get_mut(&update.id)
                .filter(|n| n.update.kind == update.kind)
            {
                node.apply(*update);
            } else {
                self.parents.retain(|_, parent| *parent != update.id);
                self.nodes.insert(update.id, Node::new(*update)?);
            }
        }
        for update in &self.updates {
            if update.kind == 10 {
                self.nodes
                    .get_mut(&update.parent)
                    .ok_or(Error::Command)?
                    .member(*update);
                self.parents.insert(update.id, update.parent);
            }
        }
        self.output.clear();
        self.output.resize(16, 0);
        let mut moving = 0;
        let mut changed = 0;
        let mut errors = 0;
        for node in self.nodes.values_mut() {
            moving += u32::from(node.advance(elapsed, reduced));
            errors += u32::from(
                node.surface
                    .as_ref()
                    .is_some_and(|s| s.last_error.is_some()),
            );
            let start = self.output.len();
            match node.write(&mut self.output) {
                Ok(written) => changed += written,
                Err(_) => {
                    self.output.truncate(start);
                    errors += 1;
                }
            }
            if self.output.len() > MAX_FRAME_BYTES {
                return Err(Error::Capacity);
            }
        }
        for (i, value) in [VERSION, changed, moving, errors].into_iter().enumerate() {
            self.output[i * 4..i * 4 + 4].copy_from_slice(&value.to_ne_bytes());
        }
        Ok(&self.output)
    }
    pub fn pending(&self) -> &[u8] {
        &self.output
    }
    pub fn len(&self) -> usize {
        self.nodes.len() + self.parents.len()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

#[cfg(test)]
mod tests;
