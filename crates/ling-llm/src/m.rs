//! The few float functions the engine needs, over `libm` (core has none of them).
#[inline]
pub fn sqrt(x: f32) -> f32 {
    libm::sqrtf(x)
}
#[inline]
pub fn exp(x: f32) -> f32 {
    libm::expf(x)
}
#[inline]
pub fn powf(x: f32, y: f32) -> f32 {
    libm::powf(x, y)
}
#[inline]
pub fn sin(x: f32) -> f32 {
    libm::sinf(x)
}
#[inline]
pub fn cos(x: f32) -> f32 {
    libm::cosf(x)
}
#[inline]
pub fn round(x: f32) -> f32 {
    libm::roundf(x)
}
#[inline]
pub fn abs(x: f32) -> f32 {
    libm::fabsf(x)
}
