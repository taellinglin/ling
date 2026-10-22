//! Triangle soup to signed distance field: surface voxelisation, exterior flood
//! fill, and an exact Euclidean distance transform.

use crate::grid::Grid;
use crate::mesh::Tris;
use rayon::prelude::*;

const FAR: f32 = 1.0e20;

/// Marks every voxel a triangle passes through by sampling each triangle on a
/// barycentric lattice finer than the voxel size.
fn rasterize(tris: &Tris, grid: &Grid) -> Vec<bool> {
    let mut hit = vec![false; grid.len()];
    let step = 0.45 * grid.dx;
    for t in tris.v.as_chunks::<9>().0 {
        let a = [t[0], t[1], t[2]];
        let b = [t[3], t[4], t[5]];
        let c = [t[6], t[7], t[8]];
        let len = |p: [f32; 3], q: [f32; 3]| {
            ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2) + (p[2] - q[2]).powi(2)).sqrt()
        };
        let longest = len(a, b).max(len(b, c)).max(len(c, a));
        let n = ((longest / step).ceil() as usize).clamp(1, 4096);
        for i in 0..=n {
            for j in 0..=(n - i) {
                let u = i as f32 / n as f32;
                let v = j as f32 / n as f32;
                let w = 1.0 - u - v;
                let p = [
                    a[0] * w + b[0] * u + c[0] * v,
                    a[1] * w + b[1] * u + c[1] * v,
                    a[2] * w + b[2] * u + c[2] * v,
                ];
                let l = grid.lattice(p);
                let (x, y, z) = (l[0].round(), l[1].round(), l[2].round());
                if x >= 0.0 && y >= 0.0 && z >= 0.0 {
                    let (x, y, z) = (x as usize, y as usize, z as usize);
                    if x < grid.nx && y < grid.ny && z < grid.nz {
                        hit[grid.idx(x, y, z)] = true;
                    }
                }
            }
        }
    }
    hit
}

fn dilate(mask: &mut [bool], grid: &Grid, rounds: usize) {
    for _ in 0..rounds {
        let src = mask.to_vec();
        for z in 0..grid.nz {
            for y in 0..grid.ny {
                for x in 0..grid.nx {
                    let i = grid.idx(x, y, z);
                    if src[i] {
                        continue;
                    }
                    let n = (x > 0 && src[i - 1])
                        || (x + 1 < grid.nx && src[i + 1])
                        || (y > 0 && src[i - grid.nx])
                        || (y + 1 < grid.ny && src[i + grid.nx])
                        || (z > 0 && src[i - grid.nx * grid.ny])
                        || (z + 1 < grid.nz && src[i + grid.nx * grid.ny]);
                    mask[i] = n;
                }
            }
        }
    }
}

/// Voxels not reachable from the grid border without crossing the shell.
fn fill_interior(shell: &[bool], grid: &Grid) -> Vec<bool> {
    let mut outside = vec![false; grid.len()];
    let mut stack: Vec<u32> = Vec::new();
    let seed = |x: usize, y: usize, z: usize, outside: &mut Vec<bool>, stack: &mut Vec<u32>| {
        let i = grid.idx(x, y, z);
        if !shell[i] && !outside[i] {
            outside[i] = true;
            stack.push(i as u32);
        }
    };
    for z in 0..grid.nz {
        for y in 0..grid.ny {
            for x in 0..grid.nx {
                if x == 0 || y == 0 || z == 0 || x + 1 == grid.nx || y + 1 == grid.ny || z + 1 == grid.nz {
                    seed(x, y, z, &mut outside, &mut stack);
                }
            }
        }
    }
    while let Some(i) = stack.pop() {
        let (x, y, z) = grid.coords(i as usize);
        let mut visit = |nx: usize, ny: usize, nz: usize| seed(nx, ny, nz, &mut outside, &mut stack);
        if x > 0 {
            visit(x - 1, y, z);
        }
        if x + 1 < grid.nx {
            visit(x + 1, y, z);
        }
        if y > 0 {
            visit(x, y - 1, z);
        }
        if y + 1 < grid.ny {
            visit(x, y + 1, z);
        }
        if z > 0 {
            visit(x, y, z - 1);
        }
        if z + 1 < grid.nz {
            visit(x, y, z + 1);
        }
    }
    outside.iter().map(|&o| !o).collect()
}

/// One-dimensional squared distance transform (Felzenszwalb–Huttenlocher).
fn edt_line(f: &[f32], d: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    let mut k = 0usize;
    v[0] = 0;
    z[0] = -f32::INFINITY;
    z[1] = f32::INFINITY;
    for q in 1..n {
        let s = loop {
            let p = v[k];
            let s = ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * (q as f32 - p as f32));
            if s <= z[k] {
                k -= 1;
            } else {
                break s;
            }
        };
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = f32::INFINITY;
    }
    k = 0;
    for (q, out) in d.iter_mut().enumerate() {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let p = v[k];
        *out = (q as f32 - p as f32).powi(2) + f[p];
    }
}

