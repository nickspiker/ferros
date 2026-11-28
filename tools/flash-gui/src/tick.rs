//! The installer's checkbox and radio button: a small squircle ring, a filled inner squircle in the one warm colour when it is on, its words to its right (docs/BRAND.md "The likeness").
//! fluor's own Checkbox draws in the platform look (fluor's action blue and a stroked tick) and fluor has no radio button, so these two draw in the installer's palette and take hits and presses as fluor's widgets do.
//! Hover and press are drawn into the paint (the window draws every shown frame), so there is no overlay tint and `tint_delta` stays zero.

use fluor::canvas::Canvas;
use fluor::coord::Coord;
use fluor::event::ModifiersState;
use fluor::host::EventResponse;
use fluor::host::widget::{Click, Hover, PaintCtx, Widget};
use fluor::paint::{self, HitId, draw_squircle_rrect};
use fluor::pixel::G2t16;
use fluor::text::{TextRenderer, TextStyle};

use crate::{BG, BODY, TICK, TITLE, WARM};

pub struct Tick {
    id: HitId,
    /// A radio button (one of two) rather than a checkbox: its corners are half its side, so it reads round.
    round: bool,
    pub on: bool,
    hovered: bool,
    pressed: bool,
    clicked: bool,
    /// Where the last paint showed: the hit claim rides it, as fluor's widgets' does.
    shown: paint::Shown,
}

impl Tick {
    pub fn new(hits: &mut HitId, round: bool, on: bool) -> Tick {
        Tick { id: fluor::host::widget::next_id(hits), round, on, hovered: false, pressed: false, clicked: false, shown: paint::Shown::default() }
    }

    /// A click since the last ask, once (Button's `take_click`).
    pub fn take_click(&mut self) -> bool {
        std::mem::take(&mut self.clicked)
    }

    /// Drawn with its box's left edge at `x0` and its centre line at `cy`, the box `side` wide, the words `gap` to its right in `style`.
    /// `grow` scales the box about its own centre (the unfold: ZERO folded, ONE open); the words' opacity is the caller's, in `style`.
    /// With a hit map it claims its box and its words' line (the gaps between the letters included), front to back as everything else: only what nothing in front already holds.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, canvas: &mut Canvas<G2t16>, text: &mut TextRenderer, x0: f32, cy: f32, side: f32, gap: f32, words: &str, style: &TextStyle, grow: f32, hits: Option<&mut [HitId]>) {
        let (w, h) = (canvas.width, canvas.height);
        let lx = x0 + side + gap;
        let lw = text.measure_text(words, style);
        let reach = side;
        if hits.is_some() {
            // The reach is a box side round the box and the words, cut to the canvas by reach_rect (the layout may put either past an edge in a narrow window).
            let r = paint::reach_rect((x0 - reach) as isize, (cy - side - reach) as isize, (lx + lw + reach) as isize, (cy + side + reach) as isize, w, h);
            self.shown.before(canvas.pixels, w, h, r);
        }

        // The words first: they are in front.
        text.draw_text_left(canvas, words, lx, cy, style, None, None);

        // The box, about its centre, each edge placed from the centre in floats and cast once (silhouette's rule: a ring the same width on every side).
        let (bx, s) = (x0 + side * 0.5, side * grow);
        let stroke = s / (1 << 3) as f32;
        let edges = |half: f32| ((bx - half) as isize, (cy - half) as isize, (bx + half) as isize, (cy + half) as isize);
        let radii = |s: f32| if self.round { [s * 0.5; 4] } else { paint::asymmetric_radii(s * 0.32) };
        if self.on {
            // The dot: a filled squircle half the box, in the warm colour.
            let (a0, b0, a1, b1) = edges(s * 0.25);
            draw_squircle_rrect(canvas, a0, b0, a1 - a0, b1 - b0, TICK, radii(s * 0.5), paint::SHAPE_POWER);
        }
        // The inside in the canvas colour, so the ring behind it shows only as a ring.
        let (i0, j0, i1, j1) = edges(s * 0.5 - stroke);
        draw_squircle_rrect(canvas, i0, j0, i1 - i0, j1 - j0, BG, radii(s - 2. * stroke), paint::SHAPE_POWER);
        // The ring: lighter under the pointer, warm while held.
        let ring = if self.pressed {
            WARM
        } else if self.hovered {
            TITLE
        } else {
            BODY
        };
        let (o0, p0, o1, p1) = edges(s * 0.5);
        draw_squircle_rrect(canvas, o0, p0, o1 - o0, p1 - p0, ring, radii(s), paint::SHAPE_POWER);

        if let Some(map) = hits {
            self.shown.settle(canvas.pixels);
            let (bx0, by0, bx1, by1) = (x0, cy - side * 0.5, x0 + side, cy + side * 0.5);
            let (tx1, ty0, ty1) = (lx + lw, cy - side * 0.7, cy + side * 0.7);
            self.shown.claim_shape(map, self.id, |x, y| {
                let (x, y) = (x as f32 + 0.5, y as f32 + 0.5);
                (x >= bx0 && x < bx1 && y >= by0 && y < by1) || (x >= bx1 && x < tx1 && y >= ty0 && y < ty1)
            });
        }
    }
}

impl Widget for Tick {
    fn id(&self) -> HitId {
        self.id
    }
    fn paint(&mut self, _ctx: &mut PaintCtx<'_, '_>) {
        // The window paints it with `draw`, where its place is known.
    }
    fn click(&mut self) -> Option<&mut dyn Click> {
        Some(self)
    }
    fn hover(&mut self) -> Option<&mut dyn Hover> {
        Some(self)
    }
}

impl Click for Tick {
    fn on_click(&mut self, _x: Coord, _y: Coord, _mods: ModifiersState) -> EventResponse {
        self.clicked = true;
        EventResponse::Handled
    }
    fn set_pressed(&mut self, pressed: bool) {
        self.pressed = pressed;
    }
}

impl Hover for Tick {
    fn set_hovered(&mut self, hovered: bool) {
        self.hovered = hovered;
    }
}
