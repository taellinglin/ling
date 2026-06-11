use crate::camera::Camera3D;
use crate::renderer::{Renderer, SoftwareRenderer};
use crate::scene::{Scene, Transform as SceneTransform};
use crate::renderer::FrameBuffer;
use crate::viewport::Viewport;

/// Terminal-based "window" that renders frames via `SoftwareRenderer`.
///
/// Renders a 3-D scene to an RGBA framebuffer then presents it either as
/// coloured ANSI art in the terminal (`present_terminal`) or drives a full
/// animation loop (`run`).
pub struct Window {
    pub viewport: Viewport,
    renderer: SoftwareRenderer,
    term_cols: usize,
    term_rows: usize,
}

impl Window {
    pub fn new(viewport: Viewport) -> Self {
        Self {
            viewport,
            renderer: SoftwareRenderer::new(),
            term_cols: 100,
            term_rows: 40,
        }
    }

    pub fn with_terminal_dimensions(mut self, cols: usize, rows: usize) -> Self {
        self.term_cols = cols.max(10);
        self.term_rows = rows.max(10);
        self
    }

    /// Render the scene into the internal framebuffer and return a clone.
    pub fn render_scene(&mut self, scene: &Scene, camera: &Camera3D) -> FrameBuffer {
        self.renderer.begin_frame(
            self.viewport.width,
            self.viewport.height,
            self.viewport.clear_color,
        );

        for &root in scene.roots() {
            self.render_node_recursive(scene, root, camera, &scene.node(root).transform, 1.0);
        }

        self.renderer.end_frame().clone()
    }

    fn render_node_recursive(
        &mut self,
        scene: &Scene,
        node_id: usize,
        camera: &Camera3D,
        _parent_transform: &SceneTransform,
        _dummy_depth: f32,
    ) {
        let node = scene.node(node_id);
        if node.visible {
            if let (Some(mesh), Some(material)) = (node.mesh.as_ref(), node.material.as_ref()) {
                let global = scene.global_transform(node_id);
                let _ = global;
                self.renderer.draw_mesh(mesh, &node.transform, material, camera);
            }
            // Collect children first to avoid borrow conflict
            let children: Vec<usize> = node.children.clone();
            let transform = node.transform.clone();
            for child in children {
                self.render_node_recursive(scene, child, camera, &transform, _dummy_depth);
            }
        }
    }

    /// Present the last rendered frame to the terminal as ANSI art.
    pub fn present_terminal(&mut self) {
        // Clone the framebuffer to release the mutable borrow on self.renderer
        // before calling present_framebuffer_ascii which needs &self.
        let fb = self.renderer.end_frame().clone();
        self.present_framebuffer_ascii(&fb);
    }

    fn present_framebuffer_ascii(&self, fb: &FrameBuffer) {
        let mut out = String::new();
        out.push_str("\x1b[2J\x1b[H");

        let w = fb.width as usize;
        let h = fb.height as usize;
        let step_x = (w / self.term_cols).max(1);
        let step_y = (h / self.term_rows).max(1);

        // Map luminance to ASCII density ramp
        let chars = b" .:-=+*#%@";

        for y in (0..h).step_by(step_y) {
            for x in (0..w).step_by(step_x) {
                let i = (y * w + x) * 4;
                let r = fb.pixels[i]     as f32 / 255.0;
                let g = fb.pixels[i + 1] as f32 / 255.0;
                let b = fb.pixels[i + 2] as f32 / 255.0;
                let lum = 0.2126 * r + 0.7152 * g + 0.0722 * b;
                let idx = ((lum.clamp(0.0, 1.0)) * ((chars.len() - 1) as f32)) as usize;
                out.push(chars[idx] as char);
            }
            out.push('\n');
        }

        print!("{}", out);
    }

    /// Drive the render / tick / present loop for `max_frames` frames.
    pub fn run(
        &mut self,
        mut scene: Scene,
        mut camera: Camera3D,
        mut tick: impl FnMut(u64, &mut Scene, &mut Camera3D),
        max_frames: u64,
    ) {
        for frame in 0..max_frames {
            tick(frame, &mut scene, &mut camera);

            // Render → clone the framebuffer → present.
            // Cloning releases the mutable borrow of self.renderer so that
            // present_framebuffer_ascii can borrow self immutably.
            let fb = self.render_scene(&scene, &camera);
            self.present_framebuffer_ascii(&fb);
        }
    }
}
