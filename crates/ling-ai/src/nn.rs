//! Optimized feed-forward neural network — fast, cache-friendly, dependency-free.
//!
//! Designed for real-time game AI: weights live in flat row-major `Vec<f32>`
//! buffers so the hot inner products autovectorize under `opt-level = 3`, there
//! are no per-call heap allocations on the inference path, and training is plain
//! SGD back-propagation with no autograd graph.
//!
//! The network doubles as the backbone for [`crate::dialog_lm`]: `forward_cache`
//! + `backward` expose the input gradient so an embedding table can be trained
//! end-to-end through the MLP.

/// Seeded SplitMix64 RNG — reproducible init & sampling (games want determinism).
#[derive(Clone)]
pub struct Rng {
    state: u64,
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed ^ 0x9E37_79B9_7F4A_7C15 }
    }

    #[inline]
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform f32 in `[0, 1)`.
    #[inline]
    pub fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }

    /// Uniform f32 in `[-a, a]`.
    #[inline]
    pub fn uniform(&mut self, a: f32) -> f32 {
        (self.next_f32() * 2.0 - 1.0) * a
    }
}

/// Non-linearity applied at a layer's output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activation {
    Linear,
    Relu,
    Tanh,
    Sigmoid,
}

impl Activation {
    /// Parse a name (`"relu"`, `"tanh"`, `"sigmoid"`, anything else → linear).
    pub fn parse(s: &str) -> Activation {
        match s.trim().to_ascii_lowercase().as_str() {
            "relu" => Activation::Relu,
            "tanh" => Activation::Tanh,
            "sigmoid" | "logistic" => Activation::Sigmoid,
            _ => Activation::Linear,
        }
    }

    fn code(self) -> u8 {
        match self {
            Activation::Linear => 0,
            Activation::Relu => 1,
            Activation::Tanh => 2,
            Activation::Sigmoid => 3,
        }
    }

    fn from_code(c: u8) -> Activation {
        match c {
            1 => Activation::Relu,
            2 => Activation::Tanh,
            3 => Activation::Sigmoid,
            _ => Activation::Linear,
        }
    }

    #[inline]
    fn apply(self, x: f32) -> f32 {
        match self {
            Activation::Linear => x,
            Activation::Relu => x.max(0.0),
            Activation::Tanh => x.tanh(),
            Activation::Sigmoid => 1.0 / (1.0 + (-x).exp()),
        }
    }

    /// Derivative expressed in terms of the layer **output** `y` (cheaper).
    #[inline]
    fn deriv(self, y: f32) -> f32 {
        match self {
            Activation::Linear => 1.0,
            Activation::Relu => {
                if y > 0.0 {
                    1.0
                } else {
                    0.0
                }
            },
            Activation::Tanh => 1.0 - y * y,
            Activation::Sigmoid => y * (1.0 - y),
        }
    }
}

/// A fully-connected layer. Weights are row-major `[n_out][n_in]`.
#[derive(Clone)]
pub struct Dense {
    pub n_in: usize,
    pub n_out: usize,
    pub w: Vec<f32>, // n_out * n_in
    pub b: Vec<f32>, // n_out
    pub act: Activation,
}

impl Dense {
    fn new(n_in: usize, n_out: usize, act: Activation, rng: &mut Rng) -> Self {
        // Glorot/He uniform init: range scaled by fan-in/out, boosted for ReLU.
        let mut limit = (6.0 / (n_in + n_out) as f32).sqrt();
        if act == Activation::Relu {
            limit *= std::f32::consts::SQRT_2;
        }
        let w = (0..n_in * n_out).map(|_| rng.uniform(limit)).collect();
        Self { n_in, n_out, w, b: vec![0.0; n_out], act }
    }

    /// `out = act(W·x + b)` — the hot inference kernel.
    #[inline]
    fn forward(&self, x: &[f32], out: &mut Vec<f32>) {
        out.clear();
        out.reserve(self.n_out);
        for o in 0..self.n_out {
            let row = &self.w[o * self.n_in..(o + 1) * self.n_in];
            let mut acc = self.b[o];
            for i in 0..self.n_in {
                acc += row[i] * x[i];
            }
            out.push(self.act.apply(acc));
        }
    }
}

/// A stack of dense layers.
#[derive(Clone)]
pub struct Net {
    pub n_in: usize,
    pub layers: Vec<Dense>,
    rng: Rng,
}

