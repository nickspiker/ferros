//! `items`: the hold on the device, as the person (docs/STORE.md, docs/HOLD-API.md), for the bench: find, card, get by range into a file, put a file as an item (a RAW clip as a single-copy body), remove.
//!
//! Each call is one STORE command, job G#F9 (service.rs `VaultJob::Items`): [G#F9][the call as ferros_items `wire` makes it], answered [0][the value u64 LE][the bytes]; the kernel stamps it as the person (process.rs `vault_stamp_items`), who may open every kind.
//! The calls are ferros_items `kist::Hold`'s, the same a kist makes, over this transport.
//! A handle is the vault process's token for an item to the person; it lasts while that vault process runs and the person stays unlocked, so `find` again after a reboot or a lock.

use ferros_items::kist::{Hold, Transport, ERR};
use ferros_items::{kinds, Card, Find, Proposed, ORDER_LARGEST, ORDER_NEWEST, ORDER_OLDEST, ORDER_TITLE};

use crate::usb;

/// The bench's transport: each call a STORE command over the link, waited for here (the calls are `Hold`'s, which are synchronous).
struct Bench {
    link: usb::UsbLink,
}

impl Transport for Bench {
    fn call(&mut self, call: &[u8], reply: &mut [u8]) -> u64 {
        let mut p = Vec::with_capacity(1 + call.len());
        p.push(0xF9);
        p.extend_from_slice(call);
        let cmd = crate::build_cmd(ferros_pt::command::caps::STORE, ferros_pt::Op::Read, &p);
        let r = tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(ferros_link::pt::call_held(&self.link, &cmd)));
        let r = match r {
            Ok(r) => r,
            Err(e) => {
                eprintln!("items: the command failed: {e}");
                return ERR;
            }
        };
        match r.first() {
            Some(0) if r.len() >= 9 => {
                let value = u64::from_le_bytes(r[1..9].try_into().unwrap());
                let bytes = &r[9..];
                // The vault fills no more than the room the call named (its length); this copies what came.
                let n = bytes.len().min(reply.len());
                reply[..n].copy_from_slice(&bytes[..n]);
                value
            }
            Some(s) => {
                eprintln!("items: {}", match s { 1 => "not found", 2 => "vault error (is the vault process up, and someone attested?)", 3 => "bad request", 4 => "no vault open on the device", 5 => "refused: a call past one transfer", _ => "unknown status" });
                ERR
            }
            None => ERR,
        }
    }
}

fn clock() -> i64 {
    vsf::eagle_time_oscillations()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect()
}

fn handle_of(s: &str) -> [u8; 32] {
    let bad = || -> ! {
        eprintln!("{s}: a handle is the 64 hex digits `items find` printed");
        std::process::exit(1)
    };
    if s.len() != 64 {
        bad();
    }
    let mut h = [0u8; 32];
    for (k, b) in h.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * k..2 * k + 2], 16).unwrap_or_else(|_| bad());
    }
    h
}

fn kind_of(p: &str) -> [u8; 32] {
    kinds::by_phrasing(p).unwrap_or_else(|| {
        eprintln!("{p}: not one of the project's kinds; `items kinds` lists them");
        std::process::exit(1)
    })
}

/// An Eagle time as the host's local wall time, for display only.
fn wall(osc: i64) -> String {
    match vsf::EagleTime::from_oscillations(osc).to_datetime_opt() {
        Some(t) => t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S").to_string(),
        None => format!("(Eagle time A#{osc})"),
    }
}

fn show(h: &[u8; 32], c: &Card) {
    let kind = kinds::phrasing(&c.kind).map(String::from).unwrap_or_else(|| format!("kind {}", hex(&c.kind[..8])));
    let body = match c.extent {
        Some(x) => format!("a single-copy body, A#{} blocks at A#{} of the clips area", x.blocks, x.first),
        None => String::from("a stored body"),
    };
    println!("{}  {kind}, {:?}, A#{} bytes, made {} by {}, {body}", hex(h), c.title.as_deref().unwrap_or(""), c.size, wall(c.made), if c.kist.name.is_empty() { "the person" } else { c.kist.name.as_str() });
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(|s| s.as_str())
}

