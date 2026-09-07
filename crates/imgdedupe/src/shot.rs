//! Draw a frame of the window into a picture file.
//!
//! The window is drawn by a graphics card, and a test has none. What a test does
//! have is everything the graphics card would be given: the shapes the frame
//! produced, cut into triangles by the toolkit's own tessellator, and the
//! textures those triangles read from. This fills those triangles into a buffer
//! and writes it out, so what a change did to the window can be looked at
//! instead of guessed at from the rectangles it reports.
//!
//! Flat fills, nearest-neighbour texture reads, no smoothing along an edge. It is
//! for looking at, not for comparing against a reference picture.

use std::collections::HashMap;
use std::path::Path;

use eframe::egui;
use egui::epaint::{ClippedPrimitive, Primitive};

/// Draw one frame, write a picture of it, and hand back the shapes it made.
///
/// Every check about where something landed goes through this, so a check that
/// fails leaves a picture of the window it was measuring beside it. The name is
/// the check's own: the file is `<name>.png`, under `IMGDEDUPE_SHOT_DIR` if that
/// says where, and in the temporary folder otherwise. A check that draws several
/// frames overwrites its own picture each time, so what is left is the frame it
/// went on to measure.
pub fn frame(
    name: &str,
    ctx: &egui::Context,
    input: egui::RawInput,
    run: impl FnMut(&egui::Context),
) -> Vec<egui::epaint::ClippedShape> {
    // One camera per test, because a frame only sends the textures that are new
    // and the letters arrive a row at a time: a camera made fresh for the frame
    // being measured would have none of them and the picture would have no words
    // in it. Tests run one to a thread, so this is that test's own.
    CAMERA.with(|camera| camera.borrow_mut().frame(name, ctx, input, run))
}

thread_local! {
    static CAMERA: std::cell::RefCell<Camera> = std::cell::RefCell::new(Camera::default());
}

