// FERROS-BRIDGE SOURCE MAP — keep updated when commands change
//
// main.rs ── CLI entry point, PT send/recv, command dispatch Commands: status — USB VID/PID probe diag — boot log via DIAG cap read <addr> [len] — MMIO via MEM cap log — stream bulk IN echo/send — PT test transfers reboot [fastboot] — REBOOT cap reload <kernel> — hot-reload via RELOAD cap install <signed> — UFS + stem via INSTALL cap beam <ring> — pull ring blocks via BEAM cap terminal — interactive PT session. See BRIDGE.md for full reference. (ping is in usage() but unwired.)
//
//   pt_send(link, data), pt_recv(link) → Vec<u8>: ferros-link's (tools/ferros-link, shared with ferros-send), the DATA blast each way
//
// ledger.rs ── the flash log: `diag log` rendered (`--raw` its VSF documents), log-render <file>, log-legacy <out> [--erase] (the old text log converted once)
//
// deaths.rs ── deaths [n]: the flash log's recent deaths (`diag "log deaths"`), each frame named from its process's ELF (addr2line)
//
// usb.rs ── UsbLink is ferros-link's (VID G#1209, PID G#4665, FERROS_BUS); here only the development vendor requests on EP0: read_dbg, read_dbg2, gate

extern crate alloc;
/// `deaths`: the flash log's recent deaths, their frames named from the processes' ELFs (KERNEL.md "A process that dies").
mod deaths;
mod input;
/// The flash log: `diag log` rendered, `log-render`, `log-legacy` (LEDGER.md "Reading it").
// The hold on the device, as the person (docs/STORE.md): `items find | card | get | put | rm | kinds`.
mod items;
mod ledger;
/// A WiFi credential document, the schema pcie1 reads (shared by path: it uses only vsf and alloc).
#[path = "../../../ferros_detail/src/network.rs"]
mod network;
/// A sponsor placed by hand, the document payloads/fgtw reads from `fgtw.sponsor` (shared by path: it uses only vsf and alloc).
#[path = "../../../ferros_kept/src/sponsor.rs"]
mod sponsor;
/// `part`: partitions block for block off the device, read-only (the per-unit backups).
mod part;
mod usb;

