//! The ferros emitter (SANDBOX.md, "What the emitter rewrites"): reads AArch64 assembly as rustc/LLVM emits it and writes it back in the forms the sandbox checker accepts. It runs before assembly, so labels and branches are resolved afterwards by the assembler.
//!
//! Usage: ferros-mask <in.s> <out.s>
//!
//! Register roles (reserved in codegen): x28 arc base, x27 mask, x26 masked offset, x25 address in the arc, x24 scratch; x23 a handle's base, x22 its mask, x21 an offset masked by it (SANDBOX.md, "Handles"; the forms are written by hand in ferros_sys and pass through untouched).

use std::fmt::Write as _;

/// Loads/stores that have a register-offset form (`[base, reg]`).
fn has_reg_offset(m: &str) -> bool {
    matches!(m, "ldr" | "str" | "ldrb" | "strb" | "ldrh" | "strh" | "ldrsb" | "ldrsh" | "ldrsw" | "prfm")
}

fn is_imm(s: &str) -> bool {
    s.starts_with('#') || s.starts_with(":lo12:")
}

/// `#N` → N (decimal or 0x hex); None for relocations.
fn imm_value(s: &str) -> Option<i64> {
    let t = s.strip_prefix('#')?.trim();
    if let Some(h) = t.strip_prefix("0x") {
        i64::from_str_radix(h, 16).ok()
    } else if let Some(h) = t.strip_prefix("-0x") {
        i64::from_str_radix(h, 16).ok().map(|v| -v)
    } else {
        t.parse().ok()
    }
}

