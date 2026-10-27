// FERROS-BRIDGE SOURCE MAP — keep updated when commands change
//
// main.rs ── CLI entry point, PT send/recv, command dispatch Commands: status — USB VID/PID probe diag — boot log via DIAG cap read <addr> [len] — MMIO via MEM cap log — stream bulk IN echo/send — PT test transfers reboot [fastboot] — REBOOT cap reload <kernel> — hot-reload via RELOAD cap install <signed> — UFS + stem via INSTALL cap beam <ring> — pull ring blocks via BEAM cap terminal — interactive PT session. See BRIDGE.md for full reference. (ping is in usage() but unwired.)
//
//   pt_send(link, sid, data) — blast DATA packets with 1ms pacing pt_recv(link, sid) → Vec<u8> — receive DATA blast, no outbound ACK
//
// usb.rs ── nusb USB link (VID G#1209, PID G#4665) struct UsbLink { interface, ep_out, ep_in } ::open() → Result<Self> ::send(data) — pads to 512 bytes (DWC3 short packet workaround) ::recv() → Vec<u8> ::recv_timeout(duration) → Result<Vec<u8>>

mod usb;

use ferros_pt::StreamId;
use ferros_pt::packet::{self, Ack, Complete, Spec};
use ferros_pt::transfer::{InboundTransfer, OutboundTransfer, bitmap_words};
use std::io::{self, Write};

/// Session clock — calibrated once at startup via nunc, advanced locally via Instant.
struct SessionClock {
    base_et:      i64,
    base_instant: std::time::Instant,
}

impl SessionClock {
    /// None when too few time sources answer. Nothing on the device path needs the session clock yet, so a flaky network must never stop a USB command (unattended hardware loops died on "got 4, need 8").
    async fn calibrate() -> Option<Self> {
        match nunc::query(nunc::Mode::Fast).await {
            Ok(t) => Some(Self { base_et: t.timestamp_et, base_instant: std::time::Instant::now() }),
            Err(e) => {
                eprintln!("nunc: clock calibration failed ({e}); continuing without a session clock");
                None
            }
        }
    }

    /// Current eagle_time — base + elapsed oscillations since calibration.
    fn now(&self) -> i64 {
        let elapsed_ns = self.base_instant.elapsed().as_nanos() as i64;
        self.base_et + elapsed_ns * nunc::OPS / 1_000_000_000
    }
}

