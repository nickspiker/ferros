//! The project's kinds as Kūaha shows them: each one's words, the kist that opens it, and its mark in its circle.

use ferros_items::kinds::*;
use ferros_lang::Msg;

/// The kinds Kūaha opens, for its capsule's `items` declaration (`--opens KIND REASON`, docs/HOLD-API.md "Kūaha": photo, RAW video, video, scan, list, contact, document), and every other project kind it can show.
pub const SHOWN: [[u8; 32]; 13] = [PHOTO, RAW_VIDEO, VIDEO, DOCUMENT, SCAN, LIST, CONTACT, SONG, GPS_TRACK, FIND, COLLECTION, GENESIS_SAVE_STATE, GENESIS_BATTERY_RAM];

/// A kind's words: its idea's phrasing in the person's language (ferros_lang's until the ideas' sheet carries them).
pub fn words(kind: &[u8; 32]) -> Msg {
    match *kind {
        PHOTO => Msg::ItemPhoto,
        RAW_VIDEO => Msg::ItemRawVideo,
        VIDEO => Msg::ItemVideo,
        DOCUMENT => Msg::ItemDocument,
        SCAN => Msg::ItemScan,
        LIST => Msg::ItemList,
        CONTACT => Msg::ItemContact,
        SONG => Msg::ItemSong,
        GPS_TRACK => Msg::ItemGpsTrack,
        FIND => Msg::ItemFind,
        COLLECTION => Msg::ItemCollection,
        GENESIS_SAVE_STATE => Msg::ItemGenesisSave,
        GENESIS_BATTERY_RAM => Msg::ItemGenesisRam,
        _ => Msg::ItemOther,
    }
}

/// The kist an item of this kind is handed to (owner, 2026-11-19, ferros_vocabulary "Kūaha": Opsin for images, RAW clips and scans, Photon for contacts); a kist's name, never translated. None: no kist is known to open it.
pub fn opener(kind: &[u8; 32]) -> Option<&'static str> {
    match *kind {
        PHOTO | RAW_VIDEO | VIDEO | SCAN => Some("Opsin"),
        CONTACT => Some("Photon"),
        GPS_TRACK => Some("Mahere"),
        GENESIS_SAVE_STATE | GENESIS_BATTERY_RAM => Some("Genesis"),
        _ => None,
    }
}

/// What a kind's circle shows inside its ring when the card carries no preview of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    /// A lens: a ring with a dot (a photo, a scan).
    Lens,
    /// A recording's dot, filled (RAW video, video).
    Dot,
    /// Lines of text (a document, a list, a contact, a find).
    Lines,
    /// Nothing but the ring.
    Ring,
}

pub fn mark(kind: &[u8; 32]) -> Mark {
    match *kind {
        PHOTO | SCAN => Mark::Lens,
        RAW_VIDEO | VIDEO | SONG => Mark::Dot,
        DOCUMENT | LIST | CONTACT | FIND | COLLECTION => Mark::Lines,
        _ => Mark::Ring,
    }
}