impl Net {
    /// Start an empty network with `n_in` inputs. Append layers with [`Net::dense`].
    pub fn new(n_in: usize, seed: u64) -> Self {
        Self { n_in, layers: Vec::new(), rng: Rng::new(seed) }
    }

    /// Width of the current output (input width if no layers yet).
    pub fn out_width(&self) -> usize {
        self.layers.last().map(|l| l.n_out).unwrap_or(self.n_in)
    }

    /// Append a dense layer of `units` neurons with the given activation.
    pub fn dense(&mut self, units: usize, act: Activation) -> &mut Self {
        let n_in = self.out_width();
        let layer = Dense::new(n_in, units, act, &mut self.rng);
        self.layers.push(layer);
        self
    }

    /// Inference — allocates only the output vector. Truncates/pads a wrong-width
    /// input so callers can't panic the interpreter.
    pub fn forward(&self, input: &[f32]) -> Vec<f32> {
        let mut cur = fit(input, self.n_in);
        let mut next = Vec::new();
        for layer in &self.layers {
            layer.forward(&cur, &mut next);
            std::mem::swap(&mut cur, &mut next);
        }
        cur
    }

    /// Forward pass keeping every layer's activations: `acts[0]` is the input,
    /// `acts[i+1]` the output of layer `i`. Used for back-propagation.
    pub fn forward_cache(&self, input: &[f32]) -> Vec<Vec<f32>> {
        let mut acts: Vec<Vec<f32>> = Vec::with_capacity(self.layers.len() + 1);
        acts.push(fit(input, self.n_in));
        for layer in &self.layers {
            let mut out = Vec::new();
            layer.forward(acts.last().unwrap(), &mut out);
            acts.push(out);
        }
        acts
    }

    /// SGD update given `dL/dy` at the network output. Returns `dL/dx` (gradient
    /// w.r.t. the inputs) so upstream tables (e.g. embeddings) can also learn.
    pub fn backward(&mut self, acts: &[Vec<f32>], d_out: &[f32], lr: f32) -> Vec<f32> {
        let last = self.layers.len() - 1;
        // delta = dL/d(pre-activation) at the output layer.
        let mut delta: Vec<f32> = (0..self.layers[last].n_out)
            .map(|o| d_out[o] * self.layers[last].act.deriv(acts[last + 1][o]))
            .collect();

        for li in (0..self.layers.len()).rev() {
            let (n_in, n_out) = (self.layers[li].n_in, self.layers[li].n_out);
            let a_prev = &acts[li];

            // Propagate to previous layer (uses *old* weights) before updating.
            let mut delta_prev = vec![0.0f32; n_in];
            for i in 0..n_in {
                let mut s = 0.0;
                for o in 0..n_out {
                    s += self.layers[li].w[o * n_in + i] * delta[o];
                }
                // Input layer (li==0) has no activation to differentiate.
                delta_prev[i] = if li > 0 {
                    s * self.layers[li - 1].act.deriv(a_prev[i])
                } else {
                    s
                };
            }

            // Apply SGD step to this layer.
            let layer = &mut self.layers[li];
            for o in 0..n_out {
                let g = delta[o];
                layer.b[o] -= lr * g;
                let row = o * n_in;
                for i in 0..n_in {
                    layer.w[row + i] -= lr * g * a_prev[i];
                }
            }
            delta = delta_prev;
        }
        delta // dL/dx
    }

    /// One SGD step against a target with mean-squared-error loss. Returns the
    /// loss before the update.
    pub fn train_mse(&mut self, input: &[f32], target: &[f32], lr: f32) -> f32 {
        let acts = self.forward_cache(input);
        let out = acts.last().unwrap();
        let t = fit(target, out.len());
        let mut d_out = vec![0.0f32; out.len()];
        let mut loss = 0.0;
        for i in 0..out.len() {
            let d = out[i] - t[i];
            loss += d * d;
            d_out[i] = d; // d(½·Σd²)/dy = d
        }
        self.backward(&acts, &d_out, lr);
        0.5 * loss
    }

