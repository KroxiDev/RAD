//! Development tool, not a test: draws the real editor, connected to the runtime in
//! `XDG_RUNTIME_DIR`, into PNG files without a window or GPU. Nothing is compared; the
//! images are for looking at. Run it with `scripts/dev-run.sh --capture`
//! (docs/testing.md). A task that needs another screen adds a scene here while it lasts.
use super::App;
use eframe::{
    App as _,
    egui::{self, Color32, Pos2, Rect, TextureId, Vec2, epaint},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

/// The editor window's default size, in points.
const SIZE: Vec2 = egui::vec2(1180., 780.);

#[test]
#[ignore = "development tool: run it with scripts/dev-run.sh --capture"]
fn editor() {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR"));
    let out = PathBuf::from(std::env::var_os("RAD_CAPTURE_DIR").expect("RAD_CAPTURE_DIR"));
    let keys = seed(&runtime);
    // Without the omarchy theme or systemd: what a new installation shows.
    let mut app = App::with_preferences_path(runtime.clone(), runtime.join("no-theme"), None);
    let mut camera = Camera::default();

    camera.settle(&mut app, |app| {
        app.connected
            && app
                .board
                .as_ref()
                .is_some_and(|b| b.buttons.len() == keys.len())
    });
    camera.save(&mut app, &out.join("main.png"));

    let (position, id) = keys.last().cloned().expect("a key");
    app.select(position, Some(id));
    camera.settle(&mut app, |app| app.inspection.is_some());
    camera.save(&mut app, &out.join("key.png"));

    app.open_settings(crate::screens::Section::General);
    camera.settle(&mut app, |_| true);
    camera.save(&mut app, &out.join("settings.png"));
}

/// A few keys through the public API, so the board is not empty: (position, id) of each.
fn seed(runtime: &Path) -> Vec<(usize, String)> {
    let mut editor =
        rad_client::connection::Client::connect(runtime, rad_client::connection::Endpoint::Editor)
            .unwrap_or_else(|_| panic!("no runtime answers in {}", runtime.display()));
    let board = call(&mut editor, "board", json!({}));
    let folder = board["folder"]["id"]
        .as_str()
        .expect("a folder id")
        .to_owned();
    let names = ["Browser", "Notes", "Short break"];
    for (position, name) in names.into_iter().enumerate() {
        call(
            &mut editor,
            "create_button",
            json!({"folder_id": folder, "position": position, "name": name}),
        );
    }
    let slots = call(&mut editor, "board", json!({}))["folder"]["fields"]["slots"].clone();
    let keys = (0..names.len())
        .map(|position| {
            let id = slots[position]["id"].as_str().expect("the new key's id");
            (position, id.to_owned())
        })
        .collect::<Vec<_>>();
    let (_, last) = &keys[keys.len() - 1];
    call(
        &mut editor,
        "add_action",
        json!({"button_id": last, "type": "wait", "parameters": {"milliseconds": 750}}),
    );
    keys
}

/// A request that must succeed; a change right after another may find the runtime busy.
fn call(editor: &mut rad_client::connection::Client, operation: &str, arguments: Value) -> Value {
    for _ in 0..100 {
        let response = editor
            .request(operation, arguments.clone())
            .unwrap_or_else(|failure| panic!("{operation}: {failure:?}"));
        match &response.error {
            None => return serde_json::to_value(&response.data).expect("response data"),
            Some(error) if error.code == "busy" => std::thread::sleep(Duration::from_millis(20)),
            Some(error) => panic!("{operation}: {error:?}"),
        }
    }
    panic!("{operation}: the runtime stayed busy")
}

/// Runs editor frames headlessly and keeps the textures they upload.
struct Camera {
    ctx: egui::Context,
    frame: eframe::Frame,
    textures: HashMap<TextureId, Texture>,
    time: f64,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            ctx: egui::Context::default(),
            frame: eframe::Frame::_new_kittest(),
            textures: HashMap::new(),
            time: 0.,
        }
    }
}

impl Camera {
    fn frame(&mut self, app: &mut App) -> egui::FullOutput {
        // A second per frame: fades and other animations have always finished.
        self.time += 1.;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SIZE)),
            time: Some(self.time),
            ..Default::default()
        };
        let frame = &mut self.frame;
        let output = self.ctx.run_ui(input, |ui| app.ui(ui, frame));
        for (id, deltas) in &output.textures_delta.set {
            for delta in deltas {
                self.textures.entry(*id).or_default().apply(delta);
            }
        }
        output
    }

    /// Frames until `ready` holds and the replies it waited for are drawn.
    fn settle(&mut self, app: &mut App, ready: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready(app) || app.worker.busy() {
            assert!(
                Instant::now() < deadline,
                "the editor did not settle in 10 s"
            );
            self.frame(app);
            std::thread::sleep(Duration::from_millis(20));
        }
        for _ in 0..8 {
            self.frame(app);
        }
    }

    fn save(&mut self, app: &mut App, path: &Path) {
        let output = self.frame(app);
        let scale = output.pixels_per_point;
        let primitives = self.ctx.tessellate(output.shapes, scale);
        let mut canvas = Canvas::new(SIZE * scale);
        for primitive in &primitives {
            if let epaint::Primitive::Mesh(mesh) = &primitive.primitive
                && let Some(texture) = self.textures.get(&mesh.texture_id)
            {
                canvas.mesh(mesh, texture, primitive.clip_rect, scale);
            }
        }
        canvas.save(path);
        println!("{}", path.display());
    }
}

/// A texture as the GPU would hold it: premultiplied, in gamma space.
#[derive(Default)]
struct Texture {
    size: [usize; 2],
    pixels: Vec<Color32>,
    linear: bool,
}