/// Distance in lattice units from every cell to the nearest cell where `seed` is true.
fn distance_to(seed: &[bool], grid: &Grid) -> Vec<f32> {
    let mut f: Vec<f32> = seed.iter().map(|&s| if s { 0.0 } else { FAR }).collect();
    let (nx, ny, nz) = (grid.nx, grid.ny, grid.nz);
    let longest = nx.max(ny).max(nz);

    f.par_chunks_mut(nx).for_each_init(
        || (vec![0.0f32; nx], vec![0usize; nx], vec![0.0f32; nx + 1]),
        |(d, v, z), row| {
            edt_line(row, d, v, z);
            row.copy_from_slice(d);
        },
    );

    let mut line = vec![0.0f32; longest];
    let mut out = vec![0.0f32; longest];
    let mut v = vec![0usize; longest];
    let mut z = vec![0.0f32; longest + 1];
    for zi in 0..nz {
        for x in 0..nx {
            for y in 0..ny {
                line[y] = f[x + nx * (y + ny * zi)];
            }
            edt_line(&line[..ny], &mut out[..ny], &mut v, &mut z);
            for y in 0..ny {
                f[x + nx * (y + ny * zi)] = out[y];
            }
        }
    }
    for y in 0..ny {
        for x in 0..nx {
            for zi in 0..nz {
                line[zi] = f[x + nx * (y + ny * zi)];
            }
            edt_line(&line[..nz], &mut out[..nz], &mut v, &mut z);
            for zi in 0..nz {
                f[x + nx * (y + ny * zi)] = out[zi];
            }
        }
    }
    f.par_iter_mut().for_each(|s| *s = s.sqrt());
    f
}

/// Signed distance (metres, negative inside) of an occupancy mask.
pub fn signed_distance(solid: &[bool], grid: &Grid) -> Vec<f32> {
    let outside: Vec<bool> = solid.iter().map(|&s| !s).collect();
    let to_solid = distance_to(solid, grid);
    let to_outside = distance_to(&outside, grid);
    to_solid
        .par_iter()
        .zip(to_outside.par_iter())
        .zip(solid.par_iter())
        .map(|((&ds, &dout), &s)| if s { -(dout - 0.5) * grid.dx } else { (ds - 0.5) * grid.dx })
        .collect()
}

/// Half thickness, in voxels, given to surfaces that enclose no volume.
const SHEET_RADIUS: f32 = 0.75;

/// Signed distance of the volume bounded by `tris`.
///
/// Openings narrower than `2 * seal` voxels are closed before the exterior flood
/// fill (a morphological closing), so garments and bodies with open necks, cuffs
/// and hems still enclose their interior; the field is then shifted back so its
/// zero level set lies on the original surface. Surfaces that still enclose
/// nothing, such as free cloth or fingers thinner than the seal, are kept as thin
/// solids instead of vanishing.
pub fn from_tris(tris: &Tris, grid: &Grid, seal: usize) -> Vec<f32> {
    let surface = rasterize(tris, grid);
    let mut shell = surface.clone();
    dilate(&mut shell, grid, seal);
    let solid = fill_interior(&shell, grid);
    let shift = (seal as f32 + 0.5) * grid.dx;
    let mut phi = signed_distance(&solid, grid);
    phi.par_iter_mut().for_each(|p| *p += shift);

    let mut near_core: Vec<bool> = solid.iter().zip(&shell).map(|(&s, &sh)| s && !sh).collect();
    dilate(&mut near_core, grid, seal + 1);
    let sheet: Vec<bool> = surface.iter().zip(&near_core).map(|(&s, &n)| s && !n).collect();
    if sheet.iter().any(|&s| s) {
        let d = distance_to(&sheet, grid);
        phi.par_iter_mut().zip(d.par_iter()).for_each(|(p, &d)| *p = p.min((d - SHEET_RADIUS) * grid.dx));
    }
    blur(&mut phi, grid, 1);
    phi
}

