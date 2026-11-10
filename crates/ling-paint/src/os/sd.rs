//! Stable Diffusion in the no_std build: LingOS has no local SD server, so every request answers with an error
//! message on its channel (same API as the desktop's `sd` module).

#[allow(unused_imports)]
use crate::prelude::*;
use core::cell::RefCell;

pub const UNAVAILABLE: &str = "Stable Diffusion is not available on LingOS (it needs a local AUTOMATIC1111 / Forge server)";

pub enum SdMsg {
    Models(Vec<String>),
    Samplers(Vec<String>),
    Progress(f32, String),
    Images(Vec<Vec<u8>>),
    Error(String),
}

/// `std::sync::mpsc::Receiver`'s `try_recv`, over messages queued up front
pub struct Receiver<T>(RefCell<Vec<T>>);
impl<T> Receiver<T> {
    pub fn try_recv(&self) -> Result<T, ()> {
        let mut q = self.0.borrow_mut();
        if q.is_empty() {
            Err(())
        } else {
            Ok(q.remove(0))
        }
    }
}

fn refused() -> Receiver<SdMsg> {
    Receiver(RefCell::new(vec![SdMsg::Error(UNAVAILABLE.into())]))
}

pub fn http(_base: &str, _method: &str, _path: &str, _body: Option<&str>, _timeout_s: u64) -> Result<String, String> {
    Err(UNAVAILABLE.into())
}

pub fn fetch_lists(_base: String) -> Receiver<SdMsg> {
    refused()
}

pub struct GenReq {
    pub base: String,
    pub prompt: String,
    pub negative: String,
    pub steps: u32,
    pub cfg: f32,
    pub denoise: f32,
    pub seed: i64,
    pub sampler: String,
    pub model: String,
    pub w: u32,
    pub h: u32,
    pub init: Option<Vec<u8>>,
    pub mask: Option<Vec<u8>>,
}

pub fn generate(_r: GenReq) -> Receiver<SdMsg> {
    refused()
}

pub fn interrupt(_base: String) {}
