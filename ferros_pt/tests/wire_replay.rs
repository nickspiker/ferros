//! Host-side replay of the exact bridge→kernel wire sequence for a DIAG command.
//! Every packet is padded to 512 bytes before the receiving side sees it, exactly as the USB transport delivers it.
//! This bisects the Pixel 8 DATA→COMPLETE hang: if this passes, the protocol logic is sound and the bug lives in the DWC3 layer.

use ferros_pt::packet::{self, Ack, Complete, Spec};
use ferros_pt::transfer::{InboundTransfer, OutboundTransfer};
use ferros_pt::StreamId;

// Pad a packet to 512 bytes of wire data, as UsbLink::send does.
fn pad512(pkt: &[u8]) -> [u8; 512] {
    let mut wire = [0u8; 512];
    wire[..pkt.len()].copy_from_slice(pkt);
    wire
}

fn build_diag_cmd() -> Vec<u8> {
    let cap = ferros_pt::command::dev_cap(ferros_pt::command::caps::DIAG);
    let mut buf = vec![0u8; 33];
    let n = ferros_pt::command::encode(&mut buf, &cap, ferros_pt::Op::Read, &[]);
    assert_eq!(n, 33);
    buf
}

#[test]
fn diag_send_replay() {
    // ---- Bridge side: build SPEC exactly as pt_send does ----
    let data = build_diag_cmd();
    let sid = StreamId::FIRST;
    let mut spec_buf = [0u8; 512];
    let mut bitmap_buf = vec![0u64; ferros_pt::transfer::outbound_bitmap_words(data.len())];
    let (mut xfer_out, spec_len) = OutboundTransfer::start(sid, &data, &mut bitmap_buf, &mut spec_buf)
        .expect("SPEC build failed");
    eprintln!("SPEC: count={} psize={} total={} spec_len={}", xfer_out.count, xfer_out.psize, xfer_out.total, spec_len);

    // ---- Wire: SPEC padded to 512 ----
    let wire_spec = pad512(&spec_buf[..spec_len]);

    // ---- Kernel side: exactly what kernel_main does on ep2 TransferComplete ----
    let n = 512usize;
    assert!(ferros_pt::is_control_packet(wire_spec[0]), "SPEC first byte not control: {:02x}", wire_spec[0]);
    let spec = Spec::decode(&wire_spec[..n]).expect("kernel Spec::decode failed on padded SPEC");
    let pt_seq_width = ferros_ledger::ewe::seq_width(spec.count);
    eprintln!("kernel: decoded SPEC count={} psize={} total={} seq_width={}", spec.count, spec.psize, spec.total, pt_seq_width);

    let bmw = ferros_pt::transfer::outbound_bitmap_words(spec.count as usize);
    let mut pt_data_buf = vec![0u8; spec.total as usize];
    let mut pt_bitmap_buf = vec![0u64; bmw];
    let mut xfer_in = InboundTransfer::new(&spec, &mut pt_data_buf, &mut pt_bitmap_buf)
        .expect("kernel InboundTransfer::new returned None");

    // Kernel sends SPEC ACK; bridge decodes it (padded on the way back too — device sends exact len, but be strict and test both).
    let ack = Ack { sid: spec.sid, seq: u64::MAX };
    let mut ack_buf = [0u8; 64];
    let ack_len = ack.encode(&mut ack_buf);
    assert!(ack_len > 0, "ACK encode returned 0");
    let ack_rx = Ack::decode(&ack_buf[..ack_len]).expect("bridge Ack::decode failed");
    assert_eq!(ack_rx.seq, u64::MAX, "bridge would reject: SPEC ACK seq != MAX");

    // ---- Bridge blasts DATA packets; kernel handles each from padded wire bytes ----
    let mut pkt_buf = [0u8; 512];
    let mut sent = 0u64;
    while !xfer_out.all_sent() {
        let pkt_len = xfer_out.next_data_packet(&data, &mut pkt_buf);
        assert!(pkt_len > 0, "next_data_packet returned 0 before all_sent");
        let wire = pad512(&pkt_buf[..pkt_len]);
        sent += 1;

        // Kernel side, verbatim from kernel_main.
        assert!(ferros_pt::is_data_packet(wire[0]), "DATA first byte not data: {:02x}", wire[0]);
        let (_sid, seq, hash, payload) = packet::decode_data(&wire[..512], pt_seq_width)
            .expect("kernel decode_data failed on padded DATA");
        eprintln!("kernel: DATA seq={} payload_len={} (padded)", seq, payload.len());
        let accepted = xfer_in.handle_data(seq, &hash, payload);
        assert!(accepted, "kernel handle_data rejected chunk seq={} (hash mismatch on padded payload?)", seq);
    }
    eprintln!("blasted {} DATA packets", sent);

    // ---- Kernel: all_received → finish → COMPLETE ----
    assert!(xfer_in.all_received(), "kernel all_received() false after all chunks (chunks_received={} expected={})", xfer_in.chunks_received, xfer_in.expected_count);
    let mut complete_buf = [0u8; 512];
    let clen = xfer_in.finish(&mut complete_buf);
    assert!(clen > 0, "kernel finish() returned 0 — no COMPLETE encoded");

    // ---- Bridge: decode COMPLETE (device sends exact clen bytes) ----
    let complete = Complete::decode(&complete_buf[..clen]).expect("bridge Complete::decode failed");
    assert!(complete.success, "COMPLETE says root hash mismatch");

    // ---- Kernel dispatch: payload must parse as the DIAG command ----
    let payload = xfer_in.payload();
    assert_eq!(payload, &data[..], "reassembled payload differs from sent command");
    let cmd = ferros_pt::command::parse(payload).expect("kernel command::parse failed");
    assert_eq!(cmd.cap, ferros_pt::command::dev_cap(ferros_pt::command::caps::DIAG));
    assert_eq!(cmd.op, ferros_pt::Op::Read);
}

