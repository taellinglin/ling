// src/runtime/ai.rs — game-AI builtins backing `ling-ai`.
//
// Three global handle tables (neural nets, behavior trees, dialog LMs) keyed by
// an integer id, mirroring the single-global pattern in `net.rs`. Each id is the
// slot index returned at creation; the Ling program keeps that number and passes
// it back to operate on the object. All accessors fail soft (return a default,
// never panic) so a bad handle can't crash the interpreter.

use ling_ai::{Activation, Dataset, DialogLM, LmConfig, Net, Tree};
use std::sync::Mutex;

static NETS: Mutex<Vec<Option<Net>>> = Mutex::new(Vec::new());
static TREES: Mutex<Vec<Option<Tree>>> = Mutex::new(Vec::new());
static LMS: Mutex<Vec<Option<DialogLM>>> = Mutex::new(Vec::new());

/// Park `item` in a slab and return its handle (`-1` if the lock is poisoned).
fn push<T>(slab: &Mutex<Vec<Option<T>>>, item: T) -> i64 {
    match slab.lock() {
        Ok(mut g) => {
            g.push(Some(item));
            (g.len() - 1) as i64
        },
        Err(_) => -1,
    }
}

/// Run `f` against the live object at `id`. `None` if the handle is invalid.
fn with<T, R>(slab: &Mutex<Vec<Option<T>>>, id: i64, f: impl FnOnce(&mut T) -> R) -> Option<R> {
    let idx = usize::try_from(id).ok()?;
    let mut g = slab.lock().ok()?;
    g.get_mut(idx)?.as_mut().map(f)
}

// ── Neural network ───────────────────────────────────────────────────────────

pub fn nn_new(n_in: usize, seed: u64) -> i64 {
    push(&NETS, Net::new(n_in, seed))
}

pub fn nn_dense(id: i64, units: usize, act: &str) {
    with(&NETS, id, |net| {
        net.dense(units, Activation::parse(act));
    });
}

pub fn nn_forward(id: i64, input: &[f32]) -> Vec<f32> {
    with(&NETS, id, |net| net.forward(input)).unwrap_or_default()
}

pub fn nn_train(id: i64, input: &[f32], target: &[f32], lr: f32) -> f32 {
    with(&NETS, id, |net| net.train_mse(input, target, lr)).unwrap_or(0.0)
}

pub fn nn_save(id: i64, path: &str) -> bool {
    with(&NETS, id, |net| {
        std::fs::write(path, net.to_bytes()).is_ok()
    })
    .unwrap_or(false)
}

pub fn nn_load(path: &str) -> i64 {
    match std::fs::read(path).ok().and_then(|b| Net::from_bytes(&b)) {
        Some(net) => push(&NETS, net),
        None => -1,
    }
}

// ── Behavior tree ────────────────────────────────────────────────────────────

pub fn bt_build(spec: &str) -> i64 {
    match Tree::parse(spec) {
        Some(t) => push(&TREES, t),
        None => -1,
    }
}

pub fn bt_set(id: i64, key: &str, value: f32) {
    with(&TREES, id, |t| t.set(key, value));
}

pub fn bt_tick(id: i64) -> String {
    with(&TREES, id, |t| t.tick()).unwrap_or_default()
}

/// 0 = failure, 1 = success, 2 = running (the last tick's result).
pub fn bt_status(id: i64) -> i64 {
    with(&TREES, id, |t| t.status() as i64).unwrap_or(0)
}

// ── Dialog language model ────────────────────────────────────────────────────

pub fn dialog_new(ctx: usize, embed: usize, hidden: usize, seed: u64) -> i64 {
    push(&LMS, DialogLM::new(LmConfig { ctx, embed, hidden, seed }))
}

pub fn dialog_learn(id: i64, text: &str) {
    with(&LMS, id, |lm| lm.learn(text));
}

/// Load a `.lingdialog` corpus into model `id`; returns lines added (`-1` on error).
pub fn dialog_load(id: i64, path: &str) -> i64 {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(_) => return -1,
    };
    let ds = Dataset::parse(&text);
    with(&LMS, id, |lm| lm.learn_dataset(&ds) as i64).unwrap_or(-1)
}

/// Train; returns final-epoch cross-entropy loss.
pub fn dialog_train(id: i64, epochs: usize, lr: f32) -> f32 {
    with(&LMS, id, |lm| lm.train(epochs, lr)).unwrap_or(0.0)
}

pub fn dialog_say(id: i64, prompt: &str, max_tokens: usize, temperature: f32) -> String {
    with(&LMS, id, |lm| lm.say(prompt, max_tokens, temperature)).unwrap_or_default()
}

pub fn dialog_save(id: i64, path: &str) -> bool {
    with(&LMS, id, |lm| std::fs::write(path, lm.to_bytes()).is_ok()).unwrap_or(false)
}

pub fn dialog_load_model(path: &str) -> i64 {
    match std::fs::read(path)
        .ok()
        .and_then(|b| DialogLM::from_bytes(&b))
    {
        Some(lm) => push(&LMS, lm),
        None => -1,
    }
}
