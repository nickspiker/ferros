//! fluor on the phone: a FluorApp through fluor's ferros host. Spawn: `ferros-bridge spawn fluordemo.bin --name fluordemo --use display --use touch --dma surface --entropy` (grant slots 1, 2, 3). Taps count; the sixteenth closes the app and hands the screen back to the map.

use fluor::canvas::Canvas;
use fluor::coord::Coord;
use fluor::event::{CursorIcon, ElementState, Event, MouseButton};
use fluor::host::EventResponse;
use fluor::host::app::{Context, FluorApp};
use fluor::pixel::G2t16;
use fluor::text::TextStyle;
use std::time::Instant;

struct Demo {
    taps: u32,
    last: (Coord, Coord),
    started: Instant,
    frames: u32,
}

// At fluor's 16-bit depth, all the way to the panel: the display decodes the surface itself (op 22 format G2T16).
impl FluorApp<G2t16> for Demo {
    type UserEvent = ();

    fn init(&mut self, _ctx: &mut Context) {}

    fn on_resize(&mut self, _width: u32, _height: u32, ctx: &mut Context) {
        ctx.window.request_redraw();
    }

    fn on_event(&mut self, event: &Event, ctx: &mut Context) -> EventResponse {
        match event {
            Event::MouseInput { state: ElementState::Pressed, button: MouseButton::Left } => {
                self.taps += 1;
                self.last = (ctx.cursor_x, ctx.cursor_y);
                ctx.window.request_redraw();
                if self.taps >= 16 { EventResponse::Close } else { EventResponse::Handled }
            }
            _ => EventResponse::Pass,
        }
    }

    fn render(&mut self, target: &mut [G2t16], ctx: &mut Context) {
        self.frames += 1;
        let (w, h) = (ctx.viewport.width_px as usize, ctx.viewport.height_px as usize);
        let mut canvas = Canvas::new(target, w, h, ctx.damage);
        // Colours authored as 8-bit darkness (0xFF000000 opaque white, 0xFFFFFFFF opaque black), widened to 16 bits as they are drawn.
        fluor::paint::fill_rect(&mut canvas, 0, 0, w as isize, h as isize, 0xFF_E8_E8_E8, None, None);
        fluor::paint::fill_rect(&mut canvas, 24, 24, w as isize - 48, 120, 0xFF_C0_30_30, None, None);
        let title = TextStyle::new(40.0, 0xFF_00_00_00);
        let body = TextStyle::new(28.0, 0xFF_20_20_20);
        ctx.text.draw_text_left(&mut canvas, "fluor on ferros", 48.0, 100.0, &title, None, None);
        let lines = [
            format!("taps A#{} (sixteen to close)", self.taps),
            format!("last tap at A#{} A#{}", self.last.0 as i32, self.last.1 as i32),
            format!("frame A#{}, up A#{} s", self.frames, self.started.elapsed().as_secs()),
            format!("viewport A#{}x{} ru {:.2}", w, h, ctx.viewport.ru),
        ];
        for (i, l) in lines.iter().enumerate() {
            ctx.text.draw_text_left(&mut canvas, l, 48.0, 220.0 + 48.0 * i as Coord, &body, None, None);
        }
        // The last tap, marked.
        if self.taps > 0 {
            fluor::paint::fill_rect(&mut canvas, self.last.0 as isize - 12, self.last.1 as isize - 12, 24, 24, 0xFF_FF_FF_FF, None, None);
        }
    }

    fn cursor_for(&self, _x: Coord, _y: Coord, _ctx: &Context) -> CursorIcon {
        CursorIcon::Default
    }

    fn tick(&mut self, ctx: &mut Context) -> bool {
        // A second's clock line.
        let _ = ctx;
        self.started.elapsed().as_millis() % 1000 < 9
    }
}

fn main() {
    println!("fluordemo: starting");
    let caps = fluor::host::ferros::Caps { display: 1, touch: 2, surface: 3 };
    let app = Demo { taps: 0, last: (0.0, 0.0), started: Instant::now(), frames: 0 };
    match fluor::host::ferros::run_app(app, caps, 1.0, Some((448, 996))) {
        Ok(()) => println!("fluordemo: closed after sixteen taps"),
        Err(e) => println!("fluordemo: {e}"),
    }
}