use ferros_pt::packet::Complete;
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
    eprintln!("  diag         Retrieve boot diagnostics from device");
    eprintln!("  diag log [--raw] | diag \"log <n>\"  The flash log's newest entries (256 or n) as lines; --raw writes its VSF documents");
    eprintln!("  log-render <file.vsf>  A saved `diag log --raw` answer or a converted old log, as lines");
    eprintln!("  deaths [n]   The recent deaths in the flash log: message, file:line, thread, frames named from the ELFs (--elf name=path, --dir, --blocks)");
    eprintln!("  log-legacy <out.vsf> [--erase]  The old 2026-10-20 text log as ledger_legacy documents, once; --erase zeroes it on the device after the file checks");
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
    eprintln!("  part list | dump <name> <out> | gpt <lu> <primary> <backup>   Partitions block for block, read-only (development kernels; per-unit backups into keys/<device>/)");
    eprintln!("  store put <key> <text|@file> | get <key> [out] | ls | rm <key>   Vault by logical key (shim)");
    eprintln!("  icon seed <decoded.vsf>...   Decoded icons (std/icondec on the host) into kists. by their hp");
    eprintln!("  items find | card <h> | get <h> <out> | put <kind> <file> [--clip] | rm <h> | usage | kinds   The hold, as the person (`items` alone for the flags)");
    eprintln!("  sponsor <address> <port> <out.vsf>  The sponsor document for `store put fgtw.sponsor @out.vsf` (IPv4 or IPv6)");
    eprintln!("  display [--hold] [text]  Render text on the panel (shim after a --live handoff; '\\n' = newline; --hold keeps it up until a REBOOT; both volume keys = killswitch power-off)");
    eprintln!("  input <tap|hold|swipe|drag|pinch|rotate|key|idle|shot|script> …  Fingers and keys in shown frames, as a VSF script (development builds; `input` alone for the list)");
    eprintln!("  kist stow <capsule.vsf>  Development fallback: a kist straight into the attested person's list, no check by the device's stower and no Stow on its screen (unattended loops). Stow a kist with `ferros send <capsule.vsf>` (tools/ferros-send), the checked path");
    eprintln!("  kist answer <name> <right> on|off|unset | kist list  The attested person's answers (development)");
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        usage();
        std::process::exit(1);
    }

    // Each transfer's steps on stderr, as the bridge always said them (ferros-link is quiet for a person's tool).
    ferros_link::trace(true);

    // Calibrate in the background: nothing on the device path reads the session clock yet, and a slow or failing nunc query (seconds to a minute when sources don't answer) must never delay a USB command.
    let _clock = tokio::spawn(SessionClock::calibrate());

    match args[1].as_str() {
        "status" => cmd_status().await,
        // `diag ufs` runs the cold boot's UFS link-startup probe (long report).
        // `diag log [n]` is rendered as lines (`--raw`: its VSF documents as they came).
        "diag" => cmd_diag(args.get(2).map(|s| s.as_bytes()).unwrap_or(&[]), args.iter().any(|a| a == "--raw")).await,
        // log-render <file.vsf>: a saved `diag log --raw` answer or a converted old log, as lines.
        "log-render" if args.len() >= 3 => ledger::cmd_render(&args[2]),
        // log-legacy <out.vsf> [--erase]: the 2026-10-20 text log, converted once (ledger.rs).
        "log-legacy" => ledger::cmd_legacy(&args[2..]).await,
        // deaths [n]: the recent deaths in the flash log, message, place and named frames (deaths.rs).
        "deaths" => deaths::cmd_deaths(&args[2..]).await,
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
        "icon" => cmd_icon(&args[2..]).await,
        "items" => items::cmd_items(&args[2..]).await,
        // part list | dump <name> <out> | gpt <lu> <primary> <backup>: Android's partitions read block for block thru the development `diag lun`, for the per-unit backups (part.rs).
        "part" => part::cmd(&args[2..]).await,
        // input tap|hold|swipe|drag|pinch|rotate|key|idle|shot|script …: fingers and keys in shown frames (input.rs, docs/INPUT-INJECT.md).
        "input" => input::cmd(&args[2..]).await,
        // kist stow <capsule.vsf>: the development fallback (KISTS.md "Stowing"): a signed kist capsule kept in the vault and added to the attested person's list with no check by the device's stower and no Stow on its screen, for unattended loops. `ferros send` (tools/ferros-send) is the checked path every script uses.
        "kist" if args.get(2).map(String::as_str) == Some("stow") && args.len() >= 4 => cmd_kist_stow(&args[3]).await,
        // kist install: the old name (owner, 2026-11-11: "the proper term for installing a kist is stow"), still taken for the scripts and hands that know it, said so each time.
        "kist" if args.get(2).map(String::as_str) == Some("install") && args.len() >= 4 => {
            eprintln!("`kist install` is the old name: it is `kist stow` now");
            cmd_kist_stow(&args[3]).await
        }
        // kist answer <name> <right> on|off: one answer of the attested person's (development); kist list: their list.
        "kist" if matches!(args.get(2).map(String::as_str), Some("answer") | Some("list")) => cmd_kist_answer(&args[3..]).await,
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
        "display" if args.get(2).map(String::as_str) == Some("ruler") => cmd_display_raw(&[21]).await,
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
        "wifi-cred" => {
            // wifi-cred <in.txt> <out.vsf>: the network credential document pcie1 reads (ferros_detail network.rs: section `network`, name and passphrase), from a host file holding the name on its first line and the passphrase on its second (keys/wifi.txt). For a staged run's input file or a `store put wifi.cred`; never printed.
            if args.len() < 4 {
                eprintln!("Usage: ferros-bridge wifi-cred <in.txt> <out.vsf>");
                std::process::exit(1);
            }
            let text = std::fs::read_to_string(&args[2]).unwrap_or_else(|e| {
                eprintln!("{}: {e}", args[2]);
                std::process::exit(1);
            });
            let mut lines = text.lines().map(|l| l.trim_end());
            let (name, pass) = (lines.next().unwrap_or(""), lines.next().unwrap_or(""));
            let Some(doc) = network::build(name.as_bytes(), pass.as_bytes(), vsf::eagle_time_oscillations()) else {
                eprintln!("{}: not a usable credential (a name of 1 to 32 bytes, a passphrase of 8 to 63)", args[2]);
                std::process::exit(1);
            };
            std::fs::write(&args[3], &doc).unwrap_or_else(|e| {
                eprintln!("{}: {e}", args[3]);
                std::process::exit(1);
            });
            println!("credential document: {} bytes", doc.len());
        }
        "sponsor" => {
            // sponsor <address> <port> <out.vsf>: the sponsor document the identity service reads from `fgtw.sponsor` (ferros_kept sponsor.rs: section `sponsor`, the address as `ni` or `nj`, the port as `np`), for a `store put fgtw.sponsor @out.vsf`.
            let (Some(Ok(ip)), Some(Ok(port)), Some(out)) = (args.get(2).map(|a| a.parse::<std::net::IpAddr>()), args.get(3).map(|p| p.parse::<u16>()), args.get(4)) else {
                eprintln!("Usage: ferros-bridge sponsor <address> <port> <out.vsf>");
                std::process::exit(1);
            };
            let addr = match ip {
                std::net::IpAddr::V4(v4) => {
                    let mut b = [0u8; 16];
                    b[..4].copy_from_slice(&v4.octets());
                    (4, b, port)
                }
                std::net::IpAddr::V6(v6) => (6, v6.octets(), port),
            };
            let doc = sponsor::build(&addr, vsf::eagle_time_oscillations()).expect("a family of 4 or 6 always builds");
            // Read back as fgtw reads it before it is written out.
            assert_eq!(sponsor::parse(&doc), Some(addr), "the sponsor document does not read back");
            std::fs::write(out, &doc).unwrap_or_else(|e| {
                eprintln!("{out}: {e}");
                std::process::exit(1);
            });
            println!("sponsor document: {} bytes", doc.len());
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
                    // --unlock: the identity unlock right (syscall 24, AUTH.md).
                    "--unlock" => { grants.push((11, String::new())); i += 1; }
                    // --pt: the kernel's PT commands, for the USB driver process (syscall 25, ptlink.rs).
                    "--pt" => { grants.push((12, String::new())); i += 1; }
                    // --sensors: publishing the sensors' block, for the sensors process (syscall 26, sensorlink.rs).
                    "--sensors" => { grants.push((13, String::new())); i += 1; }
                    // --blocks NAME: a window of blocks on the ferros partition (syscall 29; process.rs WINDOWS: vault).
                    "--blocks" => { grants.push((14, args.get(i + 1).cloned().unwrap_or_default())); i += 2; }
                    // --vault: the vault process's own call (syscall 30, the kernel's old entries to move) and with it the right to serve `vault`.
                    "--vault" => { grants.push((15, String::new())); i += 1; }
                    // --entropy: the right to draw randomness from the session's wairua (syscall 6).
                    "--entropy" => { grants.push((4, String::new())); i += 1; }
                    // --dma NAME: a DMA region from the kernel's registry (syscall 7 gives its base).
                    "--dma" => { grants.push((5, args.get(i + 1).cloned().unwrap_or_default())); i += 2; }
                    // --part NAME: read-only files on one of the device's partitions (process.rs PARTS: modem_a, efs, persist, modem_userdata, vendor_a).
                    "--part" => { grants.push((10, args.get(i + 1).cloned().unwrap_or_default())); i += 2; }
                    "--masked" => { i += 1; }
                    "--text-len" => { i += 2; }
                    "--fixed" => { i += 2; }
                    "--bti" => { i += 1; }
                    "--exact" => { i += 1; }
                    // --domain: its own vault domain (syscall 13 on its own data, KISTS.md); --kists: the right to launch kists (init's).
                    "--domain" => { grants.push((16, String::new())); i += 1; }
                    "--kists" => { grants.push((17, String::new())); i += 1; }
                    // --items: the hold, any kind (a development spawn's; a kist's kinds come from its capsule's `items` section): docs/STORE.md, ferros_items `kist::Hold`.
                    "--items" => { grants.push((18, String::new())); i += 1; }
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

async fn cmd_status() {
    match usb::UsbLink::open().await {
        Ok(_link) => println!("Device connected and ready"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

// ---------------------------------------------------------------------------
// PT send and receive: ferros-link's (tools/ferros-link), shared with ferros-send
// ---------------------------------------------------------------------------

/// Send data to the device via PT and wait for COMPLETE.
async fn pt_send(link: &usb::UsbLink, data: &[u8]) -> Result<Complete, String> {
    ferros_link::pt::send(link, data).await
}

/// Receive a PT transfer from the device (blast mode): its answer to the command sent last.
async fn pt_recv(link: &usb::UsbLink) -> Result<Vec<u8>, String> {
    ferros_link::pt::recv(link).await
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

/// Wrap position-independent native code in a signed capsule (toka's capsule format: a VSF file, section `aarch64` field `code`, signed with Ed25519 over the whole file). The developer key is the VSF key at $FERROS_DEV_KEY (default: the ferros key, keys/ferros.key.vsf; section `developer_key` { label a, seed kge }, read thru ferros_kist::secret, the one reader); only its seed is used, and it never leaves this process. A raw key file (the form before 2026-11-10) is refused, naming the conversion (kistcat `devkey convert`). A kist (`--kist NAME --option HEX [--want RIGHT REASON]... [--developer NAME] [--version 0.1.0] [--icon <icon .vsf>]`; the icon in the one icon format, tools/app-icons `icon`) is built by ferros_kist::capsule, as tools/kistcat `kist` builds one: its developer's whole public bundle in it (`kb`) and an egg per scheme, derived under the key's label, or fgtw's release bundle when the key is Photon's release key.
/// `--text-len N`: for a masked capsule, how many bytes of the image are instructions (the rest is read-only data).
fn text_len_arg(args: &[String]) -> Option<usize> {
    args.iter().position(|a| a == "--text-len").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok())
}

/// `--fixed K`: a masked capsule in fixed mode at arc size 2^K (SANDBOX.md, "Modes"), written into its masked section as `fixed`; read once from the command line.
fn fixed_arg() -> Option<u32> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == "--fixed").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok())
}

/// A kist's declaration from the command line (KISTS.md): `--kist NAME` and each `--want RIGHT REASON` in slot order; None without `--kist`. A kist declares only the rights it is asked for; the implicit set (ferros_kist::IMPLICIT) it is handed without declaring it, and a `--want` for one of those is a placement, the right put in that slot and never asked (KISTS.md "The implicit set"). A right outside the vocabulary, or a reason or name that is not printable ASCII (the list keeps them as `a`), stops here.
fn kist_arg() -> Option<ferros_kist::Decl> {
    let args: Vec<String> = std::env::args().collect();
    let name = args.iter().position(|a| a == "--kist").and_then(|i| args.get(i + 1)).cloned()?;
    let ascii = |s: &str| !s.is_empty() && s.bytes().all(|b| (0x20..0x7F).contains(&b));
    let mut wants = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--want" {
            let (Some(r), Some(why)) = (args.get(i + 1), args.get(i + 2)) else {
                eprintln!("--want takes a right and its reason");
                std::process::exit(1);
            };
            let Some(c) = ferros_kist::code(r) else {
                eprintln!("{r} is not a right ({})", ferros_kist::RIGHTS.join(", "));
                std::process::exit(1);
            };
            if !ascii(why) || wants.iter().any(|w: &(u8, String)| w.0 == c) {
                eprintln!("--want {r}: a reason of printable ASCII, each right once");
                std::process::exit(1);
            }
            wants.push((c, why.clone()));
            i += 3;
        } else {
            i += 1;
        }
    }
    if !ascii(&name) {
        eprintln!("--kist: a name of printable ASCII");
        std::process::exit(1);
    }
    Some(ferros_kist::Decl { name, wants })
}

