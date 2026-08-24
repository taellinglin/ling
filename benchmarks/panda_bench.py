#!/usr/bin/env python3
"""Python + Panda3D mirror of panda_bench.ling — engine-feature benchmark.

One benchmark per feature category, each using the idiomatic Panda3D API
(C++-backed) from Python, against Ling's builtin surface:

  MATH      vec_math         LVecBase3d dot / cross / length
  COMPUTING noise_field      StackedPerlinNoise2 fractal noise (4 octaves)
  GRAPHICS  vertex_pipeline  LMatrix4d rotate*translate compose + xform + project
  AUDIO     fm_synth         2-op FM + envelope (Panda3D has no DSP API -> pure Python)
  CRYPTO    hash_chain       HashVal MD5 hash chain (Ling side: sha256_hex)
  AI        boids            flock steering (seek + separation) with LVecBase3d
  PHYSICS   spring_rope      spring-mass rope, symplectic Euler, LVecBase3d

Emits: BENCH <name> RESULT <checksum> TIME <seconds>
noise_field and hash_chain use different underlying algorithms per language
(Perlin tables / MD5 vs SHA-256), so their checksums are per-language only.
"""
import math
import time

from panda3d.core import (
    HashVal,
    LMatrix4d,
    LPoint3d,
    LVecBase3d,
    StackedPerlinNoise2,
)

PI = 3.141592653589793


# ── MATH: vector algebra with LVecBase3d ─────────────────────────────────────
def vec_math():
    acc = 0.0
    i = 0
    while i < 400_000:
        a = LVecBase3d(math.sin(i * 0.001), math.cos(i * 0.001), 0.5)
        b = LVecBase3d(math.cos(i * 0.002), math.sin(i * 0.002), -0.5)
        d = a.dot(b)
        c = a.cross(b)
        l = c.length()
        acc += d + l
        if l > 1e-9:
            acc += c[0] / l
        i += 1
    return acc


# ── COMPUTING: fractal Perlin noise field ────────────────────────────────────
def noise_field():
    sp = StackedPerlinNoise2(1.0, 1.0, 4, 2.0, 0.5, 256, 42)
    acc = 0.0
    y = 0
    while y < 350:
        x = 0
        while x < 350:
            acc += sp.noise(x * 0.05, y * 0.05)
            x += 1
        y += 1
    return acc


# ── GRAPHICS: vertex transform pipeline with LMatrix4d ───────────────────────
def vertex_pipeline():
    P, F = 2000, 150
    up = LVecBase3d(0.0, 1.0, 0.0)
    pts = []
    j = 0
    while j < P:
        pts.append(LPoint3d(math.sin(j) * 1.5, math.cos(j * 0.7), math.sin(j * 0.3)))
        j += 1
    acc = 0.0
    f = 0
    while f < F:
        m = LMatrix4d.rotate_mat(f * 0.7, up) * LMatrix4d.translate_mat(0.0, 0.0, -6.0)
        j = 0
        while j < P:
            q = m.xform_point(pts[j])
            w = -q[2]
            acc += q[0] / w * 400.0 + 400.0 + q[1] / w * 300.0 + 300.0
            j += 1
        f += 1
    return acc


# ── AUDIO: 2-operator FM synthesis + decay envelope ──────────────────────────
def fm_synth():
    N = 400_000
    sr = 44100.0
    acc = 0.0
    j = 0
    while j < N:
        t = j / sr
        env = math.exp(-1.5 * t)
        m2 = math.sin(2.0 * PI * 660.0 * t)
        m1 = math.sin(2.0 * PI * 220.0 * t + 2.0 * m2)
        acc += env * math.sin(2.0 * PI * 110.0 * t + 3.0 * m1)
        j += 1
    return acc


