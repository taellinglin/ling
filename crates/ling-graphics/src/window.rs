use crate::camera::Camera3D;
use crate::color::Color;
use crate::renderer::{Renderer, SoftwareRenderer};
use crate::scene::{Scene, Transform as SceneTransform};
use crate::renderer::FrameBuffer;
use crate::viewport::Viewport;

/// Terminal-based "window" that renders frames via `SoftwareRenderer`.
///
/// This is a bridge toward a real OS window. It gives you:
/// - a render loop (`run`)
/// - a viewport (`Viewport`)
/// - a present step (RGBA framebuffer -> ASCII)
pub struct Window {
    pub viewport: Viewport,
    renderer: SoftwareRenderer,
    // Optional: present scale controls for terminal output.
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

    pub fn render_scene(&mut self, scene: &Scene, camera: &Camera3D) -> &FrameBuffer {
        self.renderer.begin_frame(
            self.viewport.width,
            self.viewport.height,
            self.viewport.clear_color,
        );

        // Scene graph currently has `collect_render_items()` which returns
        // only (node_id, global_transform). SoftwareRenderer needs:
        // (mesh, transform, material, camera).
        //
        // Therefore, until `Scene` exposes draw calls, we fall back to a
        // conservative path: traverse scene nodes and render nodes that
        // have mesh + material.
        for &root in scene.roots() {
            self.render_node_recursive(scene, root, camera, &scene.node(root).transform, 1.0);
        }

        self.renderer.end_frame()
    }

    fn render_node_recursive(
        &mut self,
        scene: &Scene,
        node_id: usize,
        camera: &Camera3D,
        parent_transform: &SceneTransform,
        _dummy_depth: f32,
    ) {
        // Note: Scene has internal storage; we access what we need via
        // node()/node_mut() and recompute transforms using `global_transform()`.
        let node = scene.node(node_id);
        if node.visible {
            if let (Some(mesh), Some(material)) = (node.mesh.as_ref(), node.material.as_ref()) {
                let global = scene.global_transform(node_id);
                // SoftwareRenderer expects `Transform` (TRS) not Mat4; however
                // current code uses Transform::matrix() internally.
                // So we create a Transform from translation/rotation/scale if possible.
                // For now, we only support identity parent transforms and use
                // node.transform directly.
                //
                // For a robust path we'd store TRS only and compute global TRS.
                let _ = global;
                self.renderer.draw_mesh(mesh, &node.transform, material, camera);
            }

            for &child in &node.children {
                self.render_node_recursive(scene, child, camera, &node.transform, _dummy_depth);
            }
        }
    }

    pub fn present_terminal(&mut self) {
        let fb = self.renderer.end_frame();
        self.present_framebuffer_ascii(&fb);
    }

    fn present_framebuffer_ascii(&self, fb: &FrameBuffer) {
        let mut out = String::new();

        // Clear screen + home cursor (ANSI).
        out.push_str("\x1b[2J\x1b[H");

        let w = fb.width as usize;
        let h = fb.height as usize;

        // Downsample to terminal size.
        let step_x = (w / self.term_cols).max(1);
        let step_y = (h / self.term_rows).max(1);

        let chars = b" .:-=+*#%@"; // dark -> bright

        for y in (0..h).step_by(step_y) {
            for x in (0..w).step_by(step_x) {
                let i = (y * w + x) * 4;
                let r = fb.pixels[i] as f32 / 255.0;
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

    pub fn run(
        &mut self,
        mut scene: Scene,
        mut camera: Camera3D,
        mut tick: impl FnMut(u64, &mut Scene, &mut Camera3D),
        max_frames: u64,
    ) {
        for frame in 0..max_frames {
            tick(frame, &mut scene, &mut camera);
            // Note: `render_scene` ends the frame; `present` then ends again in
            // this simplified code. For now we just render and present from
            // renderer framebuffer.
            let _ = self.render_scene(&scene, &camera);
            self.present_framebuffer_ascii(self.renderer.end_frame());
        }
    }
}