/// A kist's kinds of the hold from the command line (docs/STORE.md part 1, its capsule's `items` section): each `--makes KIND REASON` and `--opens KIND REASON`, the kind by its first phrasing (`ferros-bridge items kinds`), the reason printable ASCII. They count only where the kist also wants `items` and the person granted it.
fn items_arg() -> ferros_kist::ItemsDecl {
    let args: Vec<String> = std::env::args().collect();
    let mut d = ferros_kist::ItemsDecl::default();
    for (i, a) in args.iter().enumerate() {
        let list = match a.as_str() {
            "--makes" => &mut d.makes,
            "--opens" => &mut d.opens,
            _ => continue,
        };
        let (Some(k), Some(why)) = (args.get(i + 1), args.get(i + 2)) else {
            eprintln!("{a} takes a kind and its reason");
            std::process::exit(1);
        };
        let Some(kind) = ferros_items::kinds::by_phrasing(k) else {
            eprintln!("{a} {k}: not one of the project's kinds (`ferros-bridge items kinds`)");
            std::process::exit(1);
        };
        if why.is_empty() || !why.bytes().all(|b| (0x20..0x7F).contains(&b)) {
            eprintln!("{a} {k}: a reason of printable ASCII");
            std::process::exit(1);
        }
        list.push((kind, why.clone()));
    }
    d
}

/// `--option HEX`: the kist's option, 32 bytes (64 hex digits), written as the capsule's provenance hash: the identity that stays the same across versions (ORACLE.md §5, "the developer mints it once, keeps it stable"), so the person's answers and the kist's domain carry over an update. Without it the hash is computed from the content, and every build is a new kist.
fn option_arg() -> Option<[u8; 32]> {
    let args: Vec<String> = std::env::args().collect();
    let h = args.iter().position(|a| a == "--option").and_then(|i| args.get(i + 1))?;
    let b: Vec<u8> = (0..h.len() / 2).filter_map(|i| u8::from_str_radix(h.get(i * 2..i * 2 + 2)?, 16).ok()).collect();
    match <[u8; 32]>::try_from(b) {
        Ok(o) if h.len() == 64 => Some(o),
        _ => {
            eprintln!("--option takes 64 hex digits");
            std::process::exit(1);
        }
    }
}

/// The kist signer's bundle from the key's label and seed: Photon's release key (known by its Ed25519 public key) signs with fgtw's release bundle; any other key with its post-quantum halves derived under its label (fgtw `SigningBundle::developer`, as kistcat `kist` does).
fn kist_signer(label: &str, seed: &[u8; 32]) -> fgtw::pq::SigningBundle {
    let b = fgtw::pq::SigningBundle::release(seed);
    if b.public().ed25519() == fgtw::release::RELEASE_ED25519 {
        return b;
    }
    fgtw::pq::SigningBundle::developer(seed, label)
}

/// The developer key's file when $FERROS_DEV_KEY is unset: the ferros key as VSF (kistcat `devkey convert` writes it from the raw keys/ferros.secret).
const DEV_KEY: &str = "/mnt/Harbor/Code/keys/ferros.key.vsf";