fn usage() {
    eprintln!("ferros-bridge — USB bridge for ferros kernel");
    eprintln!();
    eprintln!("Usage: ferros-bridge <command>");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  status       Check if device is connected");
    eprintln!("  diag [ufs]   Retrieve boot diagnostics from device (ufs = run the cold UFS link-startup probe)");
    eprintln!("  read <addr> [len]  Read MMIO (hex addr, optional len, default 4)");
    eprintln!("  echo         PT echo test — send pattern, verify round-trip");
    eprintln!("  send <hex>   Send hex bytes via PT transfer");
    eprintln!("  log          Stream bulk IN data from device to stdout");
    eprintln!("  reboot [fastboot]  Reboot device (default: normal, 'fastboot' for bootloader)");
    eprintln!("  reload <kernel>   Hot-reload kernel binary (ELF path, runs mkimg internally)");
    eprintln!("  install <kernel.signed>  Install signed kernel to UFS + stem entry");
    eprintln!("  ping [count]      Raw USB ping-pong test (no PT, default 256)");
    eprintln!("  terminal     Bidirectional PT session");
    eprintln!("  gate <ferros|android|hold>  Answer the Ready? gate from the host (dev builds have USB up at the gate)");
    eprintln!("  dbg          Read PT dispatch debug counters via EP0 (works even if bulk path is wedged)");
    eprintln!("  dbg2         Read raw DWC3 event ring + TRB snapshots via EP0");
    eprintln!("  run <payload.bin> [hex]  Hot-load a payload blob, call it, print its output");
    eprintln!("  store put <key> <text|@file> | get <key> [out] | ls | rm <key>   Vault by logical key (shim)");
    eprintln!("  display [--hold] [text]  Render text on the panel (shim after a --live handoff; '\\n' = newline; --hold keeps it up until a REBOOT; both volume keys = killswitch power-off)");
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
        std::process::exit(1);
    }

    // Calibrate in the background: nothing on the device path reads the session clock yet, and a slow or failing nunc query (seconds to a minute when sources don't answer) must never delay a USB command.
    let _clock = tokio::spawn(SessionClock::calibrate());

    match args[1].as_str() {
        "status" => cmd_status(),
        // `diag ufs` runs the cold boot's UFS link-startup probe (long report).
        "diag" => cmd_diag(args.get(2).map(|s| s.as_bytes()).unwrap_or(&[])).await,
        "read" => {
            if args.len() < 3 {
                eprintln!("Usage: ferros-bridge read <hex-addr> [hex-len] [out-file]  (out-file gets the raw bytes instead of a hexdump; 'priv:<addr>' reads through the EL3 monitor — use it for the PMU: on husky a direct PMU read makes the next EL3 call hang)");
                std::process::exit(1);
            }
            let len = args
                .get(3)
                .and_then(|s| usize::from_str_radix(s.trim_start_matches("0x"), 16).ok())
                .unwrap_or(4);
            cmd_read(&args[2], len, args.get(4).map(|s| s.as_str())).await;
        }
        "poke" => {
            if args.len() < 4 {
                eprintln!("Usage: ferros-bridge poke <hex-addr> <hex-u32>  (addr bit 63 / prefix 'priv:' = write through the EL3 monitor)");
                std::process::exit(1);
            }
            cmd_poke(&args[2], &args[3]).await;
        }
        "store" => cmd_store(&args[2..]).await,
        "display" if args.get(2).map(String::as_str) == Some("dcs") => {
            // display dcs "<packet> / <packet> / ..." — each packet is hex bytes (the DCS command first), optionally followed by wNN (wait NN ms after it); a leading tNN picks the DSI data type (default: G#05 / G#15 / G#39 by length). Example: 'f0 5a 5a / 51 05 49 / f0 a5 a5' sets brightness 1353.
            let script = args[3..].join(" ");
            cmd_display_dcs(&script).await;
        }
        "display" if args.get(2).map(String::as_str) == Some("power") => {
            // display power [samples = 1] [interval in 1/16 s = 8] [battery] — battery voltage and current from the MAX77759 gauge over hsi2c_11 (PMIC.md), for display power profiling against a `display fill` scene. `battery`: the charger's buck is off for the run so the reading is the system's draw, not the charger's leftovers.
            let n: u8 = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(1);
            let gap: u8 = args.get(4).and_then(|a| a.parse().ok()).unwrap_or(8);
            // `battery2`: also deselects the CHGIN input for the window (the charger's buck off alone left USB feeding the system on 2026-10-17).
            let battery = if args.iter().skip(3).any(|a| a == "battery2") { 2 } else { args.iter().skip(3).any(|a| a == "battery") as u8 };
            cmd_display_raw(&[11, n, gap, battery]).await;
        }
        "display" if args.get(2).map(String::as_str) == Some("i2c") => {
            // display i2c <target hex> <reg hex> [val hex] — an 8-bit register read, or write + read-back, on hsi2c_11 (gauge G#36, charger G#69, PMIC G#66).
            let h = |i: usize| args.get(i).and_then(|a| u8::from_str_radix(a.trim_start_matches("0x"), 16).ok());
            match (h(3), h(4), h(5)) {
                (Some(t), Some(r), Some(v)) => cmd_display_raw(&[14, t, r, v]).await,
                (Some(t), Some(r), None) => cmd_display_raw(&[13, t, r]).await,
                _ => {
                    eprintln!("Usage: ferros-bridge display i2c <target> <reg> [val]  (hex)");
                    std::process::exit(1);
                }
            }
        }
        "display" if args.get(2).map(String::as_str) == Some("dither") => {
            // display dither <hex> — the DQE display dither word (1 = on, 0 = off, 5 = on + temporal frame control, 3 = shift mode); applied at the next present.
            let v = args.get(3).and_then(|a| u8::from_str_radix(a.trim_start_matches("0x"), 16).ok()).unwrap_or(1);
            cmd_display_raw(&[16, v]).await;
        }
        "display" if args.get(2).map(String::as_str) == Some("fb") => {
            // display fb own|abl — paint on ferros's own framebuffer (the display-fb DMA region) or ABL's splash surface.
            let own = args.get(3).map(|a| a == "own").unwrap_or(true) as u8;
            cmd_display_raw(&[15, own]).await;
        }
        "display" if args.get(2).map(String::as_str) == Some("fill") => {
            // display fill <rrggbb> — a solid colour over the whole panel.
            let rgb = args.get(3).and_then(|a| u32::from_str_radix(a.trim_start_matches('#'), 16).ok()).unwrap_or(0);
            cmd_display_raw(&[12, (rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8]).await;
        }
        "display" if args.get(2).map(String::as_str) == Some("patch") => {
            // display patch <diameter px> <rrggbb> [surround rrggbb] — a disc centred on the panel over a surround (default black); diameter 0 = full field. For instrument readings (tools/display-profile.sh).
            let diam = args.get(3).and_then(|a| a.parse::<u16>().ok()).unwrap_or(0);
            let hex = |i: usize| args.get(i).and_then(|a| u32::from_str_radix(a.trim_start_matches('#'), 16).ok()).unwrap_or(0);
            let (rgb, sur) = (hex(4), hex(5));
            cmd_display_raw(&[17, (diam >> 8) as u8, diam as u8, (rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, (sur >> 16) as u8, (sur >> 8) as u8, sur as u8]).await;
        }
        "display" if args.get(2).map(String::as_str) == Some("patchraw") => {
            // display patchraw <diameter px> <hex word> [hex surround word] — the 10-bit layer's disc with raw 32-bit words.
            let diam = args.get(3).and_then(|a| a.parse::<u16>().ok()).unwrap_or(0);
            let hex = |i: usize| args.get(i).and_then(|a| u32::from_str_radix(a.trim_start_matches("G#"), 16).ok()).unwrap_or(0);
            let mut m = vec![20u8, (diam >> 8) as u8, diam as u8];
            m.extend_from_slice(&hex(4).to_be_bytes());
            m.extend_from_slice(&hex(5).to_be_bytes());
            cmd_display_raw(&m).await;
        }
        "display" if args.get(2).map(String::as_str) == Some("bg") => {
            // display bg <r> <g> <b> | off — DECON's background colour (decimal 0-1023 each) over the whole raster with the base window off; `off` turns the window back on.
            let mut m = vec![19u8];
            if args.get(3).map(String::as_str) != Some("off") {
                for i in 3..6 {
                    let v = args.get(i).and_then(|a| a.parse::<u16>().ok()).unwrap_or(0).min(1023);
                    m.extend_from_slice(&v.to_be_bytes());
                }
            }
            cmd_display_raw(&m).await;
        }
        "display" if args.get(2).map(String::as_str) == Some("patch10") => {
            // display patch10 <diameter px> <r> <g> <b> [surround grey] — the disc in 10 bits a channel (decimal 0-1023), the layer switched to 2:10:10:10.
            let n = |i: usize, d: u16| args.get(i).and_then(|a| a.parse::<u16>().ok()).unwrap_or(d).min(1023);
            let diam = args.get(3).and_then(|a| a.parse::<u16>().ok()).unwrap_or(0);
            let (r, g, b, s) = (n(4, 0), n(5, 0), n(6, 0), n(7, 0));
            let mut m = vec![18u8, (diam >> 8) as u8, diam as u8];
            for v in [r, g, b, s, s, s] {
                m.extend_from_slice(&v.to_be_bytes());
            }
            cmd_display_raw(&m).await;
        }
        "display" if args.get(2).map(String::as_str) == Some("dsim") => cmd_display_raw(&[3]).await,
        "display" if args.get(2).map(String::as_str) == Some("terate") => cmd_display_raw(&[5]).await,
        "display" if args.get(2).map(String::as_str) == Some("tune") => cmd_display_raw(&[7]).await,
        "display" if args.get(2).map(String::as_str) == Some("modeset") => {
            // display modeset fhd|wqhd [1368|1346] [8|10|keep] — the link rate (ABL's 1368 Mbps, Android's 1346; omitted keeps the current one) and the blend depth: 10 bpc with the DQE dither is the default (two bits of gradation on the 8 bpc link for no measurable power, DISPLAY-POWER.md), `8` is ABL's path, `keep` leaves it.
            let fhd = args.get(3).map(|a| a == "fhd").unwrap_or(false) as u8;
            // `link10`: the 10 bpc DSC stream experiment (DISPLAY-DESIGN.md step 5), with `f2=XX` the panel's F2 byte to try; needs eyes on the panel.
            let mut rate = 0;
            let mut bpc = 2;
            let mut f2 = 0u8;
            for a in &args[4..] {
                match a.as_str() {
                    "1368" => rate = 1,
                    "1346" => rate = 2,
                    "8" => bpc = 1,
                    "10" => bpc = 2,
                    "link10" => bpc = 3,
                    "keep" => bpc = 0,
                    s if s.starts_with("f2=") => f2 = u8::from_str_radix(&s[3..], 16).unwrap_or(0),
                    _ => {}
                }
            }
            cmd_display_raw(&[10, fhd, rate, bpc, f2]).await;
        }
        "display" if args.get(2).map(String::as_str) == Some("idle") => {
            let floor: u8 = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(10);
            let changeable = args.get(4).map(|a| a == "changeable").unwrap_or(false) as u8;
            cmd_display_raw(&[9, floor, changeable]).await;
        }
        "display" if args.get(2).map(String::as_str) == Some("res") => {
            // display res fhd|wqhd [steps mask: 1 panel, 2 DECON+DSC+layer, 4 DSIM; default 7]
            let fhd = args.get(3).map(|a| a == "fhd").unwrap_or(false) as u8;
            let steps: u8 = args.get(4).and_then(|a| a.parse().ok()).unwrap_or(7);
            cmd_display_raw(&[8, fhd, steps]).await;
        }
        "display" if args.get(2).map(String::as_str) == Some("mode") => {
            let hz: u8 = args.get(3).and_then(|a| a.parse().ok()).unwrap_or(120);
            cmd_display_raw(&[6, hz]).await;
        }
        "display" if args.get(2).map(String::as_str) == Some("read") => {
            // display read <reg hex> [len = 1] — a DCS read from the panel.
            let reg = u8::from_str_radix(args.get(3).map(String::as_str).unwrap_or("0a"), 16).unwrap_or_else(|_| { eprintln!("bad register"); std::process::exit(1) });
            let len: u8 = args.get(4).and_then(|a| a.parse().ok()).unwrap_or(1);
            cmd_display_raw(&[4, reg, len]).await;
        }
        "display" => {
            // --hold: keep the message up (the device's 512 s idle exit is disabled until a REBOOT; both volume keys still power it off).
            let hold = args.get(2).map(|a| a == "--hold").unwrap_or(false);
            let idx = if hold { 3 } else { 2 };
            let text = args.get(idx).cloned().unwrap_or_else(|| "ferros".to_string()).replace("\\n", "\n");
            cmd_display(&text, hold).await;
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
        "install" => {
            if args.len() < 3 {
                eprintln!("Usage: ferros-bridge install <kernel.signed>");
                std::process::exit(1);
            }
            cmd_install(&args[2]).await;
        }
        "terminal" => cmd_terminal().await,
        "gate" => {
            let v = match args.get(2).map(|s| s.as_str()) {
                Some("ferros") => 1,
                Some("android") => 2,
                Some("hold") => 3,
                _ => {
                    eprintln!("Usage: ferros-bridge gate <ferros|android|hold>");
                    std::process::exit(1);
                }
            };
            cmd_gate(v).await
        }
        "dbg" => cmd_dbg().await,
        "dbg2" => cmd_dbg2().await,
        "run" => {
            if args.len() < 3 {
                eprintln!("Usage: ferros-bridge run <payload.bin> [hex-input]");
                std::process::exit(1);
            }
            cmd_run(&args[2], args.get(3).map(|s| s.as_str()), ferros_pt::command::caps::RUN, Vec::new()).await;
        }
        "capsule" => {
            // capsule <code.bin> <out.vsf>: write a signed capsule (e.g. for `store put init @out.vsf`, which the cold boot spawns as its first process).
            if args.len() < 4 {
                eprintln!("Usage: ferros-bridge capsule <code.bin> <out.vsf>");
                std::process::exit(1);
            }
            let code = std::fs::read(&args[2]).unwrap_or_else(|e| {
                eprintln!("Failed to read {}: {e}", args[2]);
                std::process::exit(1);
            });
            let masked = args.iter().any(|a| a == "--masked");
            let c = signed_capsule(&code, masked, text_len_arg(&args));
            std::fs::write(&args[3], &c).unwrap_or_else(|e| {
                eprintln!("Failed to write {}: {e}", args[3]);
                std::process::exit(1);
            });
            println!("signed capsule: {} bytes of code -> {} ({} bytes)", code.len(), args[3], c.len());
        }
        "seal" => {
            // seal <masked.bin> --text-len N: install a masked capsule — the device runs the gate and the sandbox checker once and adds its seal to the vault's `sealed` list; spawn then requires the seal (SANDBOX.md).
            if args.len() < 3 {
                eprintln!("Usage: ferros-bridge seal <masked.bin> --text-len N");
                std::process::exit(1);
            }
            cmd_run_with_tail(&args[2], None, ferros_pt::command::caps::SEAL, Vec::new(), Vec::new(), true, text_len_arg(&args)).await;
        }
        "spawn" => {
            // spawn <payload.bin> [hex-input] [--name N] [--serve E]... [--use E]...
            if args.len() < 3 {
                eprintln!("Usage: ferros-bridge spawn <payload.bin> [hex-input] [--name N] [--serve ENDPOINT]... [--use ENDPOINT]... [--hw BLOCK]...  (runs it as a process: EL0, own arc, a console capability plus endpoint and hardware grants)");
                std::process::exit(1);
            }
            let mut name = String::new();
            let mut grants: Vec<(u8, String)> = Vec::new();
            let mut files: Vec<String> = Vec::new();
            let mut hex = None;
            let masked = args.iter().any(|a| a == "--masked");
            let mut i = 3;
            while i < args.len() {
                match args[i].as_str() {
                    "--name" => { name = args.get(i + 1).cloned().unwrap_or_default(); i += 2; }
                    "--serve" => { grants.push((1, args.get(i + 1).cloned().unwrap_or_default())); i += 2; }
                    "--use" => { grants.push((2, args.get(i + 1).cloned().unwrap_or_default())); i += 2; }
                    "--hw" => { grants.push((3, args.get(i + 1).cloned().unwrap_or_default())); i += 2; }
                    "--irq" => { grants.push((8, args.get(i + 1).cloned().unwrap_or_default())); i += 2; }
                    // --store NAME: a scope of the device vault (syscall 13); --power NAME: a device's power switch (syscall 22).
                    "--store" => { grants.push((6, args.get(i + 1).cloned().unwrap_or_default())); i += 2; }
                    "--power" => { grants.push((7, args.get(i + 1).cloned().unwrap_or_default())); i += 2; }
                    // --ira: the right to a per-purpose key derived from the device's ira (syscall 23).
                    "--ira" => { grants.push((9, String::new())); i += 1; }
                    // --entropy: the right to draw randomness from the session's wairua (syscall 6).
                    "--entropy" => { grants.push((4, String::new())); i += 1; }
                    // --dma NAME: a DMA region from the kernel's registry (syscall 7 gives its base).
                    "--dma" => { grants.push((5, args.get(i + 1).cloned().unwrap_or_default())); i += 2; }
                    "--masked" => { i += 1; }
                    "--text-len" => { i += 2; }
                    // --input-file F: append F's bytes to the process input (repeatable; each framed as u32 LE length + bytes after the hex input).
                    "--input-file" => { files.push(args.get(i + 1).cloned().unwrap_or_default()); i += 2; }
                    other => { hex = Some(other.to_string()); i += 1; }
                }
            }
            let mut prefix = vec![name.len() as u8];
            prefix.extend_from_slice(name.as_bytes());
            prefix.push(grants.len() as u8);
            for (kind, g) in &grants {
                prefix.push(*kind);
                prefix.push(g.len() as u8);
                prefix.extend_from_slice(g.as_bytes());
            }
            let mut tail = Vec::new();
            for f in &files {
                let b = std::fs::read(f).unwrap_or_else(|e| {
                    eprintln!("Failed to read {f}: {e}");
                    std::process::exit(1);
                });
                tail.extend_from_slice(&(b.len() as u32).to_le_bytes());
                tail.extend_from_slice(&b);
            }
            cmd_run_with_tail(&args[2], hex.as_deref(), ferros_pt::command::caps::SPAWN, prefix, tail, masked, text_len_arg(&args)).await;
        }
        "ping" => {
            let count = args
                .get(2)
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(256);
            cmd_ping(count).await;
        }
        "beam" => {
            if args.len() < 3 {
                eprintln!("Usage: ferros-bridge beam <ring> [~N] [count]");
                eprintln!("  ring: kernel-ring, vault-root, ledger, state");
                eprintln!("  ~N:   offset from latest (default: latest)");
                eprintln!("  count: number of entries (default: 1)");
                eprintln!("  gen:N  absolute generation number");
                std::process::exit(1);
            }
            cmd_beam(&args[2..]).await;
        }
        _ => {
            eprintln!("Unknown command: {}", args[1]);
            usage();
            std::process::exit(1);
        }
    }
    // Exit now: returning from main would wait for the background clock query's blocking DNS/HTTP threads.
    std::process::exit(0);
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

    // Wait for COMPLETE. Kernel responds when all_received(). Timeout proportional to transfer size: ~1ms per packet + 2s base.
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

/// Receive a PT transfer from the device (blast mode). Device sends SPEC + DATA blast + FIN. Bridge sends SPEC ACK, then responds with COMPLETE or NAK after all data received.
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

    // Skip SPEC ACK — kernel starts blasting immediately via idle-poll pump. Sending SPEC ACK on OUT would block because the kernel's ep2 may be busy.
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

/// Wrap position-independent native code in a signed capsule (toka's capsule format: a VSF file, section `aarch64` field `code`, signed with Ed25519 over the whole file). The developer key is the 64-byte keypair at $FERROS_DEV_KEY (default: the ferros key in the keys dir); only its 32-byte seed is used, and it never leaves this process.
/// `--text-len N`: for a masked capsule, how many bytes of the image are instructions (the rest is read-only data).
fn text_len_arg(args: &[String]) -> Option<usize> {
    args.iter().position(|a| a == "--text-len").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok())
}

fn signed_capsule(code: &[u8], masked: bool, text_len: Option<usize>) -> Vec<u8> {
    use ed25519_dalek::SigningKey;
    use vsf::types::VsfType;
    let key_path = std::env::var("FERROS_DEV_KEY").unwrap_or_else(|_| "/mnt/Harbor/Code/keys/ferros.secret".to_string());
    let key = std::fs::read(&key_path).unwrap_or_else(|e| {
        eprintln!("No developer key at {key_path}: {e}");
        std::process::exit(1);
    });
    if key.len() < 32 {
        eprintln!("Developer key at {key_path} is too short");
        std::process::exit(1);
    }
    let seed: [u8; 32] = key[..32].try_into().unwrap();
    let pubkey = SigningKey::from_bytes(&seed).verifying_key().to_bytes();
    let mut section = vsf::file_format::VsfSection::new(if masked { "masked" } else { "aarch64" });
    section.add_field("code", VsfType::v(b'b', code.to_vec()));
    if let Some(t) = text_len {
        section.add_field("text", VsfType::u(t, false));
    }
    let unsigned = vsf::VsfBuilder::new()
        .add_section_direct(section)
        .signature_ed25519(pubkey, [0u8; 64])
        .build()
        .unwrap_or_else(|e| {
            eprintln!("Capsule build failed: {e}");
            std::process::exit(1);
        });
    vsf::verification::sign_file(unsigned, &seed).unwrap_or_else(|e| {
        eprintln!("Capsule signing failed: {e}");
        std::process::exit(1);
    })
}

/// `prefix` goes in front of the Exec params (spawn's name + grants header; empty for run).
async fn cmd_run(path: &str, hex_input: Option<&str>, exec_cap: &[u8], prefix: Vec<u8>) {
    cmd_run_with_tail(path, hex_input, exec_cap, prefix, Vec::new(), false, None).await
}

/// As `cmd_run`; `tail` goes after the hex input (spawn's --input-file frames).
async fn cmd_run_with_tail(path: &str, hex_input: Option<&str>, exec_cap: &[u8], prefix: Vec<u8>, tail: Vec<u8>, masked: bool, text_len: Option<usize>) {
    let blob = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("Failed to read {path}: {e}");
        std::process::exit(1);
    });
    let raw = blob.clone();
    // spawn: the kernel only spawns signed capsules (the gate); run stays raw (dev payloads at EL2).
    let blob = if (exec_cap == ferros_pt::command::caps::SPAWN || exec_cap == ferros_pt::command::caps::SEAL) && std::env::var("FERROS_RAW_SPAWN").is_err() {
        let mut c = signed_capsule(&blob, masked, text_len);
        // Test switch: flip one byte of the code after signing — the kernel's gate must refuse it.
        if std::env::var("FERROS_TAMPER_CAPSULE").is_ok() {
            let n = c.len();
            c[n - 1] ^= 0x01;
        }
        eprintln!("Signed capsule: {} bytes of code -> {} bytes", blob.len(), c.len());
        c
    } else {
        blob
    };
    let input: Vec<u8> = match hex_input {
        Some(h) => {
            let h = h.trim_start_matches("0x");
            (0..h.len() / 2)
                .map(|i| u8::from_str_radix(&h[i * 2..i * 2 + 2], 16))
                .collect::<Result<_, _>>()
                .unwrap_or_else(|e| {
                    eprintln!("Bad hex input: {e}");
                    std::process::exit(1);
                })
        }
        None => Vec::new(),
    };

    let link = match usb::UsbLink::open() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    // Stage the payload
    // A capsule past the kernel's 4 MiB staging slot streams straight into its arc in 1 MiB pieces (STD-TARGET.md): the kernel learns the arc it needs — code, the image's zeroed tail from its header, the input, the entry stack — and where the code lies inside the signed capsule.
    let streamed = exec_cap == ferros_pt::command::caps::SPAWN && (blob.len() > (1 << 20) || std::env::var("FERROS_STREAM").is_ok());
    if streamed {
        let zeroed = if raw.len() >= 56 && &raw[8..16] == b"ferrosRT" { u64::from_le_bytes(raw[16..24].try_into().unwrap()) as usize + u64::from_le_bytes(raw[48..56].try_into().unwrap()) as usize } else { 0 };
        let need = 4096 + raw.len() + zeroed + prefix.len() + input.len() + tail.len() + 65536;
        let code_offset = (0..=blob.len() - raw.len()).find(|&i| blob[i..i + raw.len()] == raw[..]).expect("the code inside its capsule");
        let t0 = std::time::Instant::now();
        eprintln!("Streaming {} bytes into a {} byte arc (code at offset {code_offset})...", blob.len(), need);
        for (k, piece) in blob.chunks(1 << 20).enumerate() {
            let off = k << 20;
            let mut params = Vec::with_capacity(40 + piece.len());
            for v in [blob.len(), need, code_offset, raw.len(), off] {
                params.extend_from_slice(&(v as u64).to_le_bytes());
            }
            params.extend_from_slice(piece);
            let cmd = build_cmd(ferros_pt::command::caps::SPAWN, ferros_pt::Op::Write, &params);
            match pt_send(&link, &cmd).await {
                Ok(c) if c.success => {}
                other => {
                    eprintln!("Piece at {off} failed: {other:?}");
                    std::process::exit(1);
                }
            }
            match tokio::time::timeout(std::time::Duration::from_secs(15), pt_recv(&link)).await {
                Ok(Ok(resp)) if resp.len() >= 4 && u32::from_le_bytes([resp[0], resp[1], resp[2], resp[3]]) as usize == off + piece.len() => {}
                other => {
                    eprintln!("Piece at {off} not taken: {other:?}");
                    std::process::exit(1);
                }
            }
            eprint!("\r  {} of {} bytes, {} ms", off + piece.len(), blob.len(), t0.elapsed().as_millis());
        }
        eprintln!();
    }
    let cmd = build_cmd(ferros_pt::command::caps::RUN, ferros_pt::Op::Write, &blob);
    if !streamed {
    eprintln!("Staging payload ({} bytes)...", blob.len());
    match pt_send(&link, &cmd).await {
        Ok(c) if c.success => {}
        Ok(_) => {
            eprintln!("Device rejected payload write");
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("Stage failed: {e}");
            std::process::exit(1);
        }
    }
    match tokio::time::timeout(std::time::Duration::from_secs(15), pt_recv(&link)).await {
        Ok(Ok(resp)) if resp.len() >= 4 => {
            let staged = u32::from_le_bytes([resp[0], resp[1], resp[2], resp[3]]);
            if staged == 0 {
                eprintln!("Device rejected payload (size 0 ack)");
                std::process::exit(1);
            }
            eprintln!("Staged {staged} bytes, executing...");
        }
        other => {
            eprintln!("No staged-size ack: {other:?}");
            std::process::exit(1);
        }
    }
    }

    // Execute — Exec params become the payload's input
    let input = {
        let mut v = prefix;
        v.extend_from_slice(&input);
        v.extend_from_slice(&tail);
        v
    };
    let cmd = build_cmd(exec_cap, ferros_pt::Op::Exec, &input);
    if let Err(e) = pt_send(&link, &cmd).await {
        eprintln!("Exec failed: {e}");
        std::process::exit(1);
    }

    // Response: [ret: 8 LE][out bytes]
    match tokio::time::timeout(std::time::Duration::from_secs(30), pt_recv(&link)).await {
        Ok(Ok(resp)) => {
            if resp.starts_with(b"ERR") {
                eprintln!("Device refused: {}", String::from_utf8_lossy(&resp));
                std::process::exit(1);
            } else if exec_cap == ferros_pt::command::caps::SEAL && resp.len() == 33 && resp[0] == 0 {
                let hex: String = resp[1..].iter().map(|b| format!("{b:02x}")).collect();
                println!("sealed: G#{hex}");
            } else if resp.len() >= 8 {
                let ret = u64::from_le_bytes(resp[..8].try_into().unwrap());
                let out = &resp[8..];
                eprintln!("Payload returned A#{ret} ({} output bytes):", out.len());
                match std::str::from_utf8(out) {
                    Ok(s) if s.chars().all(|c| !c.is_control() || c == '\n' || c == '\t') => print!("{s}"),
                    _ => {
                        for chunk in out.chunks(16) {
                            for b in chunk { print!("{b:02x} "); }
                            println!();
                        }
                    }
                }
            } else if resp.starts_with(b"ERR") {
                eprintln!("Device error: {}", String::from_utf8_lossy(&resp));
                std::process::exit(1);
            } else {
                eprintln!("Short response: {} bytes", resp.len());
                std::process::exit(1);
            }
        }
        other => {
            eprintln!("No payload response: {other:?}");
            std::process::exit(1);
        }
    }
}

