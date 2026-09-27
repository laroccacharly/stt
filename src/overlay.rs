//! Waveform overlay drawn as a wlr layer-shell surface.
//!
//! The overlay runs its own Wayland event loop on a dedicated thread and is
//! driven by [`Event`]s sent through the returned [`Overlay`] handle.

use std::{
    f32::consts::PI,
    thread::{self, JoinHandle},
    time::Instant,
};

use anyhow::{Context, Result};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData, Region},
    delegate_dispatch2, delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop::{
            EventLoop,
            channel::{self, Channel, Sender},
        },
        calloop_wayland_source::WaylandSource,
        client::{
            Connection, EventQueue, QueueHandle,
            globals::{GlobalList, registry_queue_init},
            protocol::{wl_output, wl_shm, wl_surface},
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{
        Shm, ShmHandler,
        slot::{Buffer, SlotPool},
    },
};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, PixmapMut, Rect, Transform};

const BARS: usize = 48;
const WIDTH: u32 = 360;
const HEIGHT: u32 = 64;
const BOTTOM_MARGIN: i32 = 48;

/// What the overlay should show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Connecting,
    Listening,
    Finishing,
}

/// Left and right colour of the bar gradient.
struct Gradient {
    start: [f32; 3],
    end: [f32; 3],
}

impl Phase {
    fn gradient(self) -> Gradient {
        match self {
            Phase::Connecting => Gradient {
                start: [0.55, 0.55, 0.62],
                end: [0.75, 0.75, 0.82],
            },
            Phase::Listening => Gradient {
                start: [0.33, 0.78, 1.0],
                end: [0.78, 0.45, 1.0],
            },
            Phase::Finishing => Gradient {
                start: [0.40, 0.95, 0.60],
                end: [0.30, 0.80, 0.95],
            },
        }
    }
}

#[derive(Debug)]
pub enum Event {
    /// Loudness of the latest audio chunk, `0.0..=1.0`.
    Level(f32),
    Phase(Phase),
    /// Fade out and exit.
    Close,
}

/// Handle to the overlay thread. Sending never fails loudly: if the overlay
/// could not start, dictation keeps working without it.
pub struct Overlay {
    tx: Option<Sender<Event>>,
    thread: Option<JoinHandle<()>>,
}

/// The overlay is cosmetic: dictation carries on without it.
fn overlay_failed(error: &str) {
    crate::notify("STT overlay failed", error, 4000);
}

impl Overlay {
    pub fn spawn() -> Self {
        let endpoints: (Sender<Event>, Channel<Event>) = channel::channel();
        let rx: Channel<Event> = endpoints.1;
        let thread: Option<JoinHandle<()>> = thread::Builder::new()
            .name("overlay".into())
            .spawn(move || {
                if let Err(error) = run(rx) {
                    overlay_failed(&format!("{error:#}"));
                }
            })
            .map_err(|error| overlay_failed(&error.to_string()))
            .ok();
        Self {
            tx: Some(endpoints.0),
            thread,
        }
    }

