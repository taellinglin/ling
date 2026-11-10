![ling-gpu icon](images/icon.svg)

# ling-gpu

GPU compute backend for Ling (CUDA via cudarc, with a portable CPU fallback)

Part of the [Ling](https://ling-lang.org) programming language ecosystem. See the [main repository](https://github.com/taellinglin/ling) for docs, examples, and the full workspace.

## Warped (hyperbolic) projection and when the GPU pays off

`Backend::project_points_warped` projects a vertex buffer with the full
`Camera3D` model — view roll plus the `set_camera_hyper` hyperbolic warp. The
CPU path (`cpu::project_warped_cpu`, rayon) and the CUDA kernel
(`project_points_warped`) are checked against the scalar reference
`project_warped` in the tests.

Measured on 2× RTX 4090 + a 32-thread CPU (`cargo test -p ling-gpu --release
--features cuda -- --ignored --nocapture breakdown`):

| vertices | upload | kernel | download | all CPU cores |
|---------:|-------:|-------:|---------:|--------------:|
| 1 M      | 3.9 ms | 0.42 ms | 4.1 ms  | ~2.0 ms |
| 4 M      | 16.3 ms | 0.52 ms | 14.9 ms | ~8 ms |

The kernel is ~15× faster than every CPU core together, but cudarc 0.12 copies
from pageable host memory at ~3 GB/s, so a per-frame upload + download of 24
bytes per vertex costs more than the CPU spends on the math. The CPU/GPU
crossover (`CUDA_MIN_PROJECT`, override with `LING_GPU_MIN_PROJECT`) therefore
keeps projection on the CPU for any realistic scene. The GPU becomes the right
place once vertices stay resident on the device between frames (upload a static
mesh once, send only the camera per frame) and the projected result is consumed
there too (GPU rasterisation) instead of being copied back.
