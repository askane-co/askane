use crate::budget::BudgetCore;
use crate::surface::Surface;

pub const MAX_LAYERS: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendMode {
    Normal,
    Add,
    Multiply,
}

impl BlendMode {
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "normal" => Ok(Self::Normal),
            "add" => Ok(Self::Add),
            "multiply" => Ok(Self::Multiply),
            other => Err(format!(
                "Invalid layer blend mode '{}' (expected 'normal', 'add', or 'multiply')",
                other
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Layer {
    pub surface: Surface,
    pub mode: BlendMode,
    pub visible: bool,
    pub z_order: i32,
    pub parallax_x: f64,
    pub parallax_y: f64,
    pub alive: bool,
}

#[derive(Default)]
pub struct LayerBank {
    pub slots: Vec<Option<Layer>>,
}

impl LayerBank {
    pub fn new() -> Self {
        Self { slots: Vec::new() }
    }

    pub fn active_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.as_ref().is_some_and(|l| l.alive))
            .count()
    }

    pub fn create(
        &mut self,
        width: i32,
        height: i32,
        budget: Option<&BudgetCore>,
    ) -> Result<usize, String> {
        if self.active_count() >= MAX_LAYERS {
            return Err(format!("Maximum layer limit ({}) reached", MAX_LAYERS));
        }
        let surface = Surface::new_rgba8(width, height, 0x0000_0000, budget)?;
        let layer = Layer {
            surface,
            mode: BlendMode::Normal,
            visible: true,
            z_order: 0,
            parallax_x: 1.0,
            parallax_y: 1.0,
            alive: true,
        };
        if let Some(idx) = self
            .slots
            .iter()
            .position(|slot| slot.as_ref().is_none_or(|l| !l.alive))
        {
            self.slots[idx] = Some(layer);
            Ok(idx)
        } else {
            let idx = self.slots.len();
            self.slots.push(Some(layer));
            Ok(idx)
        }
    }

    pub fn get(&self, id: usize) -> Result<&Layer, String> {
        self.slots
            .get(id)
            .and_then(|s| s.as_ref())
            .filter(|l| l.alive)
            .ok_or_else(|| format!("Invalid or freed layer handle {}", id))
    }

    pub fn get_mut(&mut self, id: usize) -> Result<&mut Layer, String> {
        self.slots
            .get_mut(id)
            .and_then(|s| s.as_mut())
            .filter(|l| l.alive)
            .ok_or_else(|| format!("Invalid or freed layer handle {}", id))
    }

    pub fn free(&mut self, id: usize, budget: Option<&BudgetCore>) -> Result<(), String> {
        let Some(slot) = self.slots.get_mut(id) else {
            return Err(format!("Invalid or freed layer handle {}", id));
        };
        let Some(mut layer) = slot.take() else {
            return Err(format!("Invalid or freed layer handle {}", id));
        };
        if !layer.alive {
            return Err(format!("Invalid or freed layer handle {}", id));
        }
        layer.alive = false;
        layer.surface.refund_to(budget);
        Ok(())
    }

    pub fn clear_all(&mut self, budget: Option<&BudgetCore>) {
        for slot in self.slots.iter_mut() {
            if let Some(mut layer) = slot.take() {
                layer.alive = false;
                layer.surface.refund_to(budget);
            }
        }
        self.slots.clear();
    }
}
