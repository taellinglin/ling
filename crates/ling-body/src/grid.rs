//! Uniform lattice shared by the wave field, the membrane and the mesher.
//! Lattice point `(x,y,z)` sits at `origin + (x,y,z) * dx`; storage is x-fastest.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    pub nx: usize,
    pub ny: usize,
    pub nz: usize,
    pub origin: [f32; 3],
    pub dx: f32,
}

impl Grid {
    pub fn fit(lo: [f32; 3], hi: [f32; 3], dx: f32, pad: usize) -> Grid {
        let p = pad as f32 * dx;
        let origin = [lo[0] - p, lo[1] - p, lo[2] - p];
        let n = |a: usize| (((hi[a] + p) - origin[a]) / dx).ceil() as usize + 1;
        Grid { nx: n(0), ny: n(1), nz: n(2), origin, dx }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.nx * self.ny * self.nz
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[inline]
    pub fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        x + self.nx * (y + self.ny * z)
    }

    #[inline]
    pub fn coords(&self, i: usize) -> (usize, usize, usize) {
        (i % self.nx, (i / self.nx) % self.ny, i / (self.nx * self.ny))
    }

    #[inline]
    pub fn world(&self, x: usize, y: usize, z: usize) -> [f32; 3] {
        [
            self.origin[0] + x as f32 * self.dx,
            self.origin[1] + y as f32 * self.dx,
            self.origin[2] + z as f32 * self.dx,
        ]
    }

    /// Continuous lattice coordinates of a world position.
    #[inline]
    pub fn lattice(&self, p: [f32; 3]) -> [f32; 3] {
        [
            (p[0] - self.origin[0]) / self.dx,
            (p[1] - self.origin[1]) / self.dx,
            (p[2] - self.origin[2]) / self.dx,
        ]
    }

    /// Same extent with every `k`-th lattice point kept.
    pub fn coarsen(&self, k: usize) -> Grid {
        let k = k.max(1);
        Grid {
            nx: (self.nx - 1) / k + 1,
            ny: (self.ny - 1) / k + 1,
            nz: (self.nz - 1) / k + 1,
            origin: self.origin,
            dx: self.dx * k as f32,
        }
    }

    /// Trilinear sample of `field` at continuous lattice coordinates; clamps at the border.
    pub fn sample(&self, field: &[f32], l: [f32; 3]) -> f32 {
        let cl = |v: f32, n: usize| v.clamp(0.0, (n - 1) as f32);
        let (fx, fy, fz) = (cl(l[0], self.nx), cl(l[1], self.ny), cl(l[2], self.nz));
        let (x0, y0, z0) = (fx as usize, fy as usize, fz as usize);
        let x1 = (x0 + 1).min(self.nx - 1);
        let y1 = (y0 + 1).min(self.ny - 1);
        let z1 = (z0 + 1).min(self.nz - 1);
        let (tx, ty, tz) = (fx - x0 as f32, fy - y0 as f32, fz - z0 as f32);
        let g = |x, y, z| field[self.idx(x, y, z)];
        let c00 = g(x0, y0, z0) * (1.0 - tx) + g(x1, y0, z0) * tx;
        let c10 = g(x0, y1, z0) * (1.0 - tx) + g(x1, y1, z0) * tx;
        let c01 = g(x0, y0, z1) * (1.0 - tx) + g(x1, y0, z1) * tx;
        let c11 = g(x0, y1, z1) * (1.0 - tx) + g(x1, y1, z1) * tx;
        let c0 = c00 * (1.0 - ty) + c10 * ty;
        let c1 = c01 * (1.0 - ty) + c11 * ty;
        c0 * (1.0 - tz) + c1 * tz
    }
}
