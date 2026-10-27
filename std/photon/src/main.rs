//! Photon on ferros: the app Photon's library builds, run through fluor's ferros host over the display server's app surface and the touch server. Spawned by the shell with (slot order) the display and touch endpoints, the app surface, the udp endpoint, the photon vault scope, the ira and entropy: `--use display --use touch --dma app-surface --use udp --store photon --ira --entropy`. The desktop main (photon's src/main.rs) does the winit things; this one does the ferros things.

use photon_messenger::ui::photon_app::PhotonApp;

fn main() {
    photon_messenger::init_logging();
    photon_messenger::log_version();
    std::panic::set_hook(Box::new(|info| {
        let location = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
        println!("PANIC at {location}: {info}");
    }));
    let caps = fluor::host::ferros::Caps { display: 1, touch: 2, surface: 3 };
    match fluor::host::ferros::run_app(PhotonApp::new(), caps, 1.0, None) {
        Ok(()) => println!("photon: closed"),
        Err(e) => println!("photon: {e}"),
    }
}
