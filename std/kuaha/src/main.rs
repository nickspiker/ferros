//! Kūaha on ferros as a kist (docs/KISTS.md "Kūaha"): its core (std/kuaha/core) in a window that fills the glass thru fluor's ferros host, the hold reached thru the kist's own `items` grant on a thread of its own, its choices kept in its own vault domain.
//! Its grant slots, as ferros lays out a kist (ferros_kist `slots`): 1 display, 2 touch, 3 surface, 4 ira, 5 its vault domain, 6 entropy, then its one want, `items`, at 7 (tools/kuaha-kist.sh declares it, with the kinds it opens).
//! Built for ferros alone (tools/kuaha-build.sh); on another target the binary only says so, and the core's tests are `cargo test -p kuaha_core`.

#[cfg(target_os = "ferros")]
mod sys;

#[cfg(target_os = "ferros")]
mod window {
    use crate::sys;
    use fluor::coord::Coord;
    use fluor::event::{CursorIcon, ElementState, Event, Ime, Key, MouseButton, NamedKey};
    use fluor::host::app::{Context, FluorApp, SafeArea};
    use fluor::host::{EventResponse, WakeSender};
    use fluor::pixel::G2t16;
    use ferros_items::kist::Sys;
    use kuaha_core::{serve, Answer, Ask, Circle, Kuaha, Locale, Over, Prefs};
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::Arc;

    const DISPLAY: u64 = 1;
    const TOUCH: u64 = 2;
    const SURFACE: u64 = 3;
    const IRA: u64 = 4;
    /// The `items` right, Kūaha's one want (slot 7: the first after the implicit six).
    const ITEMS: u64 = 7;

    fn now() -> i64 {
        vsf::types::eagle_time::eagle_time_oscillations()
    }

    fn locale() -> Locale {
        Locale { lang: sys::lang(), base: sys::base(), zone: sys::zone(), now: now(), theme: sys::theme() }
    }

    struct App {
        k: Kuaha,
        /// The hold's thread: the asks go there, the answers come back as user events.
        hold: Sender<Vec<Ask>>,
        /// The hold's thread before the host handed the proxy its answers ride on.
        waiting: Option<(Receiver<Vec<Ask>>, bool)>,
        /// The vault's writer, when the vault opened.
        vault: Option<Sender<Vec<u8>>>,
        cursor: (Coord, Coord),
        down: bool,
    }

    impl App {
        /// Whatever the model asked of the hold goes to its thread, and its choices to the vault, at once (AGENT.md "Persistence Rule").
        fn flush(&mut self) {
            let asks = self.k.take_asks();
            if !asks.is_empty() {
                let _ = self.hold.send(asks);
            }
            if let (Some(p), Some(v)) = (self.k.take_prefs(), &self.vault) {
                match p.build(now()) {
                    Ok(doc) => {
                        let _ = v.send(doc);
                    }
                    Err(e) => println!("kuaha: its choices did not build: {e}"),
                }
            }
        }
    }

    /// The hold's thread: each ask run on the kist's `items` grant in turn, the quick ones always ahead of a kist's sizing, each answer handed to the window as it comes.
    fn holder(rx: Receiver<Vec<Ask>>, right: bool, proxy: Arc<dyn WakeSender<Answer>>) {
        let mut src = Over::new(Sys { cap: sys::cap(ITEMS) }, now, right);
        let mut queue: Vec<Ask> = Vec::new();
        loop {
            if queue.is_empty() {
                match rx.recv() {
                    Ok(a) => queue.extend(a),
                    Err(_) => return,
                }
            }
            while let Ok(a) = rx.try_recv() {
                queue.extend(a);
            }
            queue.sort_by_key(|a| a.slow());
            let ask = queue.remove(0);
            if proxy.send(serve(&mut src, ask)).is_err() {
                return;
            }
        }
    }

    impl FluorApp<G2t16> for App {
        type UserEvent = Answer;

        fn set_event_proxy(&mut self, proxy: Arc<dyn WakeSender<Answer>>) {
            if let Some((rx, right)) = self.waiting.take() {
                let spawned = std::thread::Builder::new().name("kuaha-hold".into()).spawn(move || holder(rx, right, proxy));
                if let Err(e) = spawned {
                    println!("kuaha: no thread for the hold ({e})");
                }
            }
        }

        fn init(&mut self, _ctx: &mut Context) {
            self.flush();
        }

        fn on_resize(&mut self, _width: u32, _height: u32, ctx: &mut Context) {
            ctx.window.request_redraw();
        }

        fn on_safe_area(&mut self, area: SafeArea) {
            self.k.set_circle(Circle { cx: area.cx, cy: area.cy, r: area.r });
        }