fn num(args: &[String], name: &str) -> Option<u64> {
    flag(args, name).map(|v| {
        v.parse().unwrap_or_else(|_| {
            eprintln!("{name}: a decimal count");
            std::process::exit(1)
        })
    })
}

fn usage() -> ! {
    eprintln!("Usage: ferros-bridge items <command>");
    eprintln!("  kinds                                   The project's kinds, by their first phrasing");
    eprintln!("  find [--kind K]... [--words W] [--order newest|oldest|title|largest] [--skip N] [--count N]");
    eprintln!("  card <handle>");
    eprintln!("  get <handle> <out> [--from N] [--bytes N]   The body (or a range of it) streamed into out, a MiB a command");
    eprintln!("  put <kind> <file> [--title T] [--clip]      A file kept as an item; --clip: a single-copy body (a RAW clip), written once, never mirrored");
    eprintln!("  rm <handle>                             Removed, gone at once (no bin)");
    eprintln!("  usage                                   What is on the device: the hold by kind, the clips area, each stowed kist's own domain");
    std::process::exit(1)
}

pub async fn cmd_items(args: &[String]) {
    let Some(what) = args.first().map(|s| s.as_str()) else { usage() };
    if what == "kinds" {
        for (p, id) in kinds::KINDS {
            println!("{p:28} {}", hex(id));
        }
        return;
    }
    let link = usb::UsbLink::open().await.unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });
    let mut hold = Hold::new(Bench { link }, clock);
    let fail = |e: ferros_items::Error| -> ! {
        eprintln!("items {what}: {e:?}");
        std::process::exit(1)
    };
    match what {
        "find" => {
            let order = match flag(args, "--order").unwrap_or("newest") {
                "newest" => ORDER_NEWEST,
                "oldest" => ORDER_OLDEST,
                "title" => ORDER_TITLE,
                "largest" => ORDER_LARGEST,
                o => {
                    eprintln!("--order {o}: newest, oldest, title or largest");
                    std::process::exit(1)
                }
            };
            let kinds = args.iter().enumerate().filter(|(_, a)| *a == "--kind").filter_map(|(i, _)| args.get(i + 1)).map(|k| kind_of(k)).collect();
            let f = Find { kinds, words: flag(args, "--words").map(String::from), order, skip: num(args, "--skip").unwrap_or(0), count: num(args, "--count").unwrap_or(0), ..Default::default() };
            let r = hold.find(&f).unwrap_or_else(|e| fail(e));
            println!("A#{} items match (the index's change count A#{}), A#{} here", r.total, r.generation, r.items.len());
            for (h, c) in &r.items {
                show(h, c);
            }
        }
        "card" if args.len() >= 2 => {
            let h = handle_of(&args[1]);
            let c = hold.card(&h).unwrap_or_else(|e| fail(e));
            show(&h, &c);
            println!("  id {}, body BLAKE3 {}, changed {}, keeper A#{}", hex(c.id.as_bytes()), hex(&c.body), wall(c.changed), c.keeper);
            for (name, v) in &c.attrs {
                println!("  {name}: {v:?}");
            }
            for (id, how) in &c.of {
                println!("  of {} as A#{how}", hex(id.as_bytes()));
            }
        }
        "get" if args.len() >= 3 => {
            let h = handle_of(&args[1]);
            let from = num(args, "--from").unwrap_or(0);
            let size = hold.card(&h).unwrap_or_else(|e| fail(e)).size;
            // WHY: the range is the person's words; it ends at the body's end.
            let want = num(args, "--bytes").unwrap_or(u64::MAX).min(size.saturating_sub(from));
            let mut file = std::fs::File::create(&args[2]).unwrap_or_else(|e| {
                eprintln!("{}: {e}", args[2]);
                std::process::exit(1)
            });
            let t = std::time::Instant::now();
            let mut buf = vec![0u8; ferros_items::wire::TRANSFER];
            let mut hasher = blake3::Hasher::new();
            let mut at = 0u64;
            while at < want {
                let k = ((want - at) as usize).min(buf.len());
                let (n, _) = hold.read(&h, from + at, &mut buf[..k]).unwrap_or_else(|e| fail(e));
                if n == 0 {
                    break;
                }
                std::io::Write::write_all(&mut file, &buf[..n]).unwrap();
                hasher.update(&buf[..n]);
                at += n as u64;
            }
            println!("A#{at} bytes from A#{from} into {} in A#{} ms (A#{:.1} MB/s), BLAKE3 {}", args[2], t.elapsed().as_millis(), at as f64 / 1e6 / t.elapsed().as_secs_f64(), hex(hasher.finalize().as_bytes()));
        }
        "put" if args.len() >= 3 => {
            let kind = kind_of(&args[1]);
            let body = std::fs::read(&args[2]).unwrap_or_else(|e| {
                eprintln!("{}: {e}", args[2]);
                std::process::exit(1)
            });
            let p = Proposed { kind, title: flag(args, "--title").map(String::from), ..Default::default() };
            let t = std::time::Instant::now();
            let (h, c) = if args.iter().any(|a| a == "--clip") {
                let blocks = (body.len() as u64).div_ceil(4096);
                let clip = hold.make_clip(&p, blocks).unwrap_or_else(|e| fail(e));
                let mut padded = body.clone();
                padded.resize((blocks as usize) << 12, 0);
                for (k, chunk) in padded.chunks(ferros_items::wire::TRANSFER).enumerate() {
                    hold.write_clip(&clip, ((k * ferros_items::wire::TRANSFER) >> 12) as u64, chunk).unwrap_or_else(|e| fail(e));
                }
                let h = clip.handle;
                (h, hold.seal_clip(clip, body.len() as u64, blake3::hash(&body).as_bytes()).unwrap_or_else(|e| fail(e)))
            } else {
                hold.keep(&p, &body).unwrap_or_else(|e| fail(e))
            };
            println!("kept in A#{} ms:", t.elapsed().as_millis());
            show(&h, &c);
        }
        "usage" => {
            let t = std::time::Instant::now();
            let u = hold.usage().unwrap_or_else(|e| fail(e));
            println!("what is on the device ({} ms):", t.elapsed().as_millis());
            let (n, b) = u.kinds.iter().fold((0, 0), |(n, b), k| (n + k.items, b + k.bytes));
            println!("the hold: A#{n} items, A#{b} bytes");
            for k in &u.kinds {
                println!("  {:28} A#{} items, A#{} bytes", kinds::phrasing(&k.kind).map(String::from).unwrap_or_else(|| hex(&k.kind[..8])), k.items, k.bytes);
            }
            if let Some(c) = u.clips {
                println!("the clips area: A#{} blocks (A#{} GiB), A#{} taken, A#{} free, the longest free stretch A#{} blocks", c.blocks, c.blocks >> 18, c.taken, c.blocks - c.taken, c.largest_free);
            }
            println!("the stowed kists' own domains:");
            for k in &u.kists {
                let size = match (k.values, k.bytes) {
                    (Some(v), Some(b)) => format!("A#{v} values, A#{b} bytes"),
                    _ => String::from("not sized"),
                };
                println!("  {:16} {size}, its clips A#{} blocks", if k.name.is_empty() { "(not in the list)" } else { k.name.as_str() }, k.clip_blocks);
            }
        }
        "rm" if args.len() >= 2 => {
            hold.remove(&handle_of(&args[1])).unwrap_or_else(|e| fail(e));
            println!("removed");
        }
        _ => usage(),
    }
}