/// Separable 1-2-1 smoothing, `rounds` times per axis.
pub fn blur(f: &mut [f32], grid: &Grid, rounds: usize) {
    let (nx, ny, nz) = (grid.nx, grid.ny, grid.nz);
    for _ in 0..rounds {
        for axis in 0..3 {
            let src = f.to_vec();
            let stride = [1usize, nx, nx * ny][axis];
            let n = [nx, ny, nz][axis];
            f.par_iter_mut().enumerate().for_each(|(i, v)| {
                let c = [i % nx, (i / nx) % ny, i / (nx * ny)][axis];
                let lo = if c > 0 { src[i - stride] } else { src[i] };
                let hi = if c + 1 < n { src[i + stride] } else { src[i] };
                *v = 0.25 * lo + 0.5 * src[i] + 0.25 * hi;
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube_tris(h: f32) -> Tris {
        let mut t = Tris::default();
        let c = |x: f32, y: f32, z: f32| [x * h, y * h, z * h];
        let quad = |t: &mut Tris, a, b, cc, d| {
            t.push(a, b, cc);
            t.push(a, cc, d);
        };
        quad(&mut t, c(-1., -1., -1.), c(1., -1., -1.), c(1., 1., -1.), c(-1., 1., -1.));
        quad(&mut t, c(-1., -1., 1.), c(1., -1., 1.), c(1., 1., 1.), c(-1., 1., 1.));
        quad(&mut t, c(-1., -1., -1.), c(1., -1., -1.), c(1., -1., 1.), c(-1., -1., 1.));
        quad(&mut t, c(-1., 1., -1.), c(1., 1., -1.), c(1., 1., 1.), c(-1., 1., 1.));
        quad(&mut t, c(-1., -1., -1.), c(-1., 1., -1.), c(-1., 1., 1.), c(-1., -1., 1.));
        quad(&mut t, c(1., -1., -1.), c(1., 1., -1.), c(1., 1., 1.), c(1., -1., 1.));
        t
    }

    #[test]
    fn cube_sdf_signs_and_scale() {
        let tris = cube_tris(0.5);
        let grid = Grid::fit([-0.5; 3], [0.5; 3], 0.05, 5);
        let phi = from_tris(&tris, &grid, 1);
        let at = |p: [f32; 3]| {
            let l = grid.lattice(p);
            grid.sample(&phi, l)
        };
        assert!(at([0.0, 0.0, 0.0]) < -0.3, "centre {}", at([0.0; 3]));
        assert!(at([0.7, 0.0, 0.0]) > 0.15);
        assert!(at([0.5, 0.0, 0.0]).abs() < 0.05, "surface {}", at([0.5, 0.0, 0.0]));
    }

    #[test]
    fn open_box_is_closed_by_sealing() {
        let mut tris = cube_tris(0.5);
        tris.v.truncate(tris.v.len() - 18);
        let cell = 0.25;
        for j in 0..4 {
            for k in 0..4 {
                if (j, k) == (1, 2) {
                    continue;
                }
                let (y0, z0) = (-0.5 + j as f32 * cell, -0.5 + k as f32 * cell);
                tris.push([0.5, y0, z0], [0.5, y0 + cell, z0], [0.5, y0 + cell, z0 + cell]);
                tris.push([0.5, y0, z0], [0.5, y0 + cell, z0 + cell], [0.5, y0, z0 + cell]);
            }
        }
        let grid = Grid::fit([-0.5; 3], [0.5; 3], 0.05, 6);
        let at = |phi: &[f32], p: [f32; 3]| grid.sample(phi, grid.lattice(p));
        let sealed = from_tris(&tris, &grid, 4);
        assert!(at(&sealed, [0.0, 0.0, 0.0]) < -0.2, "interior of an open box should be solid");
        assert!(at(&sealed, [0.9, 0.0, 0.0]) > 0.1);
    }

    #[test]
    fn free_sheet_stays_a_thin_solid() {
        let mut tris = Tris::default();
        tris.push([-0.4, -0.4, 0.0], [0.4, -0.4, 0.0], [0.4, 0.4, 0.0]);
        tris.push([-0.4, -0.4, 0.0], [0.4, 0.4, 0.0], [-0.4, 0.4, 0.0]);
        let grid = Grid::fit([-0.4; 3], [0.4; 3], 0.05, 5);
        let phi = from_tris(&tris, &grid, 3);
        let on = grid.sample(&phi, grid.lattice([0.1, 0.1, 0.0]));
        let off = grid.sample(&phi, grid.lattice([0.1, 0.1, 0.3]));
        assert!(on < 0.0, "sheet vanished: {on}");
        assert!(off > 0.15, "sheet should stay thin: {off}");
    }

    #[test]
    fn edt_matches_brute_force() {
        let grid = Grid { nx: 9, ny: 8, nz: 7, origin: [0.0; 3], dx: 1.0 };
        let mut seed = vec![false; grid.len()];
        seed[grid.idx(2, 3, 4)] = true;
        seed[grid.idx(7, 1, 1)] = true;
        let d = distance_to(&seed, &grid);
        for (i, &got) in d.iter().enumerate() {
            let (x, y, z) = grid.coords(i);
            let best = [(2, 3, 4), (7, 1, 1)]
                .iter()
                .map(|&(a, b, c): &(i32, i32, i32)| {
                    (((x as i32 - a).pow(2) + (y as i32 - b).pow(2) + (z as i32 - c).pow(2)) as f32).sqrt()
                })
                .fold(f32::INFINITY, f32::min);
            assert!((got - best).abs() < 1e-3, "{got} vs {best}");
        }
    }
}
