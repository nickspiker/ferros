# BEAM — what a ferros beam has to speak, and the path there

A **beam** is Photon's live video, and it is not a call of its own: it is a video track inside a **wave** (a voice call). This is the contract ferros must meet to beam with the stock Photon app, mapped from the Photon source 2026-10-15 (paths under `photon/src/`; the design rationale is `photon/docs/beams.md`). Anything marked UNCONFIRMED is still to be proven against the app.

## Prerequisites

- A **CLUTCH friendship** with the peer: its `lane_root` and `history_key` (`crypto/clutch.rs`; era re-keys in `crypto/era.rs`). This needs the owner: the ferros device must be paired and enrolled.
- **Eagle time within ±30 s** of the peer (express signals outside that window are dropped).
- UDP on port 4383 (fallback 3546), on the same socket as photons. Not Photon Transport (PT).

## Signalling (`wave/signal.rs`, `types/row_control.rs`)

- Two paths, idempotent on `wave_id`: a **lane row** (an ordinary encrypted message, control kind `CTL_WAVE = 3`) and an **express datagram** `[G#C9][nonce 24][XChaCha20-Poly1305(payload)]`.
- Express payload: a VSF document, section `wexp`: `ts` (e6 eagle time), `lk` (hR, the offer's lane key, offers only), `bm` (u3, present = beam offer) and the row-control fields `cid` (16-byte wave id), `cn` (32-byte nonce), `cd` (device key).
- Express key: `blake3::derive_key("PHOTON_WAVE_v1 express signal", lane_root ‖ history_key)`.
- Signals: 1 Offer, 2 Answer, 3 Decline, 4 Busy, 5 Hangup, 6 Taken, 7 Anchor (express-only: "re-point your media at my source address"). Glare: the smaller `wave_id` wins. No timers.
- Photon sends its express Answer only after its lane-row send succeeds, so it must believe it has a working lane to us. Whether an express Offer from a ferros origin is enough without any lane row is UNCONFIRMED.

## Keys (`wave/keys.rs`, `wave/beam.rs`)

- Wave secret: `derive_key("PHOTON_WAVE_v1 wave secret", offer_lane_key ‖ wave_id ‖ origin_nonce ‖ answer_nonce)` (known-answer test at `keys.rs:133`).
- Per direction: step 0 = `derive_key("PHOTON_WAVE_v1 direction step0", secret ‖ "o>a" | "a>o")`; every 100 packets `key = derive_key("PHOTON_WAVE_v1 step", key)`, old key destroyed.
- Beam root: `derive_key("PHOTON_WAVE_v1 beam video", wave_secret)`, with its own direction chains. The origin sends origin→answer.

## Media packets (`wave/packet.rs`)

- `[magic 1][seq u32 LE][ciphertext][tag 4]`. Magic: G#C7 audio, G#C8 recording fill, G#CA beam, G#C9 express.
- XChaCha20, RFC 8439 composition (Poly1305 key from block 0, payload from block 1, no associated data); the tag is the **first 4 bytes** of the Poly1305 tag (pinned at `packet.rs:152`).
- Nonce: `seq` LE in bytes 0..4, rest zero. Step = `seq / 100`. A beam's `seq` starts at 0 once per wave per direction and never restarts.

## Beam video (`wave/beam.rs`, `wave/beam_session.rs`)

- Payload kinds: **Data 0** `[0][frame_no u32][au_len u32][gains 4×u16 Q12 R,Gr,Gb,B; 4096 = none][RaptorQ EncodingPacket]` (≤1160-byte symbols, datagram ≤1190); **Info 1** `[1][w u16][h u16][fps u8][codec u8: 1 = H.264][VSF colour_profile, may be empty]`, at start and every 30 frames; **Orient 2** `[2][quarter turns clockwise]`.
- One access unit = one RaptorQ (RFC 6330) object, `ObjectTransmissionInformation::with_defaults(len, 1160)`, repair = ⌈source/8⌉. No retransmit, no NACK. The receiver drops a frame 8 frames behind and decodes in order.
- Codec: **H.264 Annex-B**. Photon desktop decodes with OpenH264 (error concealment on). An IDR plus intra refresh, or all-intra, is valid. No A/V sync: the newest decoded picture is shown.
- Colour: YCbCr 4:2:0 as 601 full range; the phone's ISP tonemaps γ2 with AWB off and sends per-frame gains. Empty colour bytes → identity + γ2.
- Sender-local, not on the wire: `beam_ladder` (size bands, softness, fps, bitrate), `beam_scale`.

## Audio (`wave/engine.rs`) — mandatory

Photon's liveness and its TX-address learning key **only on authenticated audio**: a beam with no audio leaves it "reconnecting" forever.

- 48 kHz mono, 5 ms frames (240 samples). Rungs: Opus CELT 16k / 32k / 64k / 128k, and **"plaid" = raw i16 LE PCM, no FEC** — the receiver accepts plaid with no LAN check, so ferros can send audio with no Opus.
- Sealed payload `[ctrl][fno_src u32][fno_rep u32?][source][repair?][tail]`, `ctrl` = tier_src (3 bits) | rep_present (bit 3) | tier_rep (bits 4..7); slots `[len u16 LE][bytes]` zero-padded to `2 + TIER_MAX_ENC` (`[12, 22, 42, 82, 486]`); tail 10/11/13 bytes (µs stamp u32, echo u32, hold u16, then optional peer-loss u8, latency u16). Exact lengths only.
- `fno` = eagle-time sample index / 240 mod 2³²: the sender needs real time.
- Receiving Photon's audio needs an Opus decoder (it starts at 16k).

## What ferros needs, in order

1. **Wave crypto in `photon_core`** (keys, packet seal/open, express signal), no_std, cross-checked byte for byte against Photon's library (`tools/photon-interop`).
2. **Beam + plaid-audio packet codecs** and RaptorQ (the `raptorq` crate has a no_std cfg), cross-checked by feeding our packets to Photon's receivers.
3. **An H.264 encoder** (pure Rust, no_std): all-intra Baseline first, proven by Photon's own decoder decoding it.
4. **A beam from husky to a receiver on the dev box** built on Photon's library, under a test wave secret — the whole send path on hardware.
5. With the owner: pairing + CLUTCH, then a real wave with the stock app; then receive (H.264 decoder, Opus decoder), the camera, and the display.

Missing entirely today: a pure-Rust H.264 decoder and encoder, an Opus decoder, the camera.
