mod usb;

use std::io::{self, Read, Write};

fn usage() {
    eprintln!("ferros-bridge — USB bridge for ferros kernel");
    eprintln!();
    eprintln!("Usage: ferros-bridge <command>");
    eprintln!();
    eprintln!("Commands:");
    eprintln!("  log        Stream bulk IN data from device to stdout");
    eprintln!("  send <hex> Send hex bytes to device via bulk OUT");
    eprintln!("  terminal   Bidirectional: stdin → device, device → stdout");
    eprintln!("  echo       Send test pattern, verify echo");
    eprintln!("  status     Check if device is connected");
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
        "log" => cmd_log().await,
        "send" => {
            if args.len() < 3 {
                eprintln!("Usage: ferros-bridge send <hex-bytes>");
                eprintln!("Example: ferros-bridge send 48656c6c6f");
                std::process::exit(1);
            }
            cmd_send(&args[2]).await;
        }
        "echo" => cmd_echo().await,
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
        Ok(_link) => {
            println!("Device connected and ready");
        }
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

async fn cmd_log() {
    let link = match usb::UsbLink::open() {
        Ok(l) => l,
        Err(e) => { eprintln!("{e}"); std::process::exit(1); }
    };

    eprintln!("Streaming bulk IN... (Ctrl-C to stop)");
    loop {
        match link.recv().await {
            Ok(data) if !data.is_empty() => {
                // Print as raw bytes (kernel log stream)
                let _ = io::stdout().write_all(&data);
                let _ = io::stdout().flush();
            }
            Ok(_) => {} // empty, keep polling
            Err(e) => {
                eprintln!("\nUSB read error: {e}");
                break;
            }
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
        Err(e) => { eprintln!("{e}"); std::process::exit(1); }
    };

    match link.send(&data).await {
        Ok(()) => println!("Sent {} bytes", data.len()),
        Err(e) => eprintln!("Send failed: {e}"),
    }
}

async fn cmd_echo() {
    let link = match usb::UsbLink::open() {
        Ok(l) => l,
        Err(e) => { eprintln!("{e}"); std::process::exit(1); }
    };

    let test_data: Vec<u8> = (0..64).collect();
    eprintln!("Sending {} byte test pattern...", test_data.len());

    if let Err(e) = link.send(&test_data).await {
        eprintln!("Send failed: {e}");
        return;
    }

    match link.recv().await {
        Ok(data) => {
            if data == test_data {
                println!("Echo OK: {} bytes match", data.len());
            } else {
                eprintln!("Echo MISMATCH: sent {} bytes, got {} bytes", test_data.len(), data.len());
                eprintln!("Sent: {:02x?}", &test_data[..test_data.len().min(32)]);
                eprintln!("Recv: {:02x?}", &data[..data.len().min(32)]);
            }
        }
        Err(e) => eprintln!("Recv failed: {e}"),
    }
}

async fn cmd_terminal() {
    let link = match usb::UsbLink::open() {
        Ok(l) => l,
        Err(e) => { eprintln!("{e}"); std::process::exit(1); }
    };

    eprintln!("Terminal mode: stdin → device, device → stdout (Ctrl-C to exit)");

    // Spawn reader task (device → stdout)
    let link_ref = &link;
    let reader = tokio::spawn(async move {
        // We can't move link into spawn, so we use a different approach
        // For now, just a placeholder — proper implementation needs Arc
    });

    // For MVP: simple alternating send/recv
    // Full terminal mode with concurrent stdin/stdout needs Arc<UsbLink>
    // or splitting into separate IN/OUT handles
    let mut line = String::new();
    loop {
        line.clear();
        eprint!("> ");
        let _ = io::stderr().flush();
        match io::stdin().read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() { continue; }

                if let Err(e) = link.send(trimmed.as_bytes()).await {
                    eprintln!("Send failed: {e}");
                    break;
                }

                // Try to read response
                match link.recv().await {
                    Ok(data) if !data.is_empty() => {
                        let _ = io::stdout().write_all(&data);
                        let _ = io::stdout().flush();
                        println!();
                    }
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("Recv failed: {e}");
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

    reader.abort();
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.trim();
    if s.len() % 2 != 0 { return None; }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}
