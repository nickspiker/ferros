//! Google's USB driver on Windows, installed by the guide itself (the coordinator, 2026-11-24, for the first real user: no Android Studio, no platform-tools, no adb).
//! Windows opens a phone's bootloader (and its ADB interface) for nusb only thru WinUSB, and the Pixel's bootloader does not ask Windows for it, so a bare Windows sees an unknown device.
//! Google's own package (an INF binding WinUSB, signed by Google) comes straight from Google's SDK repository, never rehosted: the URL as Google's manifest names it (addon2-*.xml, `extras;google;usb_driver`, revision 13), its SHA-256 pinned here and checked by publish.sh against Google's file at every publish.
//! It is checked before anything is unpacked, unpacked into the temp folder, and installed with Windows' own pnputil, elevated with one UAC prompt.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The package, as Google's SDK repository manifest names it (addon2-3.xml, 2026-11-24: size A#8682039, SHA-1 08a48c39...).
pub const URL: &str = "https://dl.google.com/android/repository/usb_driver_r13-windows.zip";
/// Its SHA-256, taken from Google's file on 2026-11-24 (its SHA-1 matched the manifest's); publish.sh refuses to publish if Google's file no longer has it.
pub const SHA256: &str = "360b01d3dfb6c41621a3a64ae570dfac2c9a40cca1b5a1f136ae90d02f5e9e0b";
pub const SIZE: u64 = 8_682_039;

/// What happened, step by step, for the run's log.
pub struct Step {
    pub what: &'static str,
    pub ok: bool,
    pub words: String,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Whether `bytes` are Google's package as pinned.
pub fn check(bytes: &[u8]) -> Result<(), String> {
    use sha2::Digest;
    let got = hex(&sha2::Sha256::digest(bytes));
    if bytes.len() as u64 != SIZE || got != SHA256 {
        return Err(format!(
            "the download is not the package Google published (A#{} bytes, SHA-256 {got})",
            bytes.len()
        ));
    }
    Ok(())
}

/// A path as a PowerShell single-quoted string.
fn ps_quote(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "''"))
}

/// The text pnputil wrote (Windows PowerShell's redirection writes UTF-16 with a byte-order mark).
fn read_text(p: &Path) -> String {
    let b = std::fs::read(p).unwrap_or_default();
    if b.starts_with(&[0xFF, 0xFE]) {
        let u: Vec<u16> = b[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&u);
    }
    String::from_utf8_lossy(&b).into_owned()
}

/// Download, check, unpack, install; every step reported to `on` as it ends. Stops at the first step that fails.
pub fn install(
    fetch: impl Fn(&str) -> Result<Vec<u8>, String>,
    mut on: impl FnMut(Step),
) -> Result<(), String> {
    let mut step = |what: &'static str, r: Result<String, String>| -> Result<String, String> {
        match &r {
            Ok(w) => on(Step {
                what,
                ok: true,
                words: w.clone(),
            }),
            Err(e) => on(Step {
                what,
                ok: false,
                words: e.clone(),
            }),
        }
        r
    };
    let dir: PathBuf = std::env::temp_dir().join("ferros-usb-driver");
    let zip = dir.join("usb_driver_r13-windows.zip");
    let fetched = fetch(URL);
    step(
        "download",
        fetched
            .as_ref()
            .map(|b| format!("A#{} bytes from {URL}", b.len()))
            .map_err(|e| format!("from {URL}: {e}")),
    )?;
    let bytes = fetched?;
    step(
        "check",
        check(&bytes).map(|_| format!("SHA-256 {SHA256}, as pinned")),
    )?;
    step(
        "unpack",
        std::fs::create_dir_all(&dir)
            .and_then(|_| std::fs::write(&zip, &bytes))
            .map_err(|e| format!("{}: {e}", zip.display()))
            .and_then(|_| {
                let o = Command::new("powershell")
                    .args(["-NoProfile", "-NonInteractive", "-Command"])
                    .arg(format!(
                        "Expand-Archive -LiteralPath {} -DestinationPath {} -Force",
                        ps_quote(&zip),
                        ps_quote(&dir)
                    ))
                    .output()
                    .map_err(|e| format!("powershell: {e}"))?;
                if !o.status.success() {
                    return Err(format!(
                        "Expand-Archive: {}",
                        String::from_utf8_lossy(&o.stderr).trim()
                    ));
                }
                Ok(format!("into {}", dir.display()))
            }),
    )?;
    let inf = dir.join("usb_driver").join("android_winusb.inf");
    if !inf.exists() {
        return step(
            "unpack",
            Err(format!("no {} in the package", inf.display())),
        )
        .map(|_| ());
    }
    // pnputil runs elevated in its own PowerShell, from a script file (no command text to quote), its words written to a file this side reads.
    let script = dir.join("install-driver.ps1");
    let out = dir.join("pnputil.txt");
    let _ = std::fs::remove_file(&out);
    std::fs::write(
        &script,
        format!(
            "pnputil.exe /add-driver \"{}\" /install *> \"{}\"\r\nexit $LASTEXITCODE\r\n",
            inf.display(),
            out.display()
        ),
    )
    .map_err(|e| format!("{}: {e}", script.display()))?;
    let elevate = format!(
        "$p = Start-Process powershell -Verb RunAs -Wait -PassThru -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','\"{}\"'; exit $p.ExitCode",
        script.display().to_string().replace('\'', "''")
    );
    let o = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &elevate])
        .output()
        .map_err(|e| format!("powershell: {e}"))?;
    let said = read_text(&out);
    let code = o.status.code().unwrap_or(-1);
    let words = format!(
        "pnputil exit A#{code}: {}",
        said.split_whitespace().collect::<Vec<_>>().join(" ")
    );
    // 0 installed; 259 added, no device present to bind yet; 3010 installed, a restart asked.
    match code {
        0 | 259 | 3010 => step("install", Ok(words)).map(|_| ()),
        _ if said.is_empty() => step(
            "install",
            Err(format!(
                "the administrator prompt was refused or failed ({}): {}",
                code,
                String::from_utf8_lossy(&o.stderr).trim()
            )),
        )
        .map(|_| ()),
        _ => step("install", Err(words)).map(|_| ()),
    }
}

/// What a person does by hand when the tool cannot (Google's file gone, or the install refused).
pub const BY_HAND: &str = "To install the driver by hand instead (no Android tools needed):
  - Settings, Windows Update, Advanced options, Optional updates, Driver updates: tick \"Google, Inc. - Android Bootloader Interface\" (or any Google Android ADB or Bootloader driver), then Download and install.
  - Or Device Manager: find the phone (under \"Other devices\", perhaps named Android), right-click it, Update driver, Search automatically for drivers.
Then unplug the phone, plug it back in, and run ferros-flash again.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pin_refuses_anything_else() {
        assert!(check(b"not google's package").is_err());
        assert_eq!(SHA256.len(), 64);
    }
}