/// The developer key at $FERROS_DEV_KEY (default [`DEV_KEY`]) read thru ferros_kist::secret, the one reader: its label and seed. The file's bytes are wiped once read; a raw key file is refused, naming the conversion.
fn dev_key() -> (String, [u8; 32]) {
    let key_path = std::env::var("FERROS_DEV_KEY").unwrap_or_else(|_| DEV_KEY.to_string());
    let mut doc = std::fs::read(&key_path).unwrap_or_else(|e| {
        eprintln!("No developer key at {key_path}: {e} (the ferros key as VSF is written once from the raw keys/ferros.secret: cargo run --release --manifest-path tools/kistcat/Cargo.toml -- devkey convert --raw /mnt/Harbor/Code/keys/ferros.secret --label FERROS_PROJECT --secret {DEV_KEY})");
        std::process::exit(1);
    });
    let read = ferros_kist::secret::read(&doc);
    doc.fill(0);
    read.unwrap_or_else(|e| {
        eprintln!("Developer key at {key_path}: {e}");
        std::process::exit(1);
    })
}

fn signed_capsule(code: &[u8], masked: bool, text_len: Option<usize>) -> Vec<u8> {
    let (label, mut seed) = dev_key();
    // The code's section, as kistcat writes it (ferros_kist::capsule): `--bti` (the image carries BTI landing pads; the kernel maps its code on guarded pages, SANDBOX.md "BTI") and `--exact` (exact mode, the subtract and unsigned compare; needs `--bti`).
    let c = ferros_kist::capsule::Code { image: code, masked, text: text_len, fixed: fixed_arg(), bti: std::env::args().any(|a| a == "--bti"), exact: std::env::args().any(|a| a == "--exact") };
    // A kist: its declaration before its code (the kernel and the stower read it from the capsule's first bytes), its option in it (`kist`'s `option`), its developer's whole public bundle in it (`kb`), and an egg per scheme (owner, 2026-11-10: "no single eggs"; trust on first stow), built by the one builder kistcat's `kist` uses.
    if let Some(d) = kist_arg() {
        let Some(option) = option_arg() else {
            eprintln!("a kist needs its --option (its identity across versions, minted once)");
            std::process::exit(1);
        };
        let signer = kist_signer(&label, &seed);
        seed.fill(0);
        let developer = std::env::args().collect::<Vec<_>>().windows(2).find(|w| w[0] == "--developer").map(|w| w[1].clone());
        // `--version 0.1.0` (any number of parts): the kist's version, in its signed declaration.
        let version = std::env::args().collect::<Vec<_>>().windows(2).find(|w| w[0] == "--version").map(|w| {
            ferros_kist::Version::parse(&w[1]).unwrap_or_else(|| {
                eprintln!("--version {}: parts of decimal digits, separated by dots (0.1.0)", w[1]);
                std::process::exit(1);
            })
        });
        // `--icon <file.vsf>`: the kist's icon document (tools/app-icons `icon`, the one icon format), carried in its signed declaration (ferros_kist::Icon).
        let icon = std::env::args().collect::<Vec<_>>().windows(2).find(|w| w[0] == "--icon").map(|w| {
            let doc = std::fs::read(&w[1]).unwrap_or_else(|e| {
                eprintln!("--icon {}: {e}", w[1]);
                std::process::exit(1);
            });
            ferros_kist::icon_document(&doc).unwrap_or_else(|e| {
                eprintln!("--icon {}: {e} (make it with tools/app-icons icon)", w[1]);
                std::process::exit(1);
            })
        });
        let unsigned = ferros_kist::capsule::unsigned_with_items(&d, &items_arg(), option, version.as_ref(), developer.as_deref(), icon.as_ref(), &signer.public(), &c, vsf::eagle_time_oscillations()).unwrap_or_else(|e| {
            eprintln!("Capsule build failed: {e}");
            std::process::exit(1);
        });
        return fgtw::release::sign_release(unsigned, &signer).unwrap_or_else(|e| {
            eprintln!("Capsule signing failed: {e}");
            std::process::exit(1);
        });
    }
    seed.fill(0);
    plain_capsule(code, masked, text_len)
}

