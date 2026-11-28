use std::cmp::Ordering;
use std::collections::BinaryHeap;
use std::sync::Arc;

use crate::budget::BudgetCore;
use crate::media::{MediaBudgetGuard, temp_budget_guard};
use crate::scene::tiled::SolidGrid;

pub const MAX_NAV_CELLS: usize = 4_000_000;

#[derive(Debug)]
pub struct NavGrid {
    pub width: usize,
    pub height: usize,
    pub costs: Vec<u16>, // 0 = blocked, >=1 walkable movement cost
    #[allow(dead_code)]
    pub budget: Option<MediaBudgetGuard>,
}

impl NavGrid {
    pub fn new(
        width: usize,
        height: usize,
        default_cost: u16,
        budget_core: Option<&Arc<BudgetCore>>,
    ) -> Result<Self, String> {
        if width == 0 || height == 0 {
            return Err("E_NAV_INVALID: nav grid dimensions must be positive".to_string());
        }
        let cells = width
            .checked_mul(height)
            .ok_or_else(|| "E_NAV_INVALID: nav grid size overflow".to_string())?;
        if cells > MAX_NAV_CELLS {
            return Err("E_NAV_INVALID: nav grid exceeds 4M cell limit".to_string());
        }
        let guard = MediaBudgetGuard::charge(budget_core, cells * 2)?;
        Ok(Self {
            width,
            height,
            costs: vec![default_cost; cells],
            budget: Some(guard),
        })
    }

    pub fn from_solid_grid(
        grid: &SolidGrid,
        budget_core: Option<&Arc<BudgetCore>>,
    ) -> Result<Self, String> {
        let mut nav = Self::new(grid.width, grid.height, 1, budget_core)?;
        for (i, &solid) in grid.cells.iter().enumerate() {
            if solid {
                nav.costs[i] = 0;
            }
        }
        Ok(nav)
    }

    #[inline]
    pub fn in_bounds(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height
    }

    #[inline]
    pub fn is_walkable(&self, x: i32, y: i32) -> bool {
        if !self.in_bounds(x, y) {
            return false;
        }
        self.costs[(y as usize) * self.width + (x as usize)] > 0
    }

    pub fn set_cost(&mut self, x: i32, y: i32, cost: u16) -> Result<(), String> {
        if !self.in_bounds(x, y) {
            return Err(format!(
                "E_NAV_INVALID: cell ({}, {}) out of {}x{} bounds",
                x, y, self.width, self.height
            ));
        }
        self.costs[(y as usize) * self.width + (x as usize)] = cost;
        Ok(())
    }

    /// Deterministic A* using integer scaled costs (`10` ortho, `14` diag) and index tie-breaking.
    pub fn find_path(
        &self,
        sx: i32,
        sy: i32,
        gx: i32,
        gy: i32,
        diagonal: bool,
        budget_core: Option<&Arc<BudgetCore>>,
    ) -> Result<Vec<[i32; 2]>, String> {
        if !self.is_walkable(sx, sy) || !self.is_walkable(gx, gy) {
            return Ok(Vec::new());
        }
        if sx == gx && sy == gy {
            return Ok(vec![[sx, sy]]);
        }
        let n = self.width * self.height;
        let _tmp = temp_budget_guard(budget_core, n * 10)?;

        #[derive(Clone, Copy, Eq, PartialEq)]
        struct OpenEntry {
            f: u32,
            h: u32,
            idx: usize,
        }
        impl Ord for OpenEntry {
            fn cmp(&self, other: &Self) -> Ordering {
                // Reverse for min-heap with deterministic tie-break on lowest idx
                other
                    .f
                    .cmp(&self.f)
                    .then_with(|| other.h.cmp(&self.h))
                    .then_with(|| other.idx.cmp(&self.idx))
            }
        }
        impl PartialOrd for OpenEntry {
            fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
                Some(self.cmp(other))
            }
        }

        let heuristic = |x: i32, y: i32| -> u32 {
            let dx = (x - gx).abs() as u32;
            let dy = (y - gy).abs() as u32;
            if diagonal {
                let mn = dx.min(dy);
                let mx = dx.max(dy);
                mn * 14 + (mx - mn) * 10
            } else {
                (dx + dy) * 10
            }
        };

        let start_idx = (sy as usize) * self.width + (sx as usize);
        let goal_idx = (gy as usize) * self.width + (gx as usize);

        let mut g_score = vec![u32::MAX; n];
        let mut parent = vec![u32::MAX; n];
        let mut closed = vec![false; n];
        let mut open = BinaryHeap::new();

        g_score[start_idx] = 0;
        let h0 = heuristic(sx, sy);
        open.push(OpenEntry {
            f: h0,
            h: h0,
            idx: start_idx,
        });