async fn cmd_gate(value: u16) {
    let link = match usb::UsbLink::open() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    match link.gate(value).await {
        Ok(()) => println!("gate answered: {}", ["", "ferros", "android", "hold"][value as usize]),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

async fn cmd_dbg() {
    let link = match usb::UsbLink::open() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    match link.read_dbg().await {
        Ok(d) => {
            println!("PT dispatch debug counters (EP0 vendor readout):");
            println!("  [0]  ep2 (bulk OUT) events        A#{}", d[0]);
            println!("  [1]  bulk_out_read returned None  A#{}", d[1]);
            println!("  [2]  last packet: len=A#{} first_byte=G#{:02X}", d[2] >> 8, d[2] & 0xFF);
            println!("  [3]  PHY init attempts            A#{}", d[3]);
            println!("  [4]  SPEC decoded OK              A#{}", d[4]);
            println!("  [5]  SPEC ACK send                {}", tri_state(d[5]));
            println!("  [6]  DATA packets seen            A#{}", d[6]);
            println!("  [7]  decode_data OK               A#{}", d[7]);
            println!("  [8]  chunks accepted (hash OK)    A#{}", d[8]);
            println!("  [9]  all_received hit             A#{}", d[9]);
            println!("  [10] finish() bytes               A#{}", d[10]);
            println!("  [11] COMPLETE send                {}", tri_state(d[11]));
            println!("  [12] driver ep2 XferComplete      A#{}", d[12]);
            println!("  [13] driver ep3 XferComplete      A#{}", d[13]);
            println!("  [14] flags: bulk_in_idle={} out_armed={} out_ready={} configured={}",
                d[14] & 1, (d[14] >> 1) & 1, (d[14] >> 2) & 1, (d[14] >> 3) & 1);
            println!("  [15] DEPCMD status failures       A#{}", d[15]);
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

async fn cmd_dbg2() {
    let link = match usb::UsbLink::open() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    match link.read_dbg2().await {
        Ok(d) => {
            println!("Raw DWC3 event ring (oldest first; A#{} events total):", d[12]);
            for i in 0..12 {
                let evt = d[i];
                if evt == 0 && d[12] < 12 { continue; }
                print!("  [{i:2}] G#{evt:08X}");
                if evt & 1 == 0 {
                    let ep = (evt >> 1) & 0x1F;
                    let ty = (evt >> 6) & 0xF;
                    let status = (evt >> 12) & 0xF;
                    let name = match ty {
                        1 => "XferComplete",
                        2 => "XferInProgress",
                        3 => "XferNotReady",
                        6 => "StreamEvt",
                        7 => "EPCmdCmplt",
                        _ => "?",
                    };
                    println!("  ep=A#{ep} {name} status=G#{status:X}");
                } else {
                    let ty = (evt >> 8) & 0xF;
                    println!("  device event type=A#{ty}");
                }
            }
            println!("ep2 TRB at last XferComplete: size=G#{:08X} (remaining=A#{}) ctrl=G#{:08X} (HWO={})",
                d[13], d[13] & 0x00FF_FFFF, d[14], d[14] & 1);
            println!("last DEPCMD failure register: G#{:08X}", d[15]);
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

fn tri_state(v: u32) -> &'static str {
    match v {
        0 => "never attempted",
        1 => "OK",
        2 => "DROPPED (bulk IN busy)",
        3 => "SPEC REJECTED (buffer sizing)",
        _ => "?",
    }
}

async fn cmd_diag(params: &[u8]) {
    let link = match usb::UsbLink::open() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    // Send DIAG Read command
    let cmd = build_cmd(ferros_pt::command::caps::DIAG, ferros_pt::Op::Read, params);
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

/// Vault access on the device (shim USB service, ira-keyed vault on the ferros partition): put / get / ls / rm by logical key.
async fn cmd_store(args: &[String]) {
    let usage = || {
        eprintln!("Usage: ferros-bridge store put <key> <text|@file> | get <key> [out-file] | ls | rm <key>");
        std::process::exit(1);
    };
    let (op, params) = match args.first().map(|s| s.as_str()) {
        Some("put") if args.len() >= 3 => {
            let value = match args[2].strip_prefix('@') {
                Some(path) => std::fs::read(path).unwrap_or_else(|e| {
                    eprintln!("Failed to read {path}: {e}");
                    std::process::exit(1);
                }),
                None => args[2].as_bytes().to_vec(),
            };
            let key = args[1].as_bytes();
            let mut p = Vec::with_capacity(2 + key.len() + value.len());
            p.extend_from_slice(&(key.len() as u16).to_be_bytes());
            p.extend_from_slice(key);
            p.extend_from_slice(&value);
            (ferros_pt::Op::Write, p)
        }
        Some("get") if args.len() >= 2 => (ferros_pt::Op::Read, args[1].as_bytes().to_vec()),
        Some("ls") => (ferros_pt::Op::Read, Vec::new()),
        Some("rm") if args.len() >= 2 => (ferros_pt::Op::Exec, args[1].as_bytes().to_vec()),
        _ => usage(),
    };
    let link = usb::UsbLink::open().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let cmd = build_cmd(ferros_pt::command::caps::STORE, op, &params);
    if let Err(e) = pt_send(&link, &cmd).await {
        eprintln!("Command send failed: {e}");
        std::process::exit(1);
    }
    let r = pt_recv(&link).await.unwrap_or_else(|e| {
        eprintln!("Response receive failed: {e}");
        std::process::exit(1);
    });
    match r.first().copied() {
        Some(0) => {}
        Some(1) => {
            eprintln!("not found");
            std::process::exit(1);
        }
        Some(s) => {
            eprintln!("{}", match s { 2 => "vault error", 3 => "bad request", 4 => "no vault open on the device", _ => "unknown status" });
            // Newer kernels follow status 2 with the vault's own error text.
            if r.len() > 1 {
                eprintln!("  {}", String::from_utf8_lossy(&r[1..]));
            }
            std::process::exit(1);
        }
        None => {
            eprintln!("empty response");
            std::process::exit(1);
        }
    }
    let body = &r[1..];
    match args[0].as_str() {
        "put" => println!("stored, object {}", body.iter().map(|b| format!("{b:02x}")).collect::<String>()),
        "get" => match args.get(2) {
            Some(out) => {
                std::fs::write(out, body).unwrap_or_else(|e| {
                    eprintln!("Failed to write {out}: {e}");
                    std::process::exit(1);
                });
                eprintln!("Wrote {} bytes to {out}", body.len());
            }
            None => {
                use std::io::Write;
                let _ = std::io::stdout().write_all(body);
            }
        },
        "ls" => print!("{}", String::from_utf8_lossy(body)),
        _ => println!("removed"),
    }
}

/// Render text on the device's panel (shim, after a --live handoff) and print the device's register report.
/// Encode a DCS script into the kernel's `[type][len][bytes…][wait_ms]` packets and send it as DISPLAY op G#02.
async fn cmd_display_dcs(script: &str) {
    let mut params = vec![2u8];
    let mut n = 0;
    for pkt in script.split('/') {
        let mut bytes = Vec::new();
        let mut wait = 0u8;
        let mut kind: Option<u8> = None;
        for tok in pkt.split_whitespace() {
            if let Some(w) = tok.strip_prefix('w') {
                wait = w.parse().unwrap_or_else(|_| { eprintln!("bad wait: {tok}"); std::process::exit(1) });
            } else if let Some(t) = tok.strip_prefix('t') {
                kind = Some(u8::from_str_radix(t, 16).unwrap_or_else(|_| { eprintln!("bad type: {tok}"); std::process::exit(1) }));
            } else {
                bytes.push(u8::from_str_radix(tok, 16).unwrap_or_else(|_| { eprintln!("bad byte: {tok}"); std::process::exit(1) }));
            }
        }
        if bytes.is_empty() {
            continue;
        }
        let kind = kind.unwrap_or(match bytes.len() { 1 => 0x05, 2 => 0x15, _ => 0x39 });
        params.push(kind);
        params.push(bytes.len() as u8);
        params.extend_from_slice(&bytes);
        params.push(wait);
        n += 1;
    }
    println!("{n} packets");
    cmd_display_raw(&params).await;
}

/// Send raw DISPLAY params and print the reply.
async fn cmd_display_raw(params: &[u8]) {
    let link = usb::UsbLink::open().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let cmd = build_cmd(ferros_pt::command::caps::DISPLAY, ferros_pt::Op::Write, params);
    if let Err(e) = pt_send(&link, &cmd).await {
        eprintln!("Command send failed: {e}");
        std::process::exit(1);
    }
    match pt_recv(&link).await {
        Ok(r) => print!("{}", String::from_utf8_lossy(&r)),
        Err(e) => eprintln!("Response receive failed: {e}"),
    }
}

async fn cmd_display(text: &str, hold: bool) {
    let link = usb::UsbLink::open().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let mut params = Vec::with_capacity(text.len() + 1);
    if hold {
        params.push(1u8);
    }
    params.extend_from_slice(text.as_bytes());
    let cmd = build_cmd(ferros_pt::command::caps::DISPLAY, ferros_pt::Op::Write, &params);
    if let Err(e) = pt_send(&link, &cmd).await {
        eprintln!("Command send failed: {e}");
        std::process::exit(1);
    }
    match pt_recv(&link).await {
        Ok(r) => print!("{}", String::from_utf8_lossy(&r)),
        Err(e) => eprintln!("Response receive failed: {e}"),
    }
}

/// Parse a hex address; a `priv:` prefix sets bit 63, which the shim routes through the EL3 monitor (SMC_CMD_PRIV_REG).
fn parse_addr(s: &str) -> u64 {
    let (privileged, hex) = match s.strip_prefix("priv:") {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let a = u64::from_str_radix(hex.trim_start_matches("G#").trim_start_matches("0x"), 16).unwrap_or_else(|_| {
        eprintln!("Invalid hex address: {s}");
        std::process::exit(1);
    });
    if privileged { a | 1 << 63 } else { a }
}

async fn cmd_poke(addr_str: &str, val_str: &str) {
    let link = usb::UsbLink::open().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let addr = parse_addr(addr_str);
    let val = u32::from_str_radix(val_str.trim_start_matches("G#").trim_start_matches("0x"), 16).unwrap_or_else(|_| {
        eprintln!("Invalid hex value: {val_str}");
        std::process::exit(1);
    });
    let mut params = [0u8; 12];
    params[0..8].copy_from_slice(&addr.to_be_bytes());
    params[8..12].copy_from_slice(&val.to_be_bytes());
    let cmd = build_cmd(ferros_pt::command::caps::MEM, ferros_pt::Op::Write, &params);
    if let Err(e) = pt_send(&link, &cmd).await {
        eprintln!("Command send failed: {e}");
        std::process::exit(1);
    }
    match pt_recv(&link).await {
        Ok(r) if r.len() >= 12 => {
            let ret = u64::from_le_bytes(r[0..8].try_into().unwrap());
            let back = u32::from_le_bytes(r[8..12].try_into().unwrap());
            println!("wrote G#{val:08X} -> G#{:X}{}: smc_ret=G#{ret:X} readback=G#{back:08X}", addr & !(1 << 63), if addr >> 63 == 1 { " (via EL3)" } else { "" });
        }
        Ok(r) => eprintln!("Short response: {} bytes", r.len()),
        Err(e) => eprintln!("Response receive failed: {e}"),
    }
}

async fn cmd_read(addr_str: &str, len: usize, out_file: Option<&str>) {
    let link = match usb::UsbLink::open() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    let addr = parse_addr(addr_str);

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
        Ok(data) if out_file.is_some() => {
            let path = out_file.unwrap();
            std::fs::write(path, &data).unwrap_or_else(|e| {
                eprintln!("Failed to write {path}: {e}");
                std::process::exit(1);
            });
            eprintln!("Wrote {} bytes to {path}", data.len());
        }
        Ok(data) => {
            // Hex dump
            for (i, chunk) in data.chunks(16).enumerate() {
                print!("{:08X}  ", (addr & !(1 << 63)) as usize + i * 16);
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
    match tokio::time::timeout(
        std::time::Duration::from_secs(15),
        pt_recv(&link),
    ).await {
        Ok(Ok(resp)) => {
            if resp.len() >= 4 {
                let total = u32::from_le_bytes([resp[0], resp[1], resp[2], resp[3]]);
                eprintln!("Device staged {} bytes", total);
            }
        }
        Ok(Err(e)) => {
            eprintln!("Response receive failed: {e}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("Timed out after 15s — kernel didn't send staged-size. Power cycle required.");
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

async fn cmd_install(path: &str) {
    // Read .signed file and parse trailer: [binary][size:4 LE][hash:32][sig:64]["FERROSIG"]
    let data = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("Failed to read {path}: {e}");
        std::process::exit(1);
    });

    const TRAILER: usize = 108; // 4 + 32 + 64 + 8
    if data.len() < TRAILER + 0x1000 {
        eprintln!("File too small for signed kernel: {} bytes", data.len());
        std::process::exit(1);
    }

    // Verify FERROSIG magic at end
    let magic_start = data.len() - 8;
    if &data[magic_start..] != b"FERROSIG" {
        eprintln!("Missing FERROSIG trailer magic");
        std::process::exit(1);
    }

    // Extract fields
    let meta_start = data.len() - TRAILER;
    let kernel_size = u32::from_le_bytes([
        data[meta_start], data[meta_start + 1],
        data[meta_start + 2], data[meta_start + 3],
    ]);
    let kernel_data = &data[..kernel_size as usize];

    let mut kernel_hash = [0u8; 32];
    kernel_hash.copy_from_slice(&data[meta_start + 4..meta_start + 36]);

    let mut kernel_sig = [0u8; 64];
    kernel_sig.copy_from_slice(&data[meta_start + 36..meta_start + 100]);

    // Verify hash locally
    let computed = blake3::hash(kernel_data);
    if computed.as_bytes() != &kernel_hash {
        eprintln!("Local BLAKE3 mismatch — .signed file corrupt");
        std::process::exit(1);
    }

    eprintln!("Kernel: {} bytes, BLAKE3 {}...verified",
        kernel_size, &computed.to_hex()[..16]);

    let link = match usb::UsbLink::open() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    // Step 1: Stage kernel binary via RELOAD Write (reuses proven staging path)
    let cmd = build_cmd(
        ferros_pt::command::caps::RELOAD,
        ferros_pt::Op::Write,
        kernel_data,
    );
    eprintln!("Staging kernel ({} bytes) via RELOAD Write...", kernel_data.len());
    match pt_send(&link, &cmd).await {
        Ok(complete) => {
            if !complete.success {
                eprintln!("Device rejected kernel write");
                std::process::exit(1);
            }
            eprintln!("Write accepted, receiving staged size...");
        }
        Err(e) => {
            eprintln!("Transfer failed: {e}");
            std::process::exit(1);
        }
    }
    match tokio::time::timeout(
        std::time::Duration::from_secs(15),
        pt_recv(&link),
    ).await {
        Ok(Ok(resp)) => {
            if resp.len() >= 4 {
                let staged = u32::from_le_bytes([resp[0], resp[1], resp[2], resp[3]]);
                eprintln!("Device staged {} bytes", staged);
            }
        }
        Ok(Err(e)) => {
            eprintln!("Response receive failed: {e}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("Timed out waiting for staged-size");
            std::process::exit(1);
        }
    }

    // Step 2: Send INSTALL Exec with params [size:4 LE][hash:32][sig:64] = 100 bytes
    let mut params = [0u8; 100];
    params[0..4].copy_from_slice(&kernel_size.to_le_bytes());
    params[4..36].copy_from_slice(&kernel_hash);
    params[36..100].copy_from_slice(&kernel_sig);

    let cmd = build_cmd(
        ferros_pt::command::caps::INSTALL,
        ferros_pt::Op::Exec,
        &params,
    );
    eprintln!("Executing install (write to UFS + stem entry)...");
    match pt_send(&link, &cmd).await {
        Ok(complete) => {
            if !complete.success {
                eprintln!("Device rejected install exec");
                std::process::exit(1);
            }
            eprintln!("Exec accepted, receiving result...");
        }
        Err(e) => {
            eprintln!("Exec failed: {e}");
            std::process::exit(1);
        }
    }

    // Read response — "OK" or "ERR:..."
    match tokio::time::timeout(
        std::time::Duration::from_secs(30),
        pt_recv(&link),
    ).await {
        Ok(Ok(resp)) => {
            let msg = String::from_utf8_lossy(&resp);
            if msg.starts_with("OK") {
                println!("Install complete — kernel persisted to UFS + signed stem entry");
            } else {
                eprintln!("Install failed: {msg}");
                std::process::exit(1);
            }
        }
        Ok(Err(e)) => {
            eprintln!("Response receive failed: {e}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("Timed out waiting for install result (30s)");
            std::process::exit(1);
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

async fn cmd_beam(args: &[String]) {
    use ferros_pt::command::{ring_id, beam_mode};

    let link = match usb::UsbLink::open() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    // Parse ring name
    let rid = match args[0].as_str() {
        "kernel-ring" | "kernel" => ring_id::KERNEL,
        "vault-root" | "vault" => ring_id::VAULT_ROOT,
        "ledger" => ring_id::LEDGER,
        "state" => ring_id::STATE,
        other => {
            eprintln!("Unknown ring: {other}");
            std::process::exit(1);
        }
    };

    // Parse optional offset and count
    let mut mode = beam_mode::LATEST;
    let mut offset: u32 = 0;
    let mut count: u32 = 1;

    for arg in &args[1..] {
        if arg.starts_with('~') {
            // ~N = latest minus N
            mode = beam_mode::LATEST;
            offset = arg[1..].parse().unwrap_or(0);
        } else if arg.starts_with("gen:") {
            // gen:N = absolute generation
            mode = beam_mode::ABSOLUTE;
            offset = arg[4..].parse().unwrap_or(1);
        } else if arg == "all" {
            mode = beam_mode::ALL;
        } else if let Ok(n) = arg.parse::<u32>() {
            // bare number = count
            count = n;
        }
    }

    // Build params: [ring_id:1][mode:1][offset:4 BE][count:4 BE]
    let mut params = [0u8; 10];
    params[0] = rid;
    params[1] = mode;
    params[2..6].copy_from_slice(&offset.to_be_bytes());
    params[6..10].copy_from_slice(&count.to_be_bytes());

    let cmd = build_cmd(ferros_pt::command::caps::BEAM, ferros_pt::Op::Read, &params);
    eprintln!("Beam: ring={} mode={} offset={} count={}", args[0], mode, offset, count);

    match pt_send(&link, &cmd).await {
        Ok(complete) => {
            if !complete.success {
                eprintln!("Device rejected beam command");
                std::process::exit(1);
            }
            eprintln!("Command accepted, receiving data...");
        }
        Err(e) => {
            eprintln!("Command send failed: {e}");
            std::process::exit(1);
        }
    }

    // Receive ring data
    match pt_recv(&link).await {
        Ok(data) => {
            eprintln!("Received {} bytes ({} blocks)", data.len(), data.len() / 4096);
            let _ = io::stdout().write_all(&data);
            let _ = io::stdout().flush();
        }
        Err(e) => {
            eprintln!("Response receive failed: {e}");
            std::process::exit(1);
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

/// Raw USB ping-pong — no PT framing. Sends a small probe on bulk OUT and reads bulk IN back, `count` times, measuring round-trip latency. Isolates the USB link itself from the PT layer when bringing up a board.
async fn cmd_ping(count: u32) {
    let link = match usb::UsbLink::open() {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    eprintln!("Raw USB ping-pong: {count} round-trips (no PT)");

    let probe = b"PING";
    let mut ok = 0u32;
    let mut total_us: u128 = 0;
    let mut min_us = u128::MAX;
    let mut max_us = 0u128;

    for i in 0..count {
        let start = std::time::Instant::now();
        if let Err(e) = link.send(probe).await {
            eprintln!("  ping {i}: OUT failed: {e}");
            continue;
        }
        match link.recv_timeout(std::time::Duration::from_millis(1024)).await {
            Ok(_) => {
                let us = start.elapsed().as_micros();
                total_us += us;
                min_us = min_us.min(us);
                max_us = max_us.max(us);
                ok += 1;
            }
            Err(e) => eprintln!("  ping {i}: IN failed: {e}"),
        }
    }

    if ok == 0 {
        eprintln!("0/{count} round-trips — link dead");
        std::process::exit(1);
    }

    let avg_us = total_us / ok as u128;
    println!(
        "{ok}/{count} OK  rtt avg=A#{avg_us}us min=A#{min_us}us max=A#{max_us}us",
    );
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