/// Every capsule but a kist's (the shell's processes, a hand spawn's, under the developer key in $FERROS_DEV_KEY or ferros's) with its one Ed25519 signature: the same bytes for the same code and key, so its identity can be made again (`kist_of`).
fn plain_capsule(code: &[u8], masked: bool, text_len: Option<usize>) -> Vec<u8> {
    use ed25519_dalek::SigningKey;
    let (_label, mut seed) = dev_key();
    let c = ferros_kist::capsule::Code { image: code, masked, text: text_len, fixed: fixed_arg(), bti: std::env::args().any(|a| a == "--bti"), exact: std::env::args().any(|a| a == "--exact") };
    let pubkey = SigningKey::from_bytes(&seed).verifying_key().to_bytes();
    let unsigned = vsf::VsfBuilder::new()
        .add_section_direct(ferros_kist::capsule::code_section(&c))
        .signature_ed25519(pubkey, [0u8; 64])
        .build()
        .unwrap_or_else(|e| {
            eprintln!("Capsule build failed: {e}");
            std::process::exit(1);
        });
    // Test switches for the gate (it must refuse both): FERROS_ZERO_SIG sends the capsule with its signature slot still all zeros; FERROS_RANDOM_SIG signs it, then overwrites the signature with other bytes.
    if std::env::var("FERROS_ZERO_SIG").is_ok() {
        seed.fill(0);
        return unsigned;
    }
    let plain = unsigned.clone();
    let signed = vsf::verification::sign_file(unsigned, &seed);
    seed.fill(0);
    let mut signed = signed.unwrap_or_else(|e| {
        eprintln!("Capsule signing failed: {e}");
        std::process::exit(1);
    });
    if std::env::var("FERROS_RANDOM_SIG").is_ok() && signed.len() == plain.len() {
        let fake = blake3::hash(b"not the signature");
        let mut k = 0;
        for i in 0..signed.len() {
            if signed[i] != plain[i] || (k > 0 && k < 64) {
                if k < 64 {
                    signed[i] = fake.as_bytes()[k % 32] ^ k as u8;
                    k += 1;
                }
            }
        }
    }
    signed
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

    let link = match usb::UsbLink::open().await {
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
        // What the kernel will hash: the capsule with its 64 signature bytes zeroed — printed so a refusal on the device can be set against it.
        if let Ok((h, header_end)) = vsf::file_format::VsfHeader::decode(&blob) {
            if let Some(vsf::types::VsfType::ge(sig)) = &h.signature {
                let hdr = header_end.min(blob.len());
                match (0..=hdr.saturating_sub(64)).find(|&i| blob[i..i + 64] == sig[..]) {
                    Some(at) => {
                        let mut z = blob.clone();
                        z[at..at + 64].fill(0);
                        eprintln!("Signed: {} bytes, signature at {at} (header {hdr} bytes), hash {}", blob.len(), blake3::hash(&z).to_hex());
                    }
                    None => eprintln!("Signed: the signature is not in the first {hdr} header bytes"),
                }
            } else {
                eprintln!("Signed: the header carries no Ed25519 signature (ge)");
            }
        } else {
            eprintln!("Signed: the header does not decode");
        }
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
                // Out now: a spawn goes on streaming the device log until it is stopped, and a piped stdout held back is lost with it.
                let _ = std::io::Write::flush(&mut std::io::stdout());
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
    let link = match usb::UsbLink::open().await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    match usb::gate(&link, value).await {
        Ok(()) => println!("gate answered: {}", ["", "ferros", "android", "hold"][value as usize]),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

async fn cmd_dbg() {
    let link = match usb::UsbLink::open().await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    match usb::read_dbg(&link).await {
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
    let link = match usb::UsbLink::open().await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    match usb::read_dbg2(&link).await {
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

async fn cmd_diag(params: &[u8], raw: bool) {
    let link = match usb::UsbLink::open().await {
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
            if ledger::is_entries(params) {
                ledger::show(&data, raw);
                return;
            }
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
            // One PT command carries at most 6 MiB (the kernel's ptlink BIG region). A name the vault process holds (a scope's: map., wifi., gpu., …) takes a value up to that; the kernel's own vault refuses one over 3 MiB with status 5 (its STORE_VALUE_MAX: the seal and the read-back each need about twice the value in one piece of its heap).
            if ferros_pt::command::CMD_MIN_SIZE + 2 + key.len() + value.len() > 6 << 20 {
                eprintln!("{} bytes is over the 6 MiB one command carries", value.len());
                std::process::exit(1);
            }
            let mut p = Vec::with_capacity(2 + key.len() + value.len());
            p.extend_from_slice(&(key.len() as u16).to_be_bytes());
            p.extend_from_slice(key);
            p.extend_from_slice(&value);
            (ferros_pt::Op::Write, p)
        }
        // store extent <key> <bytes> <out> [--kist <capsule.vsf>]: a single-copy extent (docs/RAW-VIDEO.md, a RAW clip) pulled whole, 768 blocks (3 MiB) a command; store extent-info <key> [--kist <capsule.vsf>]: its record. With --kist the name is in that kist's own domain (Lumis's clips: any Lumis capsule names Lumis).
        Some("extent") if args.len() >= 4 => return extent_pull(&args[1], &args[2], &args[3], kist_of(args)).await,
        Some("extent-info") if args.len() >= 2 => {
            let mut p = match kist_of(args) {
                Some(id) => [&[0xFCu8][..], &id[..]].concat(),
                None => vec![0xFEu8],
            };
            p.extend_from_slice(args[1].as_bytes());
            (ferros_pt::Op::Read, p)
        }
        // store extent-drop-orphan <first block>: a sealed extent at that first block dropped whatever domain owns it (development builds; for an extent whose owning build no longer exists, so no --kist can name its domain).
        Some("extent-drop-orphan") if args.len() >= 2 => {
            let first: u64 = args[1].parse().unwrap_or_else(|_| {
                eprintln!("Usage: ferros-bridge store extent-drop-orphan <first block>");
                std::process::exit(1)
            });
            let mut p = vec![0xF8u8];
            p.extend_from_slice(&first.to_le_bytes());
            // A scope's name, never looked up: the kernel's G#F8 takes the name field as G#FA does.
            p.push(0);
            p.extend_from_slice(b"test.orphan");
            (ferros_pt::Op::Read, p)
        }
        // store extent-list <name> [--kist <capsule|bin>]: every extent of the clips area as records, those of the name's domain marked (the name a scope's full name, `test.x` for std/rawrec's, or with --kist any name: it is not looked up); store extent-drop <first block> <name> [--kist ...]: the domain's extent at that first block dropped (a clip whose name is lost).
        Some(c @ ("extent-list" | "extent-drop")) if args.len() >= 2 => {
            let bad = || -> ! { eprintln!("Usage: ferros-bridge store extent-list <name> [--kist <capsule|bin>] | extent-drop <first block> <name> [--kist <capsule|bin>]"); std::process::exit(1) };
            let (first, name): (u64, &String) = if c == "extent-list" { (0, &args[1]) } else if args.len() >= 3 { (args[1].parse().unwrap_or_else(|_| bad()), &args[2]) } else { bad() };
            let mut p = vec![if c == "extent-list" { 0xFBu8 } else { 0xFA }];
            p.extend_from_slice(&first.to_le_bytes());
            match kist_of(args) {
                Some(id) => {
                    p.push(1);
                    p.extend_from_slice(&id);
                }
                None => p.push(0),
            }
            p.extend_from_slice(name.as_bytes());
            (ferros_pt::Op::Read, p)
        }
        Some("get") if args.len() >= 2 => (ferros_pt::Op::Read, args[1].as_bytes().to_vec()),
        Some("ls") => (ferros_pt::Op::Read, Vec::new()),
        Some("rm") if args.len() >= 2 => (ferros_pt::Op::Exec, args[1].as_bytes().to_vec()),
        _ => usage(),
    };
    let link = usb::UsbLink::open().await.unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let cmd = build_cmd(ferros_pt::command::caps::STORE, op, &params);
    // A put that commits a large value may outlive one answer: its end read held (ferros_pt::command::held).
    let r = ferros_link::pt::call_held(&link, &cmd).await.unwrap_or_else(|e| {
        eprintln!("Command failed: {e}");
        std::process::exit(1);
    });
    match r.first().copied() {
        Some(0) => {}
        Some(1) => {
            eprintln!("not found");
            std::process::exit(1);
        }
        Some(s) => {
            eprintln!("{}", match s { 2 => "vault error", 3 => "bad request", 4 => "no vault open on the device", 5 => "refused: over 3 MiB (a put to the kernel's own vault, or a get, which the kernel answers whole)", _ => "unknown status" });
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
        // The vault process answers a put with no object hash (its values are a head and pieces, not one object).
        "put" if body.is_empty() => println!("stored (the vault process)"),
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
        "extent-info" if body.len() >= 65 => {
            let u = |k: usize| u64::from_le_bytes(body[k..k + 8].try_into().unwrap());
            println!("extent: A#{} blocks from block A#{} of the clips area, A#{} bytes, sealed {}, BLAKE3 {}, at A#{}", u(0), u(57), u(8), body[16] != 0, body[17..49].iter().map(|b| format!("{b:02X}")).collect::<String>(), u(49) as i64);
        }
        "extent-list" if body.len() >= 32 => {
            let mine = &body[..32];
            let rows: Vec<&[u8]> = body[32..].chunks_exact(65).collect();
            println!("extent-list: A#{} extents; this domain's owner {}", rows.len(), mine[..8].iter().map(|b| format!("{b:02X}")).collect::<String>());
            for r in rows {
                let u = |k: usize| u64::from_le_bytes(r[k..k + 8].try_into().unwrap());
                println!("  first A#{} blocks A#{} bytes A#{} sealed {} at A#{} owner {}{}", u(0), u(8), u(16), r[24] != 0, u(25) as i64, r[33..41].iter().map(|b| format!("{b:02X}")).collect::<String>(), if &r[33..65] == mine { " (this domain's)" } else { "" });
            }
        }
        "extent-drop" | "extent-drop-orphan" => println!("dropped"),
        _ => println!("removed"),
    }
}

/// `--kist <capsule.vsf | payload.bin>`: the identity (developer key, option) whose own domain holds the name, as the kernel gives a process its domain at spawn: a kist capsule's own (a stowed Lumis's, any version); a payload image the way `spawn` signs one by hand (the developer key in $FERROS_DEV_KEY or ferros's, the option the capsule's provenance hash: the exact image a hand-spawned Lumis ran, its domain changing with every build); any other capsule its key and provenance hash.
fn kist_of(args: &[String]) -> Option<[u8; 64]> {
    let path = args.iter().position(|a| a == "--kist").and_then(|i| args.get(i + 1))?;
    let file = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("{path}: {e}");
        std::process::exit(1);
    });
    let capsule = if file.starts_with("RÅ<".as_bytes()) { file } else { plain_capsule(&file, false, None) };
    let hp = vsf::file_format::VsfHeader::decode(&capsule).ok().and_then(|(h, _)| match h.provenance_hash {
        vsf::types::VsfType::hp(b) if b.len() == 32 => <[u8; 32]>::try_from(&b[..]).ok(),
        _ => None,
    });
    let (developer, option) = ferros_kist::identity(&capsule).or_else(|e| match (vsf::file_format::VsfHeader::decode(&capsule).ok().and_then(|(h, _)| h.signer_pubkey), hp) {
        (Some(vsf::types::VsfType::ke(k)), Some(hp)) if k.len() == 32 => Ok((k[..].try_into().unwrap(), hp)),
        _ => Err(e),
    }).unwrap_or_else(|e| {
        eprintln!("{path}: no identity: {e}");
        std::process::exit(1);
    });
    let mut id = [0u8; 64];
    id[..32].copy_from_slice(&developer);
    id[32..].copy_from_slice(&option);
    Some(id)
}

/// `store extent <key> <bytes|all> <out> [--kist <capsule.vsf>]`: the extent's first `bytes` (`all`: its sealed length, from its record; a count rounded up to whole blocks, then cut to it) into `out`, 768 blocks a command (the kernel's reply is built whole: STORE_VALUE_MAX), with the rate.
async fn extent_pull(key: &str, bytes: &str, out: &str, kist: Option<[u8; 64]>) {
    let link = usb::UsbLink::open().await.unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let named = |head: u8| -> Vec<u8> {
        match kist {
            Some(id) => [&[head - 2][..], &id[..]].concat(),
            None => vec![head],
        }
    };
    let want: u64 = match bytes {
        "all" => {
            let mut p = named(0xFE);
            p.extend_from_slice(key.as_bytes());
            let r = ferros_link::pt::call_held(&link, &build_cmd(ferros_pt::command::caps::STORE, ferros_pt::Op::Read, &p)).await.unwrap_or_else(|e| {
                eprintln!("{key}: {e}");
                std::process::exit(1);
            });
            if r.first() != Some(&0) || r.len() < 66 || r[17] == 0 {
                eprintln!("{key}: no sealed extent (status {:?})", r.first());
                std::process::exit(1);
            }
            u64::from_le_bytes(r[9..17].try_into().unwrap())
        }
        b => b.parse().unwrap_or_else(|_| {
            eprintln!("bytes: a decimal count, or all");
            std::process::exit(1);
        }),
    };
    const PULL: u64 = 768;
    let blocks = want.div_ceil(4096);
    let mut file = std::fs::File::create(out).unwrap_or_else(|e| {
        eprintln!("{out}: {e}");
        std::process::exit(1);
    });
    let t = std::time::Instant::now();
    let mut at = 0u64;
    while at < blocks {
        let n = (blocks - at).min(PULL);
        let mut p = named(0xFF);
        p.extend_from_slice(&at.to_le_bytes());
        p.extend_from_slice(&(n as u32).to_le_bytes());
        p.extend_from_slice(key.as_bytes());
        let cmd = build_cmd(ferros_pt::command::caps::STORE, ferros_pt::Op::Read, &p);
        let r = ferros_link::pt::call_held(&link, &cmd).await.unwrap_or_else(|e| {
            eprintln!("block A#{at}: {e}");
            std::process::exit(1);
        });
        if r.first() != Some(&0) || r.len() != 1 + (n as usize) * 4096 {
            eprintln!("block A#{at}: status {:?}, A#{} bytes", r.first(), r.len());
            std::process::exit(1);
        }
        let keep = ((want - at * 4096).min(n * 4096)) as usize;
        std::io::Write::write_all(&mut file, &r[1..1 + keep]).unwrap();
        at += n;
        if at % (PULL << 6) == 0 {
            eprintln!("A#{} MiB, A#{} MB/s", (at * 4096) >> 20, (at * 4096) as f64 / 1e6 / t.elapsed().as_secs_f64());
        }
    }
    println!("pulled A#{want} bytes into {out} in A#{} ms (A#{:.1} MB/s)", t.elapsed().as_millis(), want as f64 / 1e6 / t.elapsed().as_secs_f64());
}

/// One STORE command and its reply: the status byte's meaning on failure, the rest on success.
/// `icon seed <decoded.vsf>…` (coordinator, 2026-11-20: the bench and a first deploy need no fresh stow): each decoded icon (std/icondec on the host: `icondec <capsule or icon> <out.vsf>`) stored in the device's `kists.` under the `hp` its document names, by address, as the stower stores what the decoder hands it (the development store capability, which only the host reaches; a production build answers none). The document is checked first: it must be a decoded icon that says it is that `hp`. The stower's start then puts each stowed kist's icon in the index, finding these decoded already.
async fn cmd_icon(args: &[String]) {
    let files = match args.first().map(String::as_str) {
        Some("seed") if args.len() >= 2 => &args[1..],
        _ => {
            eprintln!("Usage: ferros-bridge icon seed <decoded.vsf>...   (make each with std/icondec on the host)");
            std::process::exit(1);
        }
    };
    let link = usb::UsbLink::open().await.unwrap_or_else(|e| {
        eprintln!("no device: {e}");
        std::process::exit(1);
    });
    for path in files {
        let doc = std::fs::read(path).unwrap_or_else(|e| {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        });
        // The `hp` it names, read by its type marker, then the whole document held to it (ferros_kept::icon `read_decoded`).
        let hp = vsf::VsfHeader::decode(&doc).ok().and_then(|(h, _)| h.fields.iter().find(|f| f.name == "decoded").map(|f| f.offset_bytes)).and_then(|at| {
            let mut p = at;
            vsf::VsfSection::parse(&doc, &mut p).ok()?.get_field("icon").and_then(|f| f.values.first()).and_then(|v| ferros_thing::Hp::from_vsf(v).ok())
        });
        let Some(hp) = hp.filter(|h| ferros_kept::icon::read_decoded(&doc, h).is_some()) else {
            eprintln!("{path}: not a decoded icon (std/icondec makes them)");
            std::process::exit(1);
        };
        let mut key = b"kists.".to_vec();
        key.extend_from_slice(hp.as_bytes());
        let mut p = Vec::with_capacity(2 + key.len() + doc.len());
        // Bit 14: the key is a scope's prefix and a 32-byte address (service.rs `vault_job_start`).
        p.extend_from_slice(&((key.len() as u16) | 1 << 14).to_be_bytes());
        p.extend_from_slice(&key);
        p.extend_from_slice(&doc);
        match store_call(&link, ferros_pt::Op::Write, &p).await {
            Ok(_) => println!("{path}: icon {} stored in kists., {} bytes", hp.as_bytes().iter().map(|b| format!("{b:02x}")).collect::<String>(), doc.len()),
            Err(e) => {
                eprintln!("{path}: not stored: {e}");
                std::process::exit(1);
            }
        }
    }
}

async fn store_call(link: &usb::UsbLink, op: ferros_pt::Op, params: &[u8]) -> Result<Vec<u8>, String> {
    let cmd = build_cmd(ferros_pt::command::caps::STORE, op, params);
    // A put that commits a large value may outlive one answer: its end read held (ferros_pt::command::held).
    let r = ferros_link::pt::call_held(link, &cmd).await?;
    match r.first().copied() {
        Some(0) => Ok(r[1..].to_vec()),
        Some(1) => Err("not found".into()),
        Some(s) => Err(format!("status {s}: {}", String::from_utf8_lossy(&r[1..]))),
        None => Err("empty response".into()),
    }
}

/// `kist stow <capsule.vsf>`, the development fallback (KISTS.md "Stowing"; owner, 2026-11-11: "a proper checked path"): `ferros send` (tools/ferros-send) offers a kist to the device's stower, which checks it and asks the person to Stow it; this door skips both, thru the development store capability, for unattended loops with nobody at the screen (a production build answers neither `store` nor this). As it does: the capsule's signature checked here as the gate will check it, its bytes kept in the vault under their blake3 (`capsules.`, the device's, in parts of 4 MiB, each a command, so a capsule of any size goes in), then the person's list (`kists.list`, theirs: they must be attested) read, the kist added or updated (an update keeps every answer for a right it still wants; a new want is unanswered, asked at the next launch) and written back. The device asks each new want at the kist's first launch, never here.
async fn cmd_kist_stow(path: &str) {
    let capsule = std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("Failed to read {path}: {e}");
        std::process::exit(1);
    });
    if !matches!(vsf::verification::verify_file_signature(&capsule), Ok(true)) {
        eprintln!("{path}: not a signed capsule, or its signature does not verify");
        std::process::exit(1);
    }
    let (decl, (developer, option)) = match (ferros_kist::declaration(&capsule), ferros_kist::identity(&capsule)) {
        (Ok(d), Ok(i)) => (d, i),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("{path}: {e}");
            std::process::exit(1);
        }
    };
    let (signer, eggs, file_hash) = vsf::verification::header_eggs(&capsule).unwrap_or_else(|e| {
        eprintln!("{path}: {e}: not stowed");
        std::process::exit(1);
    });
    let addr = *blake3::hash(&capsule).as_bytes();
    let link = usb::UsbLink::open().await.unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let mut list = match store_call(&link, ferros_pt::Op::Read, b"kists.list").await {
        Ok(doc) => ferros_kist::Kists::parse(&doc).unwrap_or_else(|e| {
            eprintln!("the person's list does not parse ({e}); not overwriting it");
            std::process::exit(1);
        }),
        Err(e) if e == "not found" => ferros_kist::Kists::default(),
        Err(e) => {
            eprintln!("the person's list could not be read ({e}): is someone attested?");
            std::process::exit(1);
        }
    };
    // Every egg, here, before the capsule reaches the vault (owner, 2026-11-10: all three at stow, the Ed25519 egg again at every spawn, by the kernel's gate), against the bundle the capsule carries (`kb`, its Ed25519 key the developer's), and that bundle the one the person pinned for this developer at their first kist (trust on first stow): the network stower's rule, ferros_kist `capsule::admit`. A capsule from before `kb` stows only under Photon's release key, against the bundle fgtw pins, as before.
    let admitted = ferros_kist::capsule::admit(&capsule, &signer, &eggs, &file_hash, &list).unwrap_or_else(|r| {
        eprintln!("{path}: {}: not stowed", ferros_kist::capsule::refusal_words(&r));
        std::process::exit(1);
    });
    let mut key = b"capsules.".to_vec();
    key.extend_from_slice(&addr);
    let t0 = std::time::Instant::now();
    const PART: usize = 4 << 20;
    // `--stored`: the capsule is in this device's vault already (the same address: an earlier stow whose list entry was lost), so only the list entry is written and its parts are not sent again.
    // A big capsule's last part commits the whole value, which on a busy vault ran past the USB driver's 16 s (the 8a, 2026-11-13); since then the device answers such a part STILL at its lease and the end is read held (`store_call`), so a normal stow needs no `--stored`.
    let stored = std::env::args().any(|a| a == "--stored");
    for (k, part) in capsule.chunks(PART).enumerate().filter(|_| !stored) {
        let off = k * PART;
        let more = off + part.len() < capsule.len();
        let mut p = Vec::with_capacity(2 + key.len() + 9 + part.len());
        p.extend_from_slice(&((key.len() as u16) | 1 << 15 | 1 << 14).to_be_bytes());
        p.extend_from_slice(&key);
        p.extend_from_slice(&(off as u64).to_le_bytes());
        p.push(more as u8);
        p.extend_from_slice(part);
        if let Err(e) = store_call(&link, ferros_pt::Op::Write, &p).await {
            eprintln!("\nthe capsule's part at {off} was not stored: {e}");
            std::process::exit(1);
        }
        eprint!("\r  {} of {} bytes stored, {} ms", off + part.len(), capsule.len(), t0.elapsed().as_millis());
    }
    eprintln!();
    // The developer's bundle pinned with the kist when it is their first here (the list read above, before the capsule was stored).
    if let Some(b) = admitted.bundle {
        if list.trust(&developer, &b, admitted.name.as_deref(), vsf::eagle_time_oscillations()) == ferros_kist::Trust::First {
            println!("first kist by this developer here: their key bundle is pinned in the person's list");
        }
    }
    let i = list.stow(addr, developer, option, &decl);
    // When it was stowed: its fact's stamp in the person's space, so a stow after an unstow wins over the unstow (ferros_space). Its source stays as it was: a capsule from the bridge has no URL to fetch again.
    list.kists[i].stowed = vsf::eagle_time_oscillations();
    let doc = list.build(vsf::eagle_time_oscillations()).unwrap_or_else(|e| {
        eprintln!("the list does not build: {e}");
        std::process::exit(1);
    });
    let mut p = Vec::with_capacity(2 + 10 + doc.len());
    p.extend_from_slice(&(10u16).to_be_bytes());
    p.extend_from_slice(b"kists.list");
    p.extend_from_slice(&doc);
    if let Err(e) = store_call(&link, ferros_pt::Op::Write, &p).await {
        eprintln!("the person's list was not written: {e}");
        std::process::exit(1);
    }
    let k = &list.kists[i];
    println!("stowed {} ({} bytes, capsule {}), {} kist(s) in the person's list", k.name, capsule.len(), addr.iter().map(|b| format!("{b:02X}")).collect::<String>(), list.kists.len());
    for w in &k.wants {
        let a = match w.answer {
            _ if !w.asked() => "always (the implicit set, never asked)",
            None => "unanswered (asked at its first launch)",
            Some((true, _)) => "granted",
            Some((false, _)) => "not granted",
        };
        println!("  {}: {a} -- {}", ferros_kist::RIGHTS[w.right as usize], w.reason);
    }
}

/// `kist answer <name> <right> on|off|unset` (development; unset puts the want back to never asked): the attested person's answer for one want of a stowed kist, written into their list as the first-launch question or the settings' Kists page would write it. `kist list`: the list as it stands.
async fn cmd_kist_answer(args: &[String]) {
    let link = usb::UsbLink::open().await.unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let mut list = match store_call(&link, ferros_pt::Op::Read, b"kists.list").await {
        Ok(doc) => ferros_kist::Kists::parse(&doc).unwrap_or_else(|e| {
            eprintln!("the person's list does not parse: {e}");
            std::process::exit(1);
        }),
        Err(e) => {
            eprintln!("the person's list could not be read: {e}");
            std::process::exit(1);
        }
    };
    if args.len() >= 3 {
        let (name, right, on) = (&args[0], &args[1], args[2] == "on");
        let Some(code) = ferros_kist::code(right) else {
            eprintln!("{right} is not a right ({})", ferros_kist::RIGHTS.join(", "));
            std::process::exit(1);
        };
        let Some(w) = list.kists.iter_mut().find(|k| k.name.eq_ignore_ascii_case(name)).and_then(|k| k.wants.iter_mut().find(|w| w.right == code)) else {
            eprintln!("no kist {name} that wants {right}");
            std::process::exit(1);
        };
        w.answer = if args[2] == "unset" { None } else { Some((on, vsf::eagle_time_oscillations())) };
        let doc = list.build(vsf::eagle_time_oscillations()).expect("the list builds");
        let mut p = Vec::with_capacity(12 + doc.len());
        p.extend_from_slice(&(10u16).to_be_bytes());
        p.extend_from_slice(b"kists.list");
        p.extend_from_slice(&doc);
        if let Err(e) = store_call(&link, ferros_pt::Op::Write, &p).await {
            eprintln!("the person's list was not written: {e}");
            std::process::exit(1);
        }
    }
    for k in &list.kists {
        // The version as the device shows it (ferros_kist::write_version, decimal here: its significant parts) and, when they differ, the parts its developer signed.
        let version = match &k.version {
            Some(v) => {
                let shown = v.significant().iter().map(|p| p.to_string()).collect::<Vec<_>>().join(".");
                let signed = v.0.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(".");
                if shown == signed { format!("version {shown}") } else { format!("version {shown} (signed {signed})") }
            }
            None => "no version".to_string(),
        };
        println!("{} ({version}, capsule {})", k.name, k.capsule.iter().map(|b| format!("{b:02X}")).collect::<String>());
        for w in &k.wants {
            println!("  {}: {}", ferros_kist::RIGHTS[w.right as usize], match w.answer { _ if !w.asked() => "always", None => "unanswered", Some((true, _)) => "granted", Some((false, _)) => "not granted" });
        }
    }
    println!("defaults: {}", ferros_kist::RIGHTS.iter().enumerate().filter(|(c, _)| list.defaults >> c & 1 == 1).map(|(_, r)| *r).collect::<Vec<_>>().join(", "));
}

/// Send raw DISPLAY params and print the reply.
async fn cmd_display_raw(params: &[u8]) {
    let link = usb::UsbLink::open().await.unwrap_or_else(|e| {
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

/// Render text on the device's panel (shim, after a --live handoff) and print the device's register report.
async fn cmd_display(text: &str, hold: bool) {
    let link = usb::UsbLink::open().await.unwrap_or_else(|e| {
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
    let link = usb::UsbLink::open().await.unwrap_or_else(|e| {
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
    let link = match usb::UsbLink::open().await {
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
    let link = match usb::UsbLink::open().await {
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

    let link = match usb::UsbLink::open().await {
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
    let link = match usb::UsbLink::open().await {
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

    let link = match usb::UsbLink::open().await {
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

    let link = match usb::UsbLink::open().await {
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
    let link = match usb::UsbLink::open().await {
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

    let link = match usb::UsbLink::open().await {
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
    let link = match usb::UsbLink::open().await {
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
    let link = match usb::UsbLink::open().await {
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