    // ── Serialization ────────────────────────────────────────────────────────
    // Compact little-endian binary: magic, n_in, n_layers, then per layer
    // (n_in, n_out, act, weights…, biases…). Stable across runs/platforms.

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(b"LNN1");
        b.extend_from_slice(&(self.n_in as u32).to_le_bytes());
        b.extend_from_slice(&(self.layers.len() as u32).to_le_bytes());
        for l in &self.layers {
            b.extend_from_slice(&(l.n_in as u32).to_le_bytes());
            b.extend_from_slice(&(l.n_out as u32).to_le_bytes());
            b.push(l.act.code());
            for &w in &l.w {
                b.extend_from_slice(&w.to_le_bytes());
            }
            for &bias in &l.b {
                b.extend_from_slice(&bias.to_le_bytes());
            }
        }
        b
    }

    pub fn from_bytes(data: &[u8]) -> Option<Net> {
        let mut p = Reader::new(data);
        if p.tag(4)? != b"LNN1" {
            return None;
        }
        let n_in = p.u32()? as usize;
        let n_layers = p.u32()? as usize;
        let mut layers = Vec::with_capacity(n_layers);
        for _ in 0..n_layers {
            let l_in = p.u32()? as usize;
            let l_out = p.u32()? as usize;
            let act = Activation::from_code(p.u8()?);
            let w = p.f32s(l_in * l_out)?;
            let b = p.f32s(l_out)?;
            layers.push(Dense { n_in: l_in, n_out: l_out, w, b, act });
        }
        Some(Net { n_in, layers, rng: Rng::new(0) })
    }
}

/// Truncate or zero-pad `x` to exactly `n` elements (never panics).
#[inline]
fn fit(x: &[f32], n: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; n];
    let m = x.len().min(n);
    v[..m].copy_from_slice(&x[..m]);
    v
}

/// Minimal forward byte reader for deserialization.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    fn tag(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.data.get(self.pos..self.pos + n)?;
        self.pos += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        let v = *self.data.get(self.pos)?;
        self.pos += 1;
        Some(v)
    }
    fn u32(&mut self) -> Option<u32> {
        let s = self.data.get(self.pos..self.pos + 4)?;
        self.pos += 4;
        Some(u32::from_le_bytes(s.try_into().ok()?))
    }
    fn f32s(&mut self, n: usize) -> Option<Vec<f32>> {
        let mut v = Vec::with_capacity(n);
        for _ in 0..n {
            let s = self.data.get(self.pos..self.pos + 4)?;
            self.pos += 4;
            v.push(f32::from_le_bytes(s.try_into().ok()?));
        }
        Some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_shapes() {
        let mut net = Net::new(3, 42);
        net.dense(8, Activation::Relu).dense(2, Activation::Linear);
        let y = net.forward(&[0.1, 0.2, 0.3]);
        assert_eq!(y.len(), 2);
    }

    #[test]
    fn learns_xor() {
        // The classic non-linearly-separable problem — proves backprop works.
        let mut net = Net::new(2, 7);
        net.dense(8, Activation::Tanh).dense(1, Activation::Sigmoid);
        let data = [
            ([0.0, 0.0], [0.0]),
            ([0.0, 1.0], [1.0]),
            ([1.0, 0.0], [1.0]),
            ([1.0, 1.0], [0.0]),
        ];
        let mut loss = 0.0;
        for _ in 0..4000 {
            loss = 0.0;
            for (x, t) in &data {
                loss += net.train_mse(x, t, 0.5);
            }
        }
        assert!(loss < 0.05, "XOR did not converge, loss={loss}");
        for (x, t) in &data {
            let y = net.forward(x)[0];
            assert!((y - t[0]).abs() < 0.3, "x={x:?} y={y} t={}", t[0]);
        }
    }

    #[test]
    fn serialization_roundtrips() {
        let mut net = Net::new(4, 1);
        net.dense(5, Activation::Relu).dense(3, Activation::Sigmoid);
        let bytes = net.to_bytes();
        let back = Net::from_bytes(&bytes).expect("decode");
        let x = [0.5, -0.2, 0.7, 0.1];
        assert_eq!(net.forward(&x), back.forward(&x));
    }

    #[test]
    fn wrong_width_input_is_safe() {
        let mut net = Net::new(3, 0);
        net.dense(2, Activation::Linear);
        assert_eq!(net.forward(&[1.0]).len(), 2); // padded, no panic
        assert_eq!(net.forward(&[1.0, 2.0, 3.0, 4.0]).len(), 2); // truncated
    }
}
