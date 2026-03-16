mod usb;

use ferros_pt::StreamId;
use ferros_pt::packet::{self, Ack, Complete, Spec};
use ferros_pt::transfer::{InboundTransfer, OutboundTransfer, bitmap_words};
use std::io::{self, Write};

fn usage() {
    eprintln!("ferros-bridge — USB bridge for ferros kernel");
    eprintln!();
    eprintln!("Usage: ferros-bridge <command>");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  status       Check if device is connected");
    eprintln!("  diag         Retrieve boot diagnostics from device");
    eprintln!("  read <addr> [len]  Read MMIO (hex addr, optional len, default 4)");
    eprintln!("  echo         PT echo test — send pattern, verify round-trip");
    eprintln!("  send <hex>   Send hex bytes via PT transfer");
    eprintln!("  log          Stream bulk IN data from device to stdout");
    eprintln!("  reboot [fastboot]  Reboot device (default: normal, 'fastboot' for bootloader)");
    eprintln!("  reload <kernel>   Hot-reload kernel binary (ELF path, runs mkimg internally)");
    eprintln!("  ping [count]      Raw USB ping-pong test (no PT, default 200)");
    eprintln!("  terminal     Bidirectional PT session");
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
        std::process::exit(1);
    }

    match args[1].as_str() {
        "status" => cmd_status(),
        "diag" => cmd_diag().await,
        "read" => {
            if args.len() < 3 {
                eprintln!("Usage: ferros-bridge read <hex-addr> [len]");
                std::process::exit(1);
            }
            let len = args
                .get(3)
                .and_then(|s| usize::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .unwrap_or(4);
            cmd_read(&args[2], len).await;
        }
        "echo" => cmd_echo().await,
        "send" => {
            if args.len() < 3 {
                eprintln!("Usage: ferros-bridge send <hex-bytes>");
                std::process::exit(1);
            }
            cmd_send(&args[2]).await;
        }
        "log" => cmd_log().await,
        "reboot" => {
            let mode = args.get(2).map(|s| s.as_str()).unwrap_or("normal");
            cmd_reboot(mode).await;
        }
        "reload" => {
            if args.len() < 3 {
                eprintln!("Usage: ferros-bridge reload <kernel-binary>");
                std::process::exit(1);
            }
            cmd_reload(&args[2]).await;
        }
        "terminal" => cmd_terminal().await,
        _ => {
            eprintln!("Unknown command: {}", args[1]);
            usage();
            std::process::exit(1);
        }
    }
}