        let dirs_4: [(i32, i32, u32); 4] = [(0, -1, 10), (1, 0, 10), (0, 1, 10), (-1, 0, 10)];
        let dirs_8: [(i32, i32, u32); 8] = [
            (0, -1, 10),
            (1, 0, 10),
            (0, 1, 10),
            (-1, 0, 10),
            (1, -1, 14),
            (1, 1, 14),
            (-1, 1, 14),
            (-1, -1, 14),
        ];
        let neighbors = if diagonal { &dirs_8[..] } else { &dirs_4[..] };

        while let Some(cur) = open.pop() {
            if cur.idx == goal_idx {
                let mut rev = Vec::new();
                let mut trace = goal_idx;
                while trace != start_idx && trace != u32::MAX as usize {
                    let x = (trace % self.width) as i32;
                    let y = (trace / self.width) as i32;
                    rev.push([x, y]);
                    trace = parent[trace] as usize;
                }
                rev.push([sx, sy]);
                rev.reverse();
                return Ok(rev);
            }
            if closed[cur.idx] {
                continue;
            }
            closed[cur.idx] = true;
            let cx = (cur.idx % self.width) as i32;
            let cy = (cur.idx / self.width) as i32;
            let cur_g = g_score[cur.idx];

            for &(dx, dy, step_mul) in neighbors {
                let nx = cx + dx;
                let ny = cy + dy;
                if !self.is_walkable(nx, ny) {
                    continue;
                }
                if dx != 0 && dy != 0 && (!self.is_walkable(cx + dx, cy) || !self.is_walkable(cx, cy + dy)) {
                    // Disallow corner cutting across blocked tiles
                    continue;
                }
                let nidx = (ny as usize) * self.width + (nx as usize);
                if closed[nidx] {
                    continue;
                }
                let move_cost = (self.costs[nidx] as u32).saturating_mul(step_mul);
                let cand_g = cur_g.saturating_add(move_cost);
                if cand_g < g_score[nidx] {
                    g_score[nidx] = cand_g;
                    parent[nidx] = cur.idx as u32;
                    let nh = heuristic(nx, ny);
                    open.push(OpenEntry {
                        f: cand_g.saturating_add(nh),
                        h: nh,
                        idx: nidx,
                    });
                }
            }
        }
        Ok(Vec::new())
    }

    pub fn line_of_sight(&self, x0: i32, y0: i32, x1: i32, y1: i32) -> bool {
        let dx = (x1 - x0).abs();
        let dy = (y1 - y0).abs();
        let sx = if x0 < x1 { 1 } else { -1 };
        let sy = if y0 < y1 { 1 } else { -1 };
        let mut err = dx - dy;
        let mut x = x0;
        let mut y = y0;
        loop {
            if !self.is_walkable(x, y) {
                return false;
            }
            if x == x1 && y == y1 {
                return true;
            }
            let e2 = 2 * err;
            if e2 > -dy {
                err -= dy;
                x += sx;
            }
            if e2 < dx {
                err += dx;
                y += sy;
            }
        }
    }

    pub fn smooth_path(&self, path: &[[i32; 2]]) -> Vec<[i32; 2]> {
        if path.len() <= 2 {
            return path.to_vec();
        }
        let mut out = vec![path[0]];
        let mut anchor = 0usize;
        while anchor + 1 < path.len() {
            let mut farthest = anchor + 1;
            for test in (anchor + 2)..path.len() {
                if self.line_of_sight(path[anchor][0], path[anchor][1], path[test][0], path[test][1]) {
                    farthest = test;
                }
            }
            out.push(path[farthest]);
            anchor = farthest;
        }
        out
    }

    pub fn flow_field(
        &self,
        gx: i32,
        gy: i32,
        budget_core: Option<&Arc<BudgetCore>>,
    ) -> Result<Vec<i32>, String> {
        let n = self.width * self.height;
        let _tmp = temp_budget_guard(budget_core, n * 8)?;
        let mut dists = vec![-1i32; n];
        if !self.is_walkable(gx, gy) {
            return Ok(dists);
        }
        let goal_idx = (gy as usize) * self.width + (gx as usize);
        dists[goal_idx] = 0;
        let mut queue = std::collections::VecDeque::new();
        queue.push_back(goal_idx);
        let dirs = [(0, -1), (1, 0), (0, 1), (-1, 0)];
        while let Some(idx) = queue.pop_front() {
            let cx = (idx % self.width) as i32;
            let cy = (idx / self.width) as i32;
            let next_d = dists[idx] + 1;
            for (dx, dy) in dirs {
                let nx = cx + dx;
                let ny = cy + dy;
                if !self.is_walkable(nx, ny) {
                    continue;
                }
                let nidx = (ny as usize) * self.width + (nx as usize);
                if dists[nidx] < 0 {
                    dists[nidx] = next_d;
                    queue.push_back(nidx);
                }
            }
        }
        Ok(dists)
    }
}