impl Texture {
    fn apply(&mut self, delta: &epaint::ImageDelta) {
        let epaint::ImageData::Color(image) = &delta.image;
        self.linear = delta.options.magnification == egui::TextureFilter::Linear;
        let Some([x0, y0]) = delta.pos else {
            self.size = image.size;
            self.pixels = image.pixels.clone();
            return;
        };
        let [width, _] = image.size;
        for (row, line) in image.pixels.chunks(width).enumerate() {
            let start = (y0 + row) * self.size[0] + x0;
            self.pixels[start..start + width].copy_from_slice(line);
        }
    }

    /// Sampled like the GPU: bilinear for linear textures, clamped at the edges.
    fn sample(&self, uv: Pos2) -> [f32; 4] {
        let [width, height] = self.size;
        let texel = |x: isize, y: isize| {
            let x = x.clamp(0, width as isize - 1) as usize;
            let y = y.clamp(0, height as isize - 1) as usize;
            self.pixels[y * width + x].to_array().map(f32::from)
        };
        let (x, y) = (uv.x * width as f32, uv.y * height as f32);
        if !self.linear {
            return texel(x as isize, y as isize);
        }
        let (x, y) = (x - 0.5, y - 0.5);
        let (fx, fy) = (x - x.floor(), y - y.floor());
        let (x, y) = (x.floor() as isize, y.floor() as isize);
        let mix =
            |a: [f32; 4], b: [f32; 4], t: f32| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t);
        mix(
            mix(texel(x, y), texel(x + 1, y), fx),
            mix(texel(x, y + 1), texel(x + 1, y + 1), fx),
            fy,
        )
    }
}

/// Premultiplied RGBA in gamma space, blended like egui's painters.
struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<[f32; 4]>,
}

impl Canvas {
    fn new(size: Vec2) -> Self {
        let (width, height) = (size.x.round() as usize, size.y.round() as usize);
        Self {
            width,
            height,
            pixels: vec![[0., 0., 0., 1.]; width * height],
        }
    }

    fn mesh(&mut self, mesh: &epaint::Mesh, texture: &Texture, clip: Rect, scale: f32) {
        let clip = Rect::from_min_max(
            (clip.min.to_vec2() * scale).to_pos2(),
            (clip.max.to_vec2() * scale).to_pos2(),
        );
        for triangle in mesh.indices.as_chunks::<3>().0 {
            let [a, b, c] = triangle.map(|index| {
                let mut vertex = mesh.vertices[index as usize];
                vertex.pos = (vertex.pos.to_vec2() * scale).to_pos2();
                vertex
            });
            self.triangle(a, b, c, texture, clip);
        }
    }

    fn triangle(
        &mut self,
        a: epaint::Vertex,
        mut b: epaint::Vertex,
        mut c: epaint::Vertex,
        texture: &Texture,
        clip: Rect,
    ) {
        let edge =
            |p: Pos2, q: Pos2, r: Pos2| (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x);
        let mut area = edge(a.pos, b.pos, c.pos);
        if area == 0. {
            return;
        }
        if area < 0. {
            std::mem::swap(&mut b, &mut c);
            area = -area;
        }
        // Top-left rule: a pixel on an edge shared by two triangles is drawn once.
        let owns = |p: Pos2, q: Pos2, w: f32| {
            w > 0. || (w == 0. && ((q.y == p.y && q.x < p.x) || q.y > p.y))
        };
        let bounds = Rect::from_points(&[a.pos, b.pos, c.pos]).intersect(clip);
        let x0 = bounds.min.x.floor().max(0.) as usize;
        let y0 = bounds.min.y.floor().max(0.) as usize;
        let x1 = (bounds.max.x.ceil().max(0.) as usize).min(self.width);
        let y1 = (bounds.max.y.ceil().max(0.) as usize).min(self.height);
        let color = |v: &epaint::Vertex| v.color.to_array().map(f32::from);
        let (ca, cb, cc) = (color(&a), color(&b), color(&c));
        for y in y0..y1 {
            for x in x0..x1 {
                let p = Pos2::new(x as f32 + 0.5, y as f32 + 0.5);
                let (wa, wb, wc) = (
                    edge(b.pos, c.pos, p),
                    edge(c.pos, a.pos, p),
                    edge(a.pos, b.pos, p),
                );
                if !(owns(b.pos, c.pos, wa) && owns(c.pos, a.pos, wb) && owns(a.pos, b.pos, wc)) {
                    continue;
                }
                let (wa, wb, wc) = (wa / area, wb / area, wc / area);
                let uv = Pos2::new(
                    a.uv.x * wa + b.uv.x * wb + c.uv.x * wc,
                    a.uv.y * wa + b.uv.y * wb + c.uv.y * wc,
                );
                let texel = texture.sample(uv);
                let source: [f32; 4] = std::array::from_fn(|i| {
                    (ca[i] * wa + cb[i] * wb + cc[i] * wc) * texel[i] / (255. * 255.)
                });
                let target = &mut self.pixels[y * self.width + x];
                *target = std::array::from_fn(|i| source[i] + target[i] * (1. - source[3]));
            }
        }
    }

    fn save(&self, path: &Path) {
        let bytes = self
            .pixels
            .iter()
            .flat_map(|[r, g, b, _]| {
                [*r, *g, *b]
                    .map(|v| (v * 255.).round().clamp(0., 255.) as u8)
                    .into_iter()
                    .chain([255])
            })
            .collect();
        image::RgbaImage::from_raw(self.width as u32, self.height as u32, bytes)
            .expect("canvas size")
            .save(path)
            .unwrap_or_else(|e| panic!("could not write {}: {e}", path.display()));
    }
}