    pub fn send(&self, event: Event) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(event);
        }
    }

    /// Fade out and wait for the overlay to disappear.
    pub fn close(mut self) {
        self.send(Event::Close);
        self.tx = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(events: Channel<Event>) -> Result<()> {
    let conn: Connection = Connection::connect_to_env().context("no Wayland display")?;
    let registry: (GlobalList, EventQueue<State>) = registry_queue_init(&conn)?;
    let globals: GlobalList = registry.0;
    let queue: EventQueue<State> = registry.1;
    let qh: QueueHandle<State> = queue.handle();
    let compositor: CompositorState =
        CompositorState::bind(&globals, &qh).context("wl_compositor missing")?;
    let layer_shell: LayerShell = LayerShell::bind(&globals, &qh).context("layer shell missing")?;
    let shm: Shm = Shm::bind(&globals, &qh).context("wl_shm missing")?;

    let surface: wl_surface::WlSurface = compositor.create_surface(&qh);
    let layer: LayerSurface =
        layer_shell.create_layer_surface(&qh, surface, Layer::Overlay, Some("stt-overlay"), None);
    layer.set_anchor(Anchor::BOTTOM);
    layer.set_margin(0, 0, BOTTOM_MARGIN, 0);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);
    layer.set_exclusive_zone(-1);
    layer.set_size(WIDTH, HEIGHT);
    // An empty input region lets clicks pass through to the window below.
    let region: Region = Region::new(&compositor)?;
    layer
        .wl_surface()
        .set_input_region(Some(region.wl_region()));
    layer.commit();

    let mut event_loop: EventLoop<'_, State> = EventLoop::try_new()?;
    WaylandSource::new(conn, queue).insert(event_loop.handle())?;
    event_loop
        .handle()
        .insert_source(events, |event, _, state| match event {
            channel::Event::Msg(event) => state.waveform.apply(event),
            channel::Event::Closed => state.waveform.apply(Event::Close),
        })
        .map_err(|error| anyhow::anyhow!("{error}"))?;

    let mut state: State = State {
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &qh),
        pool: SlotPool::new((WIDTH * HEIGHT * 4) as usize, &shm)?,
        shm,
        layer,
        scale: 1,
        configured: false,
        exit: false,
        waveform: Waveform::new(),
    };
    while !state.exit {
        event_loop.dispatch(None, &mut state)?;
    }
    Ok(())
}

/// Animation model, independent of Wayland.
struct Waveform {
    levels: [f32; BARS],
    shown: [f32; BARS],
    phase: Phase,
    opacity: f32,
    closing: bool,
    start: Instant,
    last: Instant,
}

impl Waveform {
    fn new() -> Self {
        let now: Instant = Instant::now();
        Self {
            levels: [0.0; BARS],
            shown: [0.0; BARS],
            phase: Phase::Connecting,
            opacity: 0.0,
            closing: false,
            start: now,
            last: now,
        }
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::Level(level) => {
                self.levels.rotate_left(1);
                self.levels[BARS - 1] = level.clamp(0.0, 1.0);
            }
            Event::Phase(phase) => self.phase = phase,
            Event::Close => self.closing = true,
        }
    }

    /// Advance the animation. Returns `false` once fully faded out.
    fn step(&mut self) -> bool {
        let now: Instant = Instant::now();
        let frames: f32 = (now - self.last).as_secs_f32() * 60.0;
        self.last = now;

        let target: f32 = if self.closing { 0.0 } else { 1.0 };
        self.opacity += (target - self.opacity) * ease(0.18, frames);

        let t: f32 = (now - self.start).as_secs_f32();
        for (i, shown) in self.shown.iter_mut().enumerate() {
            let goal: f32 = match self.phase {
                Phase::Listening => self.levels[i],
                // Gentle ripple while there is no audio to show.
                _ => 0.08 + 0.06 * (t * 4.0 + i as f32 * 0.35).sin(),
            };
            let rate: f32 = if goal > *shown { 0.5 } else { 0.15 };
            *shown += (goal - *shown) * ease(rate, frames);
        }
        !(self.closing && self.opacity < 0.02)
    }

    fn draw(&self, pixmap: &mut PixmapMut, scale: f32) {
        let transform: Transform = Transform::from_scale(scale, scale);
        let w: f32 = WIDTH as f32;
        let h: f32 = HEIGHT as f32;
        let alpha: f32 = self.opacity;
        let mut paint: Paint<'_> = Paint {
            anti_alias: true,
            ..Paint::default()
        };

        let pill: tiny_skia::Path = capsule(0.5, 0.5, w - 1.0, h - 1.0);
        paint.set_color(Color::from_rgba(0.07, 0.07, 0.10, 0.9 * alpha).unwrap());
        pixmap.fill_path(&pill, &paint, FillRule::Winding, transform, None);

        let gradient: Gradient = self.phase.gradient();
        let pad: f32 = h / 2.0;
        let step: f32 = (w - 2.0 * pad) / BARS as f32;
        let bar: f32 = step * 0.55;
        let max_half: f32 = h / 2.0 - 10.0;
        for (i, value) in self.shown.iter().enumerate() {
            let x: f32 = pad + i as f32 * step + (step - bar) / 2.0;
            // Taper the edges so the waveform sits nicely inside the pill.
            let edge: f32 = (PI * (i as f32 + 0.5) / BARS as f32).sin().powf(0.6);
            let half: f32 = (value * edge * max_half).max(bar / 2.0);
            let k: f32 = i as f32 / (BARS - 1) as f32;
            let rgb: [f32; 3] = std::array::from_fn(|c| {
                gradient.start[c] + (gradient.end[c] - gradient.start[c]) * k
            });
            paint.set_color(Color::from_rgba(rgb[0], rgb[1], rgb[2], alpha).unwrap());
            let path: tiny_skia::Path = capsule(x, h / 2.0 - half, bar, half * 2.0);
            pixmap.fill_path(&path, &paint, FillRule::Winding, transform, None);
        }
    }
}