        fn on_event(&mut self, event: &Event, ctx: &mut Context) -> EventResponse {
            match event {
                Event::CursorMoved { x, y } => {
                    self.cursor = (*x, *y);
                    if self.down {
                        self.k.move_to(*x, *y);
                    }
                }
                Event::MouseInput { state: ElementState::Pressed, button: MouseButton::Left } => {
                    self.down = true;
                    self.k.press(self.cursor.0, self.cursor.1);
                }
                Event::MouseInput { state: ElementState::Released, button: MouseButton::Left } => {
                    self.down = false;
                    self.k.release(self.cursor.0, self.cursor.1);
                }
                Event::CursorLeft => {
                    self.down = false;
                    self.k.leave();
                }
                Event::Ime(Ime::Commit(s)) => self.k.typed(s),
                Event::KeyboardInput { event } if event.state == ElementState::Pressed => match &event.logical_key {
                    Key::Named(NamedKey::Backspace) => self.k.backspace(),
                    Key::Named(NamedKey::Enter) => self.k.enter(),
                    _ => return EventResponse::Pass,
                },
                Event::Focused(front) => self.k.focus(*front),
                _ => return EventResponse::Pass,
            }
            self.flush();
            ctx.window.request_redraw();
            EventResponse::Handled
        }

        fn on_user_event(&mut self, answer: Answer, ctx: &mut Context) -> EventResponse {
            self.k.answer(answer);
            self.flush();
            ctx.window.request_redraw();
            EventResponse::Handled
        }

        fn wants_keyboard(&mut self) -> Option<bool> {
            self.k.wants_keyboard()
        }

        fn tick(&mut self, _ctx: &mut Context) -> bool {
            self.k.tick()
        }

        fn render(&mut self, target: &mut [G2t16], ctx: &mut Context) {
            let (w, h) = (ctx.viewport.width_px as usize, ctx.viewport.height_px as usize);
            self.k.draw(target, w, h, &mut *ctx.text, &locale(), &mut *ctx.damage);
        }

        fn opaque(&self) -> bool {
            true
        }

        fn cursor_for(&self, _x: Coord, _y: Coord, _ctx: &Context) -> CursorIcon {
            CursorIcon::Default
        }
    }

    /// The kist's vault domain: its key from the kist's ira, the store the kernel scopes to the kist (slot 5). None without either: Kūaha then keeps nothing of its own this launch, and says so.
    fn open_vault() -> Option<Arc<kete::FlatStorage>> {
        let Some(secret) = sys::ira_key(IRA, "kuaha vault") else {
            println!("kuaha: no ira key (slot {IRA}): its choices are not kept this launch");
            return None;
        };
        match kete::FlatStorage::open_device_shared(kete::App { id: "kuaha", dir: "kuaha" }, secret) {
            Ok(v) => Some(v),
            Err(e) => {
                println!("kuaha: the vault did not open ({e:?}): its choices are not kept this launch");
                None
            }
        }
    }

    /// The vault's writer: each document as it comes, only the newest when several wait, so a run of letters typed is one write.
    fn writer(vault: Arc<kete::FlatStorage>) -> Sender<Vec<u8>> {
        let (tx, rx) = channel::<Vec<u8>>();
        let spawned = std::thread::Builder::new().name("kuaha-vault".into()).spawn(move || {
            while let Ok(mut doc) = rx.recv() {
                while let Ok(newer) = rx.try_recv() {
                    doc = newer;
                }
                if let Err(e) = vault.write_device(kuaha_core::model::PREFS_NAME, &doc) {
                    println!("kuaha: its choices were not kept: {e:?}");
                }
            }
        });
        if let Err(e) = spawned {
            println!("kuaha: no vault writer ({e}): its choices are not kept this launch");
        }
        tx
    }

    pub fn main() {
        std::panic::set_hook(Box::new(|info| {
            let at = info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default();
            println!("kuaha: PANIC at {at}: {info}");
        }));
        let right = sys::held(ITEMS);
        println!("kuaha: Kūaha {} on ferros, {} grants, the items right {}", env!("CARGO_PKG_VERSION"), sys::grant_count(), if right { "held" } else { "not held" });
        let vault = open_vault();
        let prefs = vault.as_ref().and_then(|v| match v.read_device(kuaha_core::model::PREFS_NAME) {
            Ok(Some(doc)) => Prefs::parse(&doc).map_err(|e| println!("kuaha: its kept choices do not read ({e}): starting fresh")).ok(),
            _ => None,
        });
        let (tx, rx) = channel();
        let app = App { k: Kuaha::new(prefs, now()), hold: tx, waiting: Some((rx, right)), vault: vault.map(writer), cursor: (0., 0.), down: false };
        let caps = fluor::host::ferros::Caps { display: DISPLAY, touch: TOUCH, surface: SURFACE };
        match fluor::host::ferros::run_app(app, caps, 1.0, None) {
            Ok(()) => println!("kuaha: closed"),
            Err(e) => println!("kuaha: {e}"),
        }
    }
}

#[cfg(target_os = "ferros")]
fn main() {
    window::main()
}

#[cfg(not(target_os = "ferros"))]
fn main() {
    eprintln!("kuaha: the kist runs on ferros (tools/kuaha-build.sh); its core's tests are `cargo test -p kuaha_core`");
}
