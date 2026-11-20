//! The project's kinds (STORE.md part 1, "Kinds, and the question asked"): each an idea (IDEAS.md), its 32 bytes pinned by its first phrasing in English, `ferros_idea::id(eng, words)`.
//! Minted, not published: IDEAS.md publishes no idea before vsf v11, so these ids are computed from their phrasings and pinned here (the test `kinds_are_their_first_phrasings` holds each to `ferros_idea::id`); the sheet that phrases them for every reader waits for v11.

/// (the first phrasing, the id).
pub const KINDS: &[(&str, [u8; 32])] = &[
    ("photo", PHOTO),
    ("RAW video", RAW_VIDEO),
    ("video", VIDEO),
    ("document", DOCUMENT),
    ("scan", SCAN),
    ("list", LIST),
    ("contact", CONTACT),
    ("song", SONG),
    ("GPS track", GPS_TRACK),
    ("find", FIND),
    ("collection", COLLECTION),
    ("Sega Genesis save state", GENESIS_SAVE_STATE),
    ("Sega Genesis battery RAM", GENESIS_BATTERY_RAM),
];

pub const PHOTO: [u8; 32] = [120, 231, 226, 216, 3, 15, 249, 204, 92, 249, 221, 138, 187, 28, 21, 71, 191, 2, 222, 64, 2, 5, 75, 244, 127, 53, 247, 80, 67, 133, 138, 81];
pub const RAW_VIDEO: [u8; 32] = [47, 208, 210, 38, 40, 19, 61, 252, 162, 170, 8, 145, 138, 245, 70, 14, 135, 246, 86, 157, 188, 23, 53, 237, 63, 102, 55, 65, 163, 152, 147, 174];
pub const VIDEO: [u8; 32] = [128, 238, 148, 86, 247, 95, 143, 138, 18, 5, 246, 74, 128, 185, 26, 91, 138, 86, 119, 115, 184, 252, 201, 25, 184, 94, 57, 43, 199, 176, 161, 189];
pub const DOCUMENT: [u8; 32] = [32, 225, 14, 187, 186, 180, 206, 11, 137, 109, 245, 135, 164, 30, 231, 175, 194, 18, 236, 45, 241, 249, 57, 246, 130, 198, 62, 38, 156, 241, 191, 129];
pub const SCAN: [u8; 32] = [68, 12, 139, 46, 45, 209, 158, 163, 173, 177, 180, 136, 30, 33, 86, 200, 16, 66, 74, 183, 195, 237, 32, 32, 230, 186, 177, 241, 91, 71, 180, 178];
pub const LIST: [u8; 32] = [115, 191, 174, 108, 87, 224, 138, 91, 71, 77, 70, 202, 140, 170, 226, 191, 100, 116, 82, 136, 96, 184, 91, 23, 213, 188, 17, 174, 44, 4, 43, 139];
pub const CONTACT: [u8; 32] = [75, 70, 179, 140, 250, 88, 36, 7, 82, 170, 48, 162, 74, 120, 24, 5, 233, 201, 201, 153, 243, 38, 245, 177, 241, 227, 250, 113, 243, 23, 48, 158];
pub const SONG: [u8; 32] = [130, 187, 147, 80, 45, 37, 79, 73, 219, 224, 142, 178, 28, 181, 55, 153, 205, 158, 202, 185, 44, 2, 236, 29, 179, 197, 248, 173, 198, 49, 73, 14];
pub const GPS_TRACK: [u8; 32] = [146, 234, 222, 142, 227, 27, 35, 157, 198, 134, 185, 247, 108, 139, 53, 205, 36, 238, 255, 90, 120, 111, 178, 21, 142, 210, 106, 179, 153, 102, 243, 87];
pub const FIND: [u8; 32] = [109, 119, 169, 82, 158, 100, 153, 214, 82, 11, 212, 42, 26, 213, 213, 167, 7, 116, 177, 186, 199, 80, 55, 5, 43, 242, 174, 122, 49, 232, 62, 21];
pub const COLLECTION: [u8; 32] = [55, 37, 163, 136, 32, 31, 148, 0, 129, 191, 83, 161, 228, 192, 232, 185, 213, 39, 3, 37, 79, 107, 209, 195, 40, 227, 5, 233, 6, 21, 255, 181];
pub const GENESIS_SAVE_STATE: [u8; 32] = [185, 51, 235, 74, 73, 115, 233, 253, 83, 242, 127, 49, 199, 15, 71, 31, 157, 96, 133, 90, 74, 42, 56, 168, 113, 99, 41, 243, 144, 183, 56, 28];
pub const GENESIS_BATTERY_RAM: [u8; 32] = [197, 171, 9, 230, 54, 152, 168, 156, 51, 173, 95, 43, 227, 254, 233, 226, 117, 94, 100, 122, 21, 62, 178, 102, 76, 18, 92, 113, 52, 175, 14, 236];

/// A kind's first phrasing, for a bench tool to name it (a person reads the idea's phrasing in their language once sheets carry them).
pub fn phrasing(kind: &[u8; 32]) -> Option<&'static str> {
    KINDS.iter().find(|(_, id)| id == kind).map(|(p, _)| *p)
}

/// A kind by its first phrasing, as a bench tool takes it.
pub fn by_phrasing(p: &str) -> Option<[u8; 32]> {
    KINDS.iter().find(|(w, _)| *w == p).map(|(_, id)| *id)
}

#[cfg(test)]
mod tests {
    #[test]
    fn kinds_are_their_first_phrasings() {
        for (words, id) in super::KINDS {
            let want = ferros_idea::id(*b"eng", words).unwrap();
            assert_eq!(*id, want, "{words}: {}", want.iter().map(|b| format!("{b}, ")).collect::<String>());
        }
    }
}