/// Exponential smoothing that behaves the same at any refresh rate.
fn ease(rate: f32, frames: f32) -> f32 {
    1.0 - (1.0 - rate).powf(frames)
}

/// A rectangle with fully rounded ends along its shorter side.
fn capsule(x: f32, y: f32, w: f32, h: f32) -> tiny_skia::Path {
    const K: f32 = 0.552_284_8; // cubic Bézier approximation of a quarter circle
    let r: f32 = w.min(h) / 2.0;
    let c: f32 = r * K;
    let x1: f32 = x + w;
    let y1: f32 = y + h;
    let mut pb: PathBuilder = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x1 - r, y);
    pb.cubic_to(x1 - r + c, y, x1, y + r - c, x1, y + r);
    pb.line_to(x1, y1 - r);
    pb.cubic_to(x1, y1 - r + c, x1 - r + c, y1, x1 - r, y1);
    pb.line_to(x + r, y1);
    pb.cubic_to(x + r - c, y1, x, y1 - r + c, x, y1 - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - c, x + r - c, y, x + r, y);
    pb.close();
    pb.finish().unwrap_or_else(|| {
        PathBuilder::from_rect(Rect::from_xywh(x, y, w.max(1.0), h.max(1.0)).unwrap())
    })
}

struct State {
    registry: RegistryState,
    outputs: OutputState,
    shm: Shm,
    pool: SlotPool,
    layer: LayerSurface,
    scale: i32,
    configured: bool,
    exit: bool,
    waveform: Waveform,
}

impl State {
    fn draw(&mut self, qh: &QueueHandle<Self>) {
        if !self.waveform.step() {
            self.exit = true;
            return;
        }
        let width: i32 = WIDTH as i32 * self.scale;
        let height: i32 = HEIGHT as i32 * self.scale;
        let allocation: (Buffer, &mut [u8]) = if let Ok(allocation) =
            self.pool
                .create_buffer(width, height, width * 4, wl_shm::Format::Argb8888)
        {
            allocation
        } else {
            overlay_failed("failed to allocate buffer");
            self.exit = true;
            return;
        };
        let buffer: Buffer = allocation.0;
        let canvas: &mut [u8] = allocation.1;
        canvas.fill(0);
        if let Some(mut pixmap) = PixmapMut::from_bytes(canvas, width as u32, height as u32) {
            self.waveform.draw(&mut pixmap, self.scale as f32);
        }
        // tiny-skia writes premultiplied RGBA; Wayland's ARGB8888 is BGRA in memory.
        for pixel in canvas.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
        }

        let surface: &wl_surface::WlSurface = self.layer.wl_surface();

        surface.damage_buffer(0, 0, width, height);
        surface.frame(qh, FrameCallbackData(surface.clone()));
        if buffer.attach_to(surface).is_err() {
            self.exit = true;
            return;
        }
        self.layer.commit();
    }
}

impl CompositorHandler for State {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        self.scale = factor.max(1);
        surface.set_buffer_scale(self.scale);
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, qh: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {
        self.draw(qh);
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl LayerShellHandler for State {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        _: &LayerSurface,
        _: LayerSurfaceConfigure,
        _: u32,
    ) {
        if !self.configured {
            self.configured = true;
            self.draw(qh);
        }
    }
}

impl OutputHandler for State {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ShmHandler for State {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for State {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }

    registry_handlers![OutputState];
}

delegate_registry!(State);
delegate_dispatch2!(State);