fn cmd_status() {
    match usb::UsbLink::open() {
        Ok(_link) => println!("Device connected and ready"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

// ---------------------------------------------------------------------------
// PT send (host→device) — unchanged from before
// ---------------------------------------------------------------------------

/// Send data to the device via PT and wait for COMPLETE.
async fn pt_send(link: &usb::UsbLink, data: &[u8]) -> Result<Complete, String> {
    let sid = StreamId::FIRST;

    let mut spec_buf = [0u8; 512];
    let mut bitmap_buf = vec![0u64; ferros_pt::transfer::outbound_bitmap_words(data.len())];

    let (mut xfer, spec_len) = OutboundTransfer::start(sid, data, &mut bitmap_buf, &mut spec_buf)
        .ok_or_else(|| "Failed to build SPEC".to_string())?;

    eprintln!(
        "  SPEC: sid={} count={} psize={} total={}",
        sid.0 as char, xfer.count, xfer.psize, xfer.total
    );

    eprintln!("  SPEC sending...");
    link.send(&spec_buf[..spec_len])
        .await
        .map_err(|e| format!("SPEC send failed: {e}"))?;
    eprintln!("  SPEC sent OK");

    // Wait for SPEC ACK
    let resp = link
        .recv()
        .await
        .map_err(|e| format!("SPEC ACK recv failed: {e}"))?;
    if let Some(ack) = Ack::decode(&resp) {
        if ack.seq != u64::MAX {
            return Err(format!("Expected SPEC ACK (seq=MAX), got seq={}", ack.seq));
        }
        eprintln!("  SPEC ACK received");
    } else {
        return Err(format!(
            "Expected SPEC ACK, got {} bytes: {:02x?}",
            resp.len(),
            &resp[..resp.len().min(8)]
        ));
    }

    // Small delay after SPEC ACK — let kernel finish arming ep2
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    // Blast all DATA packets — no ACK wait, USB guarantees delivery
    let mut pkt_buf = [0u8; 512];
    let mut sent = 0u64;

    while !xfer.all_sent() {
        let pkt_len = xfer.next_data_packet(data, &mut pkt_buf);
        if pkt_len == 0 {
            break;
        }

        link.send(&pkt_buf[..pkt_len])
            .await
            .map_err(|e| format!("DATA send failed at pkt {sent}: {e}"))?;
        sent += 1;
        // Pace sends — kernel needs time to process + re-arm ep2
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }

    eprintln!("  {} DATA packets blasted", sent);

    // Wait for COMPLETE. Kernel responds when all_received().
    // Timeout proportional to transfer size: ~1ms per packet + 2s base.
    let wait_ms = (1u64 << 15) + (sent << 6); // 32768ms base + 64ms per packet

    // Try multiple recv() calls — COMPLETE might not be the first thing back
    for attempt in 0..16u32 {
        match tokio::time::timeout(
            std::time::Duration::from_millis(if attempt == 0 { wait_ms } else { 4096 }),
            link.recv(),
        ).await {
            Ok(Ok(resp)) => {
                eprintln!("  recv[{attempt}]: {} bytes first=0x{:02x}", resp.len(), resp.get(0).copied().unwrap_or(0));
                if let Some(complete) = Complete::decode(&resp) {
                    xfer.handle_complete(&complete);
                    eprintln!("  COMPLETE received!");
                    return Ok(complete);
                }
                continue; // try next recv
            }
            Ok(Err(e)) => return Err(format!("recv error: {e}")),
            Err(_) => {
                eprintln!("  timeout on attempt {attempt}");
                if attempt == 0 {
                    continue; // long timeout expired, try short ones
                }
                return Err(format!("Timeout waiting for COMPLETE after {}ms ({} packets)", wait_ms, sent));
            }
        }
    }
    Err(format!("No COMPLETE after 16 recv attempts ({sent} packets)"))
} // pt_send

// ---------------------------------------------------------------------------
// PT recv (device→host) — bridge acts as InboundTransfer receiver
// ---------------------------------------------------------------------------

/// Receive a PT transfer from the device (blast mode).
/// Device sends SPEC + DATA blast + FIN. Bridge sends SPEC ACK, then
/// responds with COMPLETE or NAK after all data received.
async fn pt_recv(link: &usb::UsbLink) -> Result<Vec<u8>, String> {
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

    eprintln!(
        "  RECV SPEC: sid={} count={} psize={} total={}",
        spec.sid.0 as char, spec.count, spec.psize, spec.total
    );

    // Allocate buffers
    let mut data_buf = vec![0u8; spec.total as usize];
    let mut bitmap_buf = vec![0u64; bitmap_words(spec.count)];

    let mut xfer = InboundTransfer::new(&spec, &mut data_buf, &mut bitmap_buf)
        .ok_or_else(|| "Failed to create InboundTransfer".to_string())?;

    // Skip SPEC ACK — kernel starts blasting immediately via idle-poll pump.
    // Sending SPEC ACK on OUT would block because the kernel's ep2 may be busy.
    eprintln!("  SPEC ACK skipped (kernel auto-blasts)");

    // Receive DATA blast — silent, no ACKs
    let mut received = 0u64;

    loop {
        let resp = link
            .recv()
            .await
            .map_err(|e| format!("DATA recv failed: {e}"))?;

        if resp.is_empty() {
            eprintln!("  recv: empty");
            continue;
        }

        eprintln!("  recv: {} bytes first=G#{:02x}", resp.len(), resp[0]);

        if ferros_pt::is_data_packet(resp[0]) {
            if let Some((_sid, seq, chunk_hash, payload)) =
                packet::decode_data(&resp, xfer.seq_width)
            {
                let ok = xfer.handle_data(seq, &chunk_hash, payload);
                if ok { received += 1; }
                eprintln!("  DATA seq={} ok={} rcv={}/{}", seq, ok, xfer.chunks_received, xfer.expected_count);
            } else {
                eprintln!("  DATA decode failed");
            }

            // All chunks received? Verify and return (no COMPLETE send needed for response direction)
            if xfer.all_received() {
                let mut resp_buf = [0u8; 128];
                xfer.finish(&mut resp_buf); // verify hash
                break;
            }
        } else if resp[0] == b'F' {
            // FIN — sender says blast is done. Check what we have.
            let mut resp_buf = [0u8; 128];
            xfer.finish(&mut resp_buf);
            if xfer.state == ferros_pt::TransferState::Done {
                break;
            }
            // If missing chunks, keep receiving (can't NAK without OUT send)
        } else {
            eprintln!("  (unexpected packet during recv: {:02x})", resp[0]);
        }
    }

    eprintln!("  {} DATA packets received", received);

    let result = data_buf[..spec.total as usize].to_vec();
    Ok(result)
}

// ---------------------------------------------------------------------------
// Command helpers — build cap-addressed payloads
// ---------------------------------------------------------------------------

fn build_cmd(cap_name: &[u8], op: ferros_pt::Op, params: &[u8]) -> Vec<u8> {
    let cap = ferros_pt::command::dev_cap(cap_name);
    let mut buf = vec![0u8; 33 + params.len()];
    ferros_pt::command::encode(&mut buf, &cap, op, params);
    buf
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

async fn cmd_diag() {
    let link = match usb::UsbLink::open() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    // Send DIAG Read command
    let cmd = build_cmd(ferros_pt::command::caps::DIAG, ferros_pt::Op::Read, &[]);
    eprintln!("Sending DIAG Read ({} bytes)", cmd.len());

    match pt_send(&link, &cmd).await {
        Ok(complete) => {
            if !complete.success {
                eprintln!("Device reported command failure");
                std::process::exit(1);
            }
            eprintln!("Command accepted, receiving response...");
        }
        Err(e) => {
            eprintln!("Command send failed: {e}");
            std::process::exit(1);
        }
    }

    // Receive response via PT
    match pt_recv(&link).await {
        Ok(data) => {
            eprintln!("Received {} bytes", data.len());
            let _ = io::stdout().write_all(&data);
            let _ = io::stdout().flush();
        }
        Err(e) => {
            eprintln!("Response receive failed: {e}");
            std::process::exit(1);
        }
    }
}

async fn cmd_read(addr_str: &str, len: usize) {
    let link = match usb::UsbLink::open() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let addr = u64::from_str_radix(addr_str.trim_start_matches("0x"), 16).unwrap_or_else(|_| {
        eprintln!("Invalid hex address: {addr_str}");
        std::process::exit(1);
    });

    // Build MEM Read command: params = [addr:8][len:4]
    let mut params = [0u8; 12];
    params[0..8].copy_from_slice(&addr.to_be_bytes());
    params[8..12].copy_from_slice(&(len as u32).to_be_bytes());

    let cmd = build_cmd(ferros_pt::command::caps::MEM, ferros_pt::Op::Read, &params);
    eprintln!("Reading {} bytes from G#{:X}", len, addr);

    match pt_send(&link, &cmd).await {
        Ok(complete) => {
            if !complete.success {
                eprintln!("Device reported command failure");
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("Command send failed: {e}");
            std::process::exit(1);
        }
    }

    match pt_recv(&link).await {
        Ok(data) => {
            // Hex dump
            for (i, chunk) in data.chunks(16).enumerate() {
                print!("{:08X}  ", addr as usize + i * 16);
                for b in chunk {
                    print!("{:02X} ", b);
                }
                println!();
            }
        }
        Err(e) => {
            eprintln!("Response receive failed: {e}");
            std::process::exit(1);
        }
    }
}

async fn cmd_echo() {
    let link = match usb::UsbLink::open() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let test_data: Vec<u8> = (0..256u16).map(|i| (i & 0xFF) as u8).collect();
    let data_hash = blake3::hash(&test_data);

    eprintln!(
        "PT echo: {} bytes, BLAKE3={}",
        test_data.len(),
        data_hash.to_hex()
    );

    match pt_send(&link, &test_data).await {
        Ok(complete) => {
            if complete.data_hash == *data_hash.as_bytes() {
                println!("Layer 2 OK: transfer BLAKE3 matches");
            } else {
                eprintln!("Layer 2 FAIL: hash mismatch");
            }

            if complete.success {
                println!("COMPLETE: success");
            } else {
                eprintln!("COMPLETE: device reported failure");
            }
        }
        Err(e) => {
            eprintln!("PT transfer failed: {e}");
            std::process::exit(1);
        }
    }
}

async fn cmd_send(hex: &str) {
    let data = match hex_decode(hex) {
        Some(d) => d,
        None => {
            eprintln!("Invalid hex string: {hex}");
            std::process::exit(1);
        }
    };

    let link = match usb::UsbLink::open() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    match pt_send(&link, &data).await {
        Ok(complete) => {
            if complete.success {
                println!("Sent {} bytes via PT — COMPLETE OK", data.len());
            } else {
                eprintln!("Sent {} bytes but device reported failure", data.len());
            }
        }
        Err(e) => eprintln!("PT send failed: {e}"),
    }
}

async fn cmd_log() {
    let link = match usb::UsbLink::open() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    eprintln!("Streaming bulk IN... (Ctrl-C to stop)");
    loop {
        match link.recv().await {
            Ok(data) if !data.is_empty() => {
                let _ = io::stdout().write_all(&data);
                let _ = io::stdout().flush();
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!("\nUSB read error: {e}");
                break;
            }
        }
    }
}

async fn cmd_reload(path: &str) {
    // Convert ELF to flat binary via objcopy, or read raw binary directly
    let bin_data = if path.ends_with(".bin") || path.ends_with(".img") {
        // Raw binary file
        std::fs::read(path).unwrap_or_else(|e| {
            eprintln!("Failed to read {path}: {e}");
            std::process::exit(1);
        })
    } else {
        // Assume ELF — run objcopy to get flat binary via temp file
        eprintln!("Converting ELF to flat binary...");
        let tmp = format!("/tmp/ferros_reload_{}.bin", std::process::id());
        let result = std::process::Command::new("llvm-objcopy")
            .args(["-O", "binary", path, &tmp])
            .status()
            .or_else(|_| {
                std::process::Command::new("rust-objcopy")
                    .args(["-O", "binary", path, &tmp])
                    .status()
            });
        match result {
            Ok(s) if s.success() => {
                let data = std::fs::read(&tmp).unwrap_or_else(|e| {
                    eprintln!("Failed to read {tmp}: {e}");
                    std::process::exit(1);
                });
                let _ = std::fs::remove_file(&tmp);
                data
            }
            _ => {
                eprintln!("objcopy failed. Install llvm-objcopy, or pass a raw .bin file");
                std::process::exit(1);
            }
        }
    };

    // Validate: check for MZ magic (0x91005A4D)
    if bin_data.len() < 0x1000 {
        eprintln!("Binary too small: {} bytes", bin_data.len());
        std::process::exit(1);
    }
    let magic = u32::from_le_bytes([bin_data[0], bin_data[1], bin_data[2], bin_data[3]]);
    if magic != 0x91005A4D {
        eprintln!("Bad magic: G#{magic:08X} (expected G#91005A4D MZ header)");
        std::process::exit(1);
    }

    eprintln!("Kernel binary: {} bytes", bin_data.len());

    let link = match usb::UsbLink::open() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    // Step 1: Send binary as RELOAD Write
    let mut cmd = build_cmd(
        ferros_pt::command::caps::RELOAD,
        ferros_pt::Op::Write,
        &bin_data,
    );
    eprintln!("Sending kernel ({} bytes)...", bin_data.len());
    match pt_send(&link, &cmd).await {
        Ok(complete) => {
            if !complete.success {
                eprintln!("Device rejected kernel write");
                std::process::exit(1);
            }
            eprintln!("Write accepted, receiving ack...");
        }
        Err(e) => {
            eprintln!("Transfer failed: {e}");
            std::process::exit(1);
        }
    }
    match pt_recv(&link).await {
        Ok(resp) => {
            if resp.len() >= 4 {
                let total = u32::from_le_bytes([resp[0], resp[1], resp[2], resp[3]]);
                eprintln!("Device staged {} bytes", total);
            }
        }
        Err(e) => {
            eprintln!("Response receive failed: {e}");
            std::process::exit(1);
        }
    }

    // Step 2: Send RELOAD Exec to trigger jump
    cmd = build_cmd(ferros_pt::command::caps::RELOAD, ferros_pt::Op::Exec, &[]);
    eprintln!("Executing reload...");
    match pt_send(&link, &cmd).await {
        Ok(_) => println!("Reload triggered — new kernel booting"),
        Err(e) => {
            if e.contains("recv") || e.contains("transfer") {
                println!("Device is reloading");
            } else {
                eprintln!("Exec failed: {e}");
            }
        }
    }
}

async fn cmd_reboot(mode: &str) {
    let link = match usb::UsbLink::open() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let mode_byte: u8 = match mode {
        "fastboot" | "bootloader" => 0x01,
        "normal" => 0x00,
        other => {
            eprintln!("Unknown reboot mode: {other} (use 'normal' or 'fastboot')");
            std::process::exit(1);
        }
    };

    let cmd = build_cmd(
        ferros_pt::command::caps::REBOOT,
        ferros_pt::Op::Exec,
        &[mode_byte],
    );

    eprintln!("Rebooting device (mode={mode})...");

    // Send command — device will reboot immediately, so we won't get COMPLETE
    match pt_send(&link, &cmd).await {
        Ok(_) => println!("Reboot command sent"),
        Err(e) => {
            // USB disconnect during reboot is expected
            if e.contains("recv") || e.contains("transfer") {
                println!("Device is rebooting");
            } else {
                eprintln!("Send failed: {e}");
            }
        }
    }
}

async fn cmd_terminal() {
    let link = match usb::UsbLink::open() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    eprintln!("Terminal mode (PT): type input, Enter to send (Ctrl-C to exit)");

    let mut line = String::new();
    loop {
        line.clear();
        eprint!("> ");
        let _ = io::stderr().flush();
        match io::stdin().read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                match pt_send(&link, trimmed.as_bytes()).await {
                    Ok(complete) => {
                        if complete.success {
                            println!("OK ({} bytes)", trimmed.len());
                        } else {
                            eprintln!("Device reported failure");
                        }
                    }
                    Err(e) => {
                        eprintln!("PT send failed: {e}");
                        break;
                    }
                }
            }
            Err(e) => {
                eprintln!("stdin error: {e}");
                break;
            }
        }
    }
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}