/// The folder pictures are written to.
pub fn folder() -> std::path::PathBuf {
    std::env::var_os("IMGDEDUPE_SHOT_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// Everything a frame needs to be drawn: the shapes it made and the textures
/// they read from, which arrive as changes to what was there before.
///
/// One of these is worth keeping across frames where a picture arrives a frame
/// or two after it was asked for: the textures come as changes to what the last
/// frame had, and one made fresh every frame has none of them.
#[derive(Default)]
pub struct Camera {
    textures: HashMap<egui::TextureId, Held>,
    last: Option<Frame>,
}

/// A frame that has been cut into triangles and not yet filled in.
///
/// Filling one is the slow part, and a check draws the same window over and over
/// to let it settle. Only the last of them is worth a picture, so that is the one
/// that gets filled: when the camera goes, at the end of the check that was using
/// it, whether the check passed or panicked.
struct Frame {
    primitives: Vec<ClippedPrimitive>,
    screen: egui::Rect,
    points: f32,
    path: std::path::PathBuf,
}

impl Drop for Camera {
    fn drop(&mut self) {
        self.develop();
    }
}

struct Held {
    size: [usize; 2],
    pixels: Vec<egui::Color32>,
}

impl Camera {
    /// Take one frame of a window and write it to `path` as a PNG.
    ///
    /// `run` is what draws the frame: whatever the test would pass to
    /// `Context::run`. Everything the frame changed about its textures is kept,
    /// because a frame only sends what is new and the atlas of letters arrives a
    /// patch at a time.
    pub fn shoot(
        &mut self,
        ctx: &egui::Context,
        input: egui::RawInput,
        run: impl FnMut(&egui::Context),
        path: &Path,
    ) {
        let _ = self.draw(ctx, input, run, path);
        self.develop();
    }

    /// The same, keeping the shapes: what a check measures and what the picture
    /// shows are then the same frame, and not two frames that happen to look
    /// alike.
    pub fn frame(
        &mut self,
        name: &str,
        ctx: &egui::Context,
        input: egui::RawInput,
        run: impl FnMut(&egui::Context),
    ) -> Vec<egui::epaint::ClippedShape> {
        let at = folder().join(format!("{name}.png"));
        self.draw(ctx, input, run, &at)
    }

    fn draw(
        &mut self,
        ctx: &egui::Context,
        input: egui::RawInput,
        run: impl FnMut(&egui::Context),
        path: &Path,
    ) -> Vec<egui::epaint::ClippedShape> {
        let screen = input.screen_rect.expect("a shot needs to know how big the window is");
        let output = ctx.run(input, run);
        self.take_textures(&output.textures_delta);
        let shapes = output.shapes.clone();
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        self.last = Some(Frame {
            primitives,
            screen,
            points: output.pixels_per_point,
            path: path.to_path_buf(),
        });
        shapes
    }

    /// Fill in the frame that is waiting, if there is one, and write it out.
    pub fn develop(&mut self) {
        let Some(frame) = self.last.take() else {
            return;
        };
        let picture = self.fill(&frame.primitives, frame.screen, frame.points);
        if let Some(folder) = frame.path.parent() {
            let _ = std::fs::create_dir_all(folder);
        }
        // Without the alpha, which is the same everywhere in a picture of a
        // window, and packed as hard as the encoder will: these are looked at and
        // some of them end up inside a page as text.
        let solid = image::DynamicImage::ImageRgba8(picture).to_rgb8();
        let Ok(file) = std::fs::File::create(&frame.path) else {
            return;
        };
        let encoder = image::codecs::png::PngEncoder::new_with_quality(
            std::io::BufWriter::new(file),
            image::codecs::png::CompressionType::Best,
            image::codecs::png::FilterType::Adaptive,
        );
        let _ = image::ImageEncoder::write_image(
            encoder,
            solid.as_raw(),
            solid.width(),
            solid.height(),
            image::ExtendedColorType::Rgb8,
        );
    }

    fn take_textures(&mut self, delta: &egui::TexturesDelta) {
        for (id, change) in &delta.set {
            let (size, pixels) = match &change.image {
                egui::ImageData::Color(image) => (image.size, image.pixels.clone()),
                egui::ImageData::Font(image) => {
                    (image.size, image.srgba_pixels(None).collect::<Vec<_>>())
                }
            };
            match change.pos {
                // A patch of a texture that is already here: the atlas of letters
                // grows a row at a time as more of them are drawn.
                Some([x, y]) => {
                    if let Some(held) = self.textures.get_mut(id) {
                        for row in 0..size[1] {
                            for column in 0..size[0] {
                                let at = (y + row) * held.size[0] + x + column;
                                if let Some(pixel) = held.pixels.get_mut(at) {
                                    *pixel = pixels[row * size[0] + column];
                                }
                            }
                        }
                    }
                }
                None => {
                    self.textures.insert(*id, Held { size, pixels });
                }
            }
        }
        for id in &delta.free {
            self.textures.remove(id);
        }
    }

    /// Fill every triangle of every primitive into a buffer the size of the
    /// window, on the page's own white.
    fn fill(
        &self,
        primitives: &[ClippedPrimitive],
        screen: egui::Rect,
        points: f32,
    ) -> image::RgbaImage {
        let width = (screen.width() * points).round() as u32;
        let height = (screen.height() * points).round() as u32;
        let mut out = image::RgbaImage::from_pixel(width, height, image::Rgba([255, 255, 255, 255]));

        for primitive in primitives {
            let Primitive::Mesh(mesh) = &primitive.primitive else {
                continue;
            };
            let clip = primitive.clip_rect;
            let held = self.textures.get(&mesh.texture_id);
            for triangle in mesh.indices.chunks_exact(3) {
                let corners = [
                    mesh.vertices[triangle[0] as usize],
                    mesh.vertices[triangle[1] as usize],
                    mesh.vertices[triangle[2] as usize],
                ];
                paint(&mut out, &corners, clip, held, points);
            }
        }
        out
    }
}

/// One triangle, filled where it covers a pixel's middle.
fn paint(
    out: &mut image::RgbaImage,
    corners: &[egui::epaint::Vertex; 3],
    clip: egui::Rect,
    held: Option<&Held>,
    points: f32,
) {
    let at = |corner: &egui::epaint::Vertex| (corner.pos.x * points, corner.pos.y * points);
    let (x0, y0) = at(&corners[0]);
    let (x1, y1) = at(&corners[1]);
    let (x2, y2) = at(&corners[2]);
    let area = (x1 - x0) * (y2 - y0) - (x2 - x0) * (y1 - y0);
    if area.abs() < 1e-6 {
        return;
    }

    let left = x0.min(x1).min(x2).max(clip.left() * points).max(0.0).floor() as i64;
    let right =
        x0.max(x1).max(x2).min(clip.right() * points).min(out.width() as f32).ceil() as i64;
    let top = y0.min(y1).min(y2).max(clip.top() * points).max(0.0).floor() as i64;
    let bottom =
        y0.max(y1).max(y2).min(clip.bottom() * points).min(out.height() as f32).ceil() as i64;

    for y in top..bottom {
        for x in left..right {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let one = ((x1 - px) * (y2 - py) - (x2 - px) * (y1 - py)) / area;
            let two = ((x2 - px) * (y0 - py) - (x0 - px) * (y2 - py)) / area;
            let three = 1.0 - one - two;
            if one < 0.0 || two < 0.0 || three < 0.0 {
                continue;
            }

            let colour = mix(corners, [one, two, three]);
            let colour = match held {
                Some(held) => {
                    let uv = [
                        one * corners[0].uv.x + two * corners[1].uv.x + three * corners[2].uv.x,
                        one * corners[0].uv.y + two * corners[1].uv.y + three * corners[2].uv.y,
                    ];
                    times(colour, read(held, uv))
                }
                None => colour,
            };
            over(out.get_pixel_mut(x as u32, y as u32), colour);
        }
    }
}

fn mix(corners: &[egui::epaint::Vertex; 3], weights: [f32; 3]) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (corner, weight) in corners.iter().zip(weights) {
        let colour = corner.color.to_array();
        for (channel, value) in out.iter_mut().enumerate() {
            *value += f32::from(colour[channel]) * weight;
        }
    }
    out
}

fn read(held: &Held, uv: [f32; 2]) -> [f32; 4] {
    if held.size[0] == 0 || held.size[1] == 0 {
        return [255.0; 4];
    }
    let x = (uv[0] * held.size[0] as f32).round().clamp(0.0, held.size[0] as f32 - 1.0) as usize;
    let y = (uv[1] * held.size[1] as f32).round().clamp(0.0, held.size[1] as f32 - 1.0) as usize;
    let pixel = held.pixels[y * held.size[0] + x].to_array();
    [
        f32::from(pixel[0]),
        f32::from(pixel[1]),
        f32::from(pixel[2]),
        f32::from(pixel[3]),
    ]
}

fn times(one: [f32; 4], other: [f32; 4]) -> [f32; 4] {
    let mut out = [0.0; 4];
    for channel in 0..4 {
        out[channel] = one[channel] * other[channel] / 255.0;
    }
    out
}

/// The toolkit's colours carry their alpha already multiplied in, so what is
/// underneath is kept by what is left of it.
fn over(under: &mut image::Rgba<u8>, colour: [f32; 4]) {
    let left = 1.0 - colour[3] / 255.0;
    for channel in 0..4 {
        let was = f32::from(under.0[channel]);
        under.0[channel] = (colour[channel] + was * left).clamp(0.0, 255.0) as u8;
    }
}
