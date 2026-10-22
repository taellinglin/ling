//! ling-body — humanoid skin from acoustic impulses.
//!
//! A body scaffold is voxelised into a signed distance field, acoustic sources
//! excite a finite-difference pressure field inside it, a level-set membrane
//! responds to the field energy, and the zero level set is extracted as a mesh.
//! Bodies round-trip through WAV files (`codec`), and arbitrary audio can be
//! analysed back into source parameters (`analysis`).

pub mod analysis;
pub mod codec;
pub mod curves;
pub mod detail;
pub mod export;
pub mod fft;
pub mod gltf;
pub mod grid;
pub mod humanoid;
pub mod kit;
pub mod membrane;
pub mod mesh;
pub mod mesher;
pub mod model;
pub mod params;
pub mod sdf;
pub mod signature;
pub mod sim;
pub mod wave;
pub mod wavio;

pub use grid::Grid;
pub use kit::BodyKit;
pub use mesh::{Mesh, Tris};
pub use params::{Params, Source, Waveform};
