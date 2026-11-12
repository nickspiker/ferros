//! Photon Transport over the link (docs/USB.md, ferros_pt): a command sent as one transfer (SPEC, the DATA blast, COMPLETE) and the device's answer received as one. One command at a time: the device takes no other until its answer has gone out.

use crate::usb::UsbLink;
use ferros_pt::packet::{self, Ack, Complete, Spec};
use ferros_pt::transfer::{bitmap_words, InboundTransfer, OutboundTransfer};
use ferros_pt::StreamId;

/// A transfer's progress lines, on stderr, only while tracing (the bridge's; `crate::trace`).
macro_rules! trace {
    ($($t:tt)*) => {
        if crate::traced() {
            eprintln!($($t)*);
        }
    };
}

/// A command for the capability named `name` (its hash, ferros_pt `dev_cap`: blake3 of the name): [cap 32][op][params].
pub fn command(name: &[u8], op: ferros_pt::Op, params: &[u8]) -> Vec<u8> {
    let cap = ferros_pt::command::dev_cap(name);
    let mut buf = vec![0u8; ferros_pt::command::CMD_MIN_SIZE + params.len()];
    ferros_pt::command::encode(&mut buf, &cap, op, params);
    buf
}

/// A command sent and its answer received.
pub async fn call(link: &UsbLink, cmd: &[u8]) -> Result<Vec<u8>, String> {
    send(link, cmd).await?;
    recv(link).await
}

/// Send data to the device via PT and wait for COMPLETE.
pub async fn send(link: &UsbLink, data: &[u8]) -> Result<Complete, String> {
    let sid = StreamId::FIRST;

    // The kernel takes at most 6 MiB in one transfer (its PT_BIG receive region, service.rs) and refuses more, answering COMPLETE-failed: said here first.
    if data.len() > 6 << 20 {
        return Err(format!("{} bytes is more than the kernel takes in one transfer (6 MiB)", data.len()));
    }
    let mut spec_buf = [0u8; 512];
    let mut bitmap_buf = vec![0u64; ferros_pt::transfer::outbound_bitmap_words(data.len())];

    let (mut xfer, spec_len) = OutboundTransfer::start(sid, data, &mut bitmap_buf, &mut spec_buf)
        .ok_or_else(|| "Failed to build SPEC".to_string())?;

    trace!(
        "  SPEC: sid={} count={} psize={} total={}",
        sid.0 as char, xfer.count, xfer.psize, xfer.total
    );

    trace!("  SPEC sending...");
    link.send(&spec_buf[..spec_len])
        .await
        .map_err(|e| format!("SPEC send failed: {e}"))?;
    trace!("  SPEC sent OK");

    // Wait for SPEC ACK. A packet of a reply an earlier session left on its way out is passed over: the device ends that reply when this session starts (`UsbLink::open`) or when this SPEC comes, but a packet the controller had already taken in can still come first (2026-11-12, a killed `ferros send`'s held read: `Expected SPEC ACK, got 512 bytes: [53, 61, ...]`, a SPEC).
    let resp = loop {
        let r = link
            .recv()
            .await
            .map_err(|e| format!("SPEC ACK recv failed: {e}"))?;
        if r.is_empty() || Spec::decode(&r).is_some() || ferros_pt::is_data_packet(r[0]) {
            trace!("  an earlier session's packet passed over ({} bytes)", r.len());
            continue;
        }
        break r;
    };
    if ferros_pt::packet::Complete::decode(&resp).is_some_and(|c| !c.success) {
        return Err("the kernel refused the transfer (too large for its receive region)".into());
    }
    if let Some(ack) = Ack::decode(&resp) {
        if ack.seq != u64::MAX {
            return Err(format!("Expected SPEC ACK (seq=MAX), got seq={}", ack.seq));
        }
        trace!("  SPEC ACK received");
    } else {
        return Err(format!(
            "Expected SPEC ACK, got {} bytes: {:02x?}",
            resp.len(),
            &resp[..resp.len().min(8)]
        ));
    }

    // Small delay after SPEC ACK — let kernel finish arming ep2
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    let pace_us: u64 = std::env::var("FERROS_PT_PACE_US").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    // Blast all DATA packets — no ACK wait, USB guarantees delivery. Each is padded to 512 bytes and they go out with several bulk transfers in flight (usb.rs `send_queued`), so the host never waits on this program between them (FERROS_PT_PACE_US > 0 goes back to one packet at a time, paced).
    let mut pkt_buf = [0u8; 512];
    let mut sent = 0u64;
    let mut batch: Vec<u8> = Vec::with_capacity(data.len() / 477 * 512 + 1024);

    while !xfer.all_sent() {
        let pkt_len = xfer.next_data_packet(data, &mut pkt_buf);
        if pkt_len == 0 {
            break;
        }
        sent += 1;
        if pace_us == 0 {
            let at = batch.len();
            batch.extend_from_slice(&pkt_buf[..pkt_len]);
            batch.resize(at + 512, 0);
            continue;
        }

        link.send(&pkt_buf[..pkt_len])
            .await
            .map_err(|e| format!("DATA send failed at pkt {sent}: {e}"))?;
        // No pacing by default (FERROS_PT_PACE_US, microseconds, to bring it back): the kernel takes one 512-byte packet per USB transfer and re-arms, and until it does the controller NAKs and the host retries, so nothing is lost. The 1 ms tokio sleep that was here (really 1-2 ms) held PT to 215 KB/s; without it 2 MiB went over in 0.62 s, byte-exact both ways (2026-10-31).
        if pace_us > 0 {
            tokio::time::sleep(std::time::Duration::from_micros(pace_us)).await;
        }
    }

    if !batch.is_empty() {
        link.send_queued(&batch).await.map_err(|e| format!("DATA send failed: {e}"))?;
    }
    trace!("  {} DATA packets blasted", sent);

    // Wait for COMPLETE. Kernel responds when all_received(). Timeout proportional to transfer size: ~1ms per packet + 2s base.
    let wait_ms = (1u64 << 15) + (sent << 6); // 32768ms base + 64ms per packet

    // Try multiple recv() calls — COMPLETE might not be the first thing back
    for attempt in 0..16u32 {
        match tokio::time::timeout(
            std::time::Duration::from_millis(if attempt == 0 { wait_ms } else { 4096 }),
            link.recv(),
        ).await {
            Ok(Ok(resp)) => {
                trace!("  recv[{attempt}]: {} bytes first=G#{:02X}", resp.len(), resp.get(0).copied().unwrap_or(0));
                if let Some(complete) = Complete::decode(&resp) {
                    xfer.handle_complete(&complete);
                    trace!("  COMPLETE received!");
                    return Ok(complete);
                }
                continue; // try next recv
            }
            Ok(Err(e)) => return Err(format!("recv error: {e}")),
            Err(_) => {
                trace!("  timeout on attempt {attempt}");
                if attempt == 0 {
                    continue; // long timeout expired, try short ones
                }
                return Err(format!("Timeout waiting for COMPLETE after {}ms ({} packets)", wait_ms, sent));
            }
        }
    }
    Err(format!("No COMPLETE after 16 recv attempts ({sent} packets)"))
}