#[test]
fn multi_chunk_send_replay() {
    // Same replay with a payload big enough to need multiple chunks — covers the RELOAD path shape.
    let data: Vec<u8> = (0..4096u32).flat_map(|i| i.to_le_bytes()).collect();
    let sid = StreamId::FIRST;
    let mut spec_buf = [0u8; 512];
    let mut bitmap_buf = vec![0u64; ferros_pt::transfer::outbound_bitmap_words(data.len())];
    let (mut xfer_out, spec_len) = OutboundTransfer::start(sid, &data, &mut bitmap_buf, &mut spec_buf)
        .expect("SPEC build failed");
    eprintln!("SPEC: count={} psize={} total={}", xfer_out.count, xfer_out.psize, xfer_out.total);

    let wire_spec = pad512(&spec_buf[..spec_len]);
    let spec = Spec::decode(&wire_spec[..512]).expect("Spec::decode failed on padded SPEC");
    let pt_seq_width = ferros_ledger::ewe::seq_width(spec.count);

    let bmw = ferros_pt::transfer::outbound_bitmap_words(spec.count as usize);
    let mut pt_data_buf = vec![0u8; spec.total as usize];
    let mut pt_bitmap_buf = vec![0u64; bmw];
    let mut xfer_in = InboundTransfer::new(&spec, &mut pt_data_buf, &mut pt_bitmap_buf)
        .expect("InboundTransfer::new returned None");

    let mut pkt_buf = [0u8; 512];
    while !xfer_out.all_sent() {
        let pkt_len = xfer_out.next_data_packet(&data, &mut pkt_buf);
        assert!(pkt_len > 0);
        let wire = pad512(&pkt_buf[..pkt_len]);
        let (_sid, seq, hash, payload) = packet::decode_data(&wire[..512], pt_seq_width)
            .expect("decode_data failed on padded DATA");
        assert!(xfer_in.handle_data(seq, &hash, payload), "handle_data rejected seq={}", seq);
    }

    assert!(xfer_in.all_received());
    let mut complete_buf = [0u8; 512];
    let clen = xfer_in.finish(&mut complete_buf);
    assert!(clen > 0, "finish() returned 0");
    let complete = Complete::decode(&complete_buf[..clen]).expect("Complete::decode failed");
    assert!(complete.success);
    assert_eq!(xfer_in.payload(), &data[..]);
}