/// Split at top-level commas (none of LLVM's operands nest brackets inside an address).
fn split_commas(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    for c in s.chars() {
        match c {
            '{' | '[' => {
                depth += 1;
                cur.push(c)
            }
            '}' | ']' => {
                depth -= 1;
                cur.push(c)
            }
            ',' if depth == 0 => {
                out.push(cur.trim().to_string());
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

struct Out {
    text: String,
    rewritten: usize,
    /// Fixed mode's K (SANDBOX.md, "Modes"): addresses are made safe with `bfi x26, xN, #0, #K` and used from x26 directly; None is ring mode.
    fixed: Option<u32>,
}

impl Out {
    fn ins(&mut self, s: &str) {
        let _ = writeln!(self.text, "\t{s}");
    }
    /// Move sp to `sp + delta` through the masked form.
    fn sp_adjust(&mut self, delta: i64) {
        if delta >= 0 {
            self.ins(&format!("add x24, sp, #{delta}"));
        } else {
            self.ins(&format!("sub x24, sp, #{}", -delta));
        }
        self.sp_from_x24();
    }
    fn sp_from_x24(&mut self) {
        match self.fixed {
            None => {
                self.ins("and x26, x24, x27");
                self.ins("add sp, x28, x26, uxtx");
            }
            Some(k) => {
                self.ins(&format!("bfi x26, x24, #0, #{k}"));
                self.ins("mov sp, x26");
            }
        }
    }
    /// `src` made safe: ring mode the masked offset in x26, fixed mode the address itself in x26.
    fn safe(&mut self, src: &str) {
        match self.fixed {
            None => self.ins(&format!("and x26, {src}, x27")),
            Some(k) => self.ins(&format!("bfi x26, {src}, #0, #{k}")),
        }
    }
    fn base_update(&mut self, base: &str, delta: i64) {
        if delta >= 0 {
            self.ins(&format!("add {base}, {base}, #{delta}"));
        } else {
            self.ins(&format!("sub {base}, {base}, #{}", -delta));
        }
    }
}

/// Rewrite one instruction (mnemonic + operand text). Returns false if it passes through unchanged.
fn rewrite(o: &mut Out, mnem: &str, ops: &str) -> Result<bool, String> {
    // An address is a bracket that opens an operand (`[x8]`, `str w0, [x1, #4]`); a bracket inside an operand is a vector lane (`mov v1.b[4], v0.b[6]`) and no memory access.
    let address = ops.match_indices('[').map(|(i, _)| i).find(|&open| open == 0 || ops[..open].trim_end().ends_with(','));
    if let Some(open) = address {
        let close = ops[open..].find(']').map(|c| open + c).ok_or("unclosed address")?;
        let pre = ops[..open].trim().trim_end_matches(',').trim();
        let inner = split_commas(&ops[open + 1..close]);
        let post = ops[close + 1..].trim();
        let base = inner.first().ok_or("empty address")?.as_str();
        let writeback_pre = post == "!";
        let writeback_post = post.starts_with(',');
        // The handle forms (SANDBOX.md, "Handles"), written by hand in ferros_sys and left as they are: the pair load from the table and an access through the pair.
        if mnem == "ldp" && pre == "x23, x22" && base == "x28" && !writeback_pre && !writeback_post && (inner.len() == 1 || (inner.len() == 2 && is_imm(&inner[1]))) {
            return Ok(false);
        }
        if base == "x23" && inner.len() == 2 && inner[1] == "x21" && !writeback_pre && !writeback_post {
            return Ok(false);
        }
        if matches!(base, "x21" | "x22" | "x23" | "x24" | "x25" | "x26" | "x27" | "x28") {
            return Err(format!("reserved register as base: {mnem} {ops}"));
        }
        let post_imm = if writeback_post { imm_value(post.trim_start_matches(',').trim()) } else { None };
        let access = |o: &mut Out, addr: &str| o.ins(&format!("{mnem} {pre}, {addr}"));
        if base == "sp" {
            match (inner.len(), writeback_pre, writeback_post) {
                // [sp] / [sp, #imm] without writeback: already allowed.
                (1, false, false) => return Ok(false),
                // A non-negative offset only: a negative one reaches below the arc base, so it is folded into the masked address below.
                (2, false, false) if is_imm(&inner[1]) && imm_value(&inner[1]).is_none_or(|v| v >= 0) => return Ok(false),
                // [sp, #imm]! : move sp first, then access [sp].
                (2, true, false) => {
                    let d = imm_value(&inner[1]).ok_or("pre-index needs a number")?;
                    o.sp_adjust(d);
                    access(o, "[sp]");
                    return Ok(true);
                }
                // [sp], #imm : access [sp], then move sp.
                (1, false, true) => {
                    access(o, "[sp]");
                    o.sp_adjust(post_imm.ok_or("post-index needs a number")?);
                    return Ok(true);
                }
                _ => {} // [sp, reg...]: handled as a computed address below
            }
        }
        // A general base (or sp with a register offset).
        if writeback_pre {
            let d = imm_value(&inner[1]).ok_or("pre-index needs a number")?;
            o.base_update(base, d);
        }
        let computed = inner.len() >= 2 && !is_imm(&inner[1]);
        // A negative immediate (no writeback): the address computed first, then masked, then accessed with no offset; the checker admits no access below the arc base.
        let neg = if inner.len() == 2 && !writeback_pre && is_imm(&inner[1]) { imm_value(&inner[1]).filter(|&v| v < 0) } else { None };
        if let Some(v) = neg {
            o.ins(&format!("sub x24, {base}, #{}", -v));
            o.safe("x24");
            if o.fixed.is_some() {
                access(o, "[x26]");
            } else if has_reg_offset(mnem) {
                access(o, "[x28, x26]");
            } else {
                o.ins("add x25, x28, x26");
                access(o, "[x25]");
            }
            return Ok(true);
        }
        if computed {
            // Register offset (with optional shift/extend): fold into x24 first.
            o.ins(&format!("add x24, {base}, {}", inner[1..].join(", ")));
            o.safe("x24");
        } else {
            o.safe(base);
        }
        let imm = if inner.len() == 2 && is_imm(&inner[1]) && !writeback_pre { Some(inner[1].as_str()) } else { None };
        match imm {
            // Fixed mode: x26 is the address; the immediate, when there is one, stays on the access.
            None if o.fixed.is_some() => access(o, "[x26]"),
            Some(i) if o.fixed.is_some() => access(o, &format!("[x26, {i}]")),
            None if has_reg_offset(mnem) => access(o, "[x28, x26]"),
            None => {
                o.ins("add x25, x28, x26");
                access(o, "[x25]");
            }
            Some(i) => {
                o.ins("add x25, x28, x26");
                access(o, &format!("[x25, {i}]"));
            }
        }
        if writeback_post {
            o.base_update(base, post_imm.ok_or("post-index needs a number")?);
        }
        return Ok(true);
    }
    // A write to sp outside a memory access: compute into x24, then the masked form.
    let parts = split_commas(ops);
    if parts.first().map(|p| p == "sp").unwrap_or(false) && !matches!(mnem, "cmp" | "cmn" | "tst") {
        if ops.trim() == "sp, x28, x26, uxtx" && mnem == "add" {
            return Ok(false);
        }
        o.ins(&format!("{mnem} x24, {}", parts[1..].join(", ")));
        o.sp_from_x24();
        return Ok(true);
    }
    Ok(false)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // `--fixed K` (SANDBOX.md, "Modes"): fixed mode at arc size 2^K; without it, ring mode.
    let fixed = args.iter().position(|a| a == "--fixed").map(|i| args.get(i + 1).and_then(|v| v.parse::<u32>().ok()).filter(|k| (16..=40).contains(k)).unwrap_or_else(|| {
        eprintln!("--fixed needs K, 16 to 40");
        std::process::exit(2)
    }));
    let args: Vec<String> = args.iter().enumerate().filter(|(i, a)| *a != "--fixed" && !(fixed.is_some() && *i > 0 && args[*i - 1] == "--fixed")).map(|(_, a)| a.clone()).collect();
    if args.len() != 3 {
        eprintln!("usage: ferros-mask <in.s> <out.s> [--fixed K]");
        std::process::exit(2);
    }
    let src = std::fs::read_to_string(&args[1]).expect("read input");
    let mut o = Out { text: String::with_capacity(src.len() * 5 / 4), rewritten: 0, fixed };
    for (n, line) in src.lines().enumerate() {
        let code = line.split("//").next().unwrap_or("");
        let t = code.trim();
        // Directives, labels and blank lines pass through.
        if t.is_empty() || t.starts_with('.') || t.ends_with(':') || !line.starts_with(|c: char| c.is_whitespace()) {
            let _ = writeln!(o.text, "{line}");
            continue;
        }
        let (mnem, ops) = match t.split_once(char::is_whitespace) {
            Some((m, r)) => (m, r.trim()),
            None => (t, ""),
        };
        match rewrite(&mut o, mnem, ops) {
            Ok(true) => o.rewritten += 1,
            Ok(false) => {
                let _ = writeln!(o.text, "{line}");
            }
            Err(e) => {
                eprintln!("{}:{}: {e}", args[1], n + 1);
                std::process::exit(1);
            }
        }
    }
    std::fs::write(&args[2], &o.text).expect("write output");
    eprintln!("{}: {} instructions rewritten", args[1], o.rewritten);
}