/// Receive a PT transfer from the device (blast mode). Device sends SPEC + DATA blast + FIN. Bridge sends SPEC ACK, then responds with COMPLETE or NAK after all data received.
pub async fn recv(link: &UsbLink) -> Result<Vec<u8>, String> {
    // Read SPEC from device
    let resp = link
        .recv()
        .await
        .map_err(|e| format!("SPEC recv failed: {e}"))?;

    let spec = Spec::decode(&resp).ok_or_else(|| {
        format!(
            "Expected SPEC, got {} bytes: {:02x?}",
            resp.len(),
            &resp[..resp.len().min(8)]
        )
    })?;

    trace!(
        "  RECV SPEC: sid={} count={} psize={} total={}",
        spec.sid.0 as char, spec.count, spec.psize, spec.total
    );

    // Allocate buffers
    let mut data_buf = vec![0u8; spec.total as usize];
    let mut bitmap_buf = vec![0u64; bitmap_words(spec.count)];

    let mut xfer = InboundTransfer::new(&spec, &mut data_buf, &mut bitmap_buf)
        .ok_or_else(|| "Failed to create InboundTransfer".to_string())?;

    // Skip SPEC ACK — kernel starts blasting immediately via idle-poll pump. Sending SPEC ACK on OUT would block because the kernel's ep2 may be busy.
    trace!("  SPEC ACK skipped (kernel auto-blasts)");

    // Receive the DATA blast — silent, no ACKs. The device sends exactly `count` packets, each padded to 512 bytes, so they are read with several exact bulk transfers in flight (usb.rs `recv_queued`). Each ends when its last packet is in; asking for more than is coming would wait for packets that never come.
    let mut received = 0u64;
    let run = link
        .recv_queued(spec.count as usize * 512)
        .await
        .map_err(|e| format!("DATA recv failed: {e}"))?;
    for pkt in run.chunks(512) {
        if !ferros_pt::is_data_packet(pkt[0]) {
            trace!("  (unexpected packet during recv: G#{:02X})", pkt[0]);
            continue;
        }
        match packet::decode_data(pkt, xfer.seq_width) {
            Some((_sid, seq, chunk_hash, payload)) => {
                if xfer.handle_data(seq, &chunk_hash, payload) {
                    received += 1;
                }
            }
            None => trace!("  DATA decode failed"),
        }
    }
    let mut resp_buf = [0u8; 128];
    xfer.finish(&mut resp_buf);
    if !xfer.all_received() {
        return Err(format!("reply incomplete: {} of {} packets", xfer.chunks_received, xfer.expected_count));
    }

    trace!("  {} DATA packets received", received);

    let result = data_buf[..spec.total as usize].to_vec();
    Ok(result)
}
