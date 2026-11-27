//! The thermometer puck's face (owner, 2026-11-20): the object's temperature large in the middle, the sensor's own temperature smaller under it, from the IR thermometer's mailbox (ferros_mailbox::thermo).
//! No word says which is which: the large number is what the device points at, the small one the device itself.
//! Before the first sample, the sky alone.

use crate::{u, TEXT, TEXT2};
use ferros_puck::{fmt::unit, q14, Full, Writer};

/// What the face shows: the object's and the sensor's own temperature, °C, as the AoC reports them (None before the first sample).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Reading {
    pub object: Option<f32>,
    pub sensor: Option<f32>,
}

/// °C as the ops' Q16.
fn q16(c: f32) -> i64 {
    (c * (1 << 16) as f32) as i64
}

pub fn face(w: &mut Writer, r: &Reading, focus: bool) -> Result<(), Full> {
    // In focus the numbers carry one more place: the circle has the room.
    let places = 1 + focus as u8;
    if let Some(o) = r.object {
        w.number(0, q14(0.04), u(0.26), TEXT, q16(o), places, unit::CELSIUS)?;
    }
    if let Some(s) = r.sensor {
        w.number(0, q14(0.5), u(0.1), TEXT2, q16(s), places, unit::CELSIUS)?;
    }
    crate::sky(w)?;
    Ok(())
}
