//! N-dimensional tensor (pure-Rust, no external deps).

#[derive(Debug, Clone, PartialEq)]
pub struct Shape(pub Vec<usize>);

impl Shape {
    pub fn numel(&self) -> usize { self.0.iter().product() }
    pub fn rank(&self) -> usize  { self.0.len() }
}

#[derive(Debug, Clone)]
pub struct Tensor {
    pub shape: Shape,
    pub data:  Vec<f32>,
}

impl Tensor {
    pub fn zeros(shape: impl Into<Vec<usize>>) -> Self {
        let shape = Shape(shape.into());
        let n     = shape.numel();
        Self { shape, data: vec![0.0; n] }
    }

    pub fn ones(shape: impl Into<Vec<usize>>) -> Self {
        let shape = Shape(shape.into());
        let n     = shape.numel();
        Self { shape, data: vec![1.0; n] }
    }

    pub fn from_vec(shape: impl Into<Vec<usize>>, data: Vec<f32>) -> Self {
        let shape = Shape(shape.into());
        assert_eq!(shape.numel(), data.len(), "shape/data length mismatch");
        Self { shape, data }
    }

    pub fn get(&self, idx: &[usize]) -> f32 {
        self.data[self.flat_index(idx)]
    }

    pub fn set(&mut self, idx: &[usize], val: f32) {
        let i = self.flat_index(idx);
        self.data[i] = val;
    }

    fn flat_index(&self, idx: &[usize]) -> usize {
        assert_eq!(idx.len(), self.shape.rank());
        let mut flat = 0;
        let mut stride = 1;
        for (i, &dim) in self.shape.0.iter().enumerate().rev() {
            flat   += idx[i] * stride;
            stride *= dim;
        }
        flat
    }

    /// Element-wise addition.
    pub fn add(&self, rhs: &Self) -> Self {
        assert_eq!(self.shape, rhs.shape);
        Self::from_vec(
            self.shape.0.clone(),
            self.data.iter().zip(&rhs.data).map(|(a, b)| a + b).collect(),
        )
    }

    /// Element-wise multiplication.
    pub fn mul(&self, rhs: &Self) -> Self {
        assert_eq!(self.shape, rhs.shape);
        Self::from_vec(
            self.shape.0.clone(),
            self.data.iter().zip(&rhs.data).map(|(a, b)| a * b).collect(),
        )
    }

    /// Matrix multiply (2-D tensors only).
    pub fn matmul(&self, rhs: &Self) -> Self {
        assert_eq!(self.shape.rank(), 2);
        assert_eq!(rhs.shape.rank(), 2);
        let (m, k) = (self.shape.0[0], self.shape.0[1]);
        let n      = rhs.shape.0[1];
        assert_eq!(k, rhs.shape.0[0]);
        let mut out = vec![0.0f32; m * n];
        for i in 0..m { for j in 0..n { for p in 0..k {
            out[i * n + j] += self.data[i * k + p] * rhs.data[p * n + j];
        }}}
        Self::from_vec(vec![m, n], out)
    }

    /// Softmax along the last dimension.
    pub fn softmax(&self) -> Self {
        let mut out = self.data.clone();
        let max = out.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        for v in &mut out { *v = (*v - max).exp(); }
        let sum: f32 = out.iter().sum();
        for v in &mut out { *v /= sum; }
        Self::from_vec(self.shape.0.clone(), out)
    }

    /// ReLU activation.
    pub fn relu(&self) -> Self {
        Self::from_vec(
            self.shape.0.clone(),
            self.data.iter().map(|v| v.max(0.0)).collect(),
        )
    }
}