# ── CRYPTO: iterated hash chain with HashVal (MD5) ───────────────────────────
def hash_chain():
    hv = HashVal()
    hv.hash_string("ling-panda-benchmark-0")
    s = hv.as_hex()[:32]
    i = 0
    while i < 50_000:
        msg = s * 8  # 256-char message per iteration
        hv.hash_string(msg)
        s = hv.as_hex()[:32]
        i += 1
    return float(int(s[:8], 16))


# ── AI: boid flock steering (seek center + separation) ───────────────────────
def boids():
    B, S = 50, 200
    pos = []
    vel = []
    i = 0
    while i < B:
        # exact rational init (no trig) so Ling/Python trajectories are bit-identical
        pos.append(LVecBase3d(((i * 37) % 101) * 0.07 - 3.5,
                              ((i * 53) % 97) * 0.08 - 3.8,
                              ((i * 71) % 89) * 0.05 - 2.2))
        vel.append(LVecBase3d(0.0, 0.0, 0.0))
        i += 1
    s = 0
    while s < S:
        center = LVecBase3d(0.0, 0.0, 0.0)
        i = 0
        while i < B:
            center += pos[i]
            i += 1
        center = center * (1.0 / B)  # explicit reciprocal — matches Ling bit-for-bit
        i = 0
        while i < B:
            p = pos[i]
            steer = (center - p) * 0.01
            sep = LVecBase3d(0.0, 0.0, 0.0)
            j = 0
            while j < B:
                if j != i:
                    d = p - pos[j]
                    d2 = d.length_squared()
                    # smooth 1/d^2 falloff, no cutoff branch: a cutoff turns
                    # Panda3D's FMA-contracted dot products into flipped
                    # branches vs Ling's strict IEEE math, and the flock
                    # amplifies that into checksum divergence
                    sep += d * (0.05 / (d2 + 0.01))
                j += 1
            v = (vel[i] + steer + sep) * 0.98
            spd = v.length()
            if spd > 0.5:
                v = v * (0.5 / spd)
            vel[i] = v
            pos[i] = p + v
            i += 1
        s += 1
    acc = 0.0
    i = 0
    while i < B:
        acc += pos[i].length()
        i += 1
    return acc


# ── PHYSICS: spring-mass rope, symplectic Euler ──────────────────────────────
def spring_rope():
    M, S = 64, 1200
    K, REST, DT, DRAG, G = 200.0, 0.12, 0.002, 0.995, 2.0
    pos = []
    vel = []
    frc = []
    i = 0
    while i < M:
        pos.append(LVecBase3d(i * 0.12, 0.0, 0.0))
        vel.append(LVecBase3d(0.0, 0.0, 0.0))
        frc.append(LVecBase3d(0.0, 0.0, 0.0))
        i += 1
    s = 0
    while s < S:
        i = 0
        while i < M:
            frc[i] = LVecBase3d(0.0, G, 0.0)  # +y is down (screen convention)
            i += 1
        i = 0
        while i < M - 1:
            d = pos[i + 1] - pos[i]
            dist = d.length()
            f = K * (dist - REST)
            dirv = d * (f / dist)
            frc[i] += dirv
            frc[i + 1] -= dirv
            i += 1
        i = 1  # mass 0 is pinned
        while i < M:
            v = (vel[i] + frc[i] * DT) * DRAG
            vel[i] = v
            pos[i] = pos[i] + v * DT
            i += 1
        s += 1
    acc = 0.0
    i = 0
    while i < M:
        acc += pos[i][0] + pos[i][1]
        i += 1
    return acc


def run(name, fn):
    t0 = time.perf_counter()
    r = fn()
    dt = time.perf_counter() - t0
    print(f"BENCH {name} RESULT {r} TIME {dt:.6f}")


if __name__ == "__main__":
    run("vec_math", vec_math)
    run("noise_field", noise_field)
    run("vertex_pipeline", vertex_pipeline)
    run("fm_synth", fm_synth)
    run("hash_chain", hash_chain)
    run("boids", boids)
    run("spring_rope", spring_rope)
