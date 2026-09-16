//! Retained contour and trace workspace, independent of a drawing backend.
use crate::{
    contour::{trace_with_workspace, Workspace},
    Contour, ContourError, Cubic, Simulation,
};
use std::rc::Rc;

pub struct Surface {
    pub simulation: Simulation,
    contour: Rc<Contour>,
    base: Rc<Contour>,
    cutouts: Vec<Vec<Cubic>>,
    workspace: Workspace,
    attempted: u64,
    pub last_error: Option<ContourError>,
}
impl Surface {
    pub fn new(simulation: Simulation) -> Result<Self, ContourError> {
        let mut workspace = Workspace::default();
        let contour = Rc::new(trace_with_workspace(&simulation, &mut workspace)?);
        Ok(Self {
            attempted: simulation.revision,
            simulation,
            base: contour.clone(),
            contour,
            cutouts: Vec::new(),
            workspace,
            last_error: None,
        })
    }
    /// Changed geometry receives a new identity. Failed revisions retain the
    /// last valid contour and are retried only when the simulation changes.
    pub fn prepare(&mut self) -> bool {
        if self.attempted == self.simulation.revision {
            return false;
        }
        self.attempted = self.simulation.revision;
        match trace_with_workspace(&self.simulation, &mut self.workspace) {
            Ok(path) => {
                self.base = Rc::new(path);
                self.refresh_cutouts();
                self.last_error = None;
                true
            }
            Err(error) => {
                self.last_error = Some(error);
                false
            }
        }
    }
    pub fn set_cutouts(&mut self, cutouts: Vec<Vec<Cubic>>) {
        if self.cutouts != cutouts {
            self.cutouts = cutouts;
            self.refresh_cutouts();
        }
    }
    fn refresh_cutouts(&mut self) {
        if self.cutouts.is_empty() {
            self.contour = self.base.clone();
            return;
        }
        let mut contour = (*self.base).clone();
        contour.loops.extend(self.cutouts.clone());
        self.contour = Rc::new(contour);
    }
    pub fn contour(&self) -> Rc<Contour> {
        self.contour.clone()
    }
}
