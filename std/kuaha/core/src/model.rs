//! What Kūaha knows and wants, without a screen: the find the person built (kind, time, maker, words, order), the page of cards it answered, the hold's totals, the device's usage, which card is open and whether its remove is armed.
//! The model never calls the hold: it queues asks (`take_asks`), the platform runs them off the drawing thread thru `serve`, and their answers come back (`answer`), each find tagged with the generation it was asked under so a stale answer is dropped.
//! Nothing is asked on a timer (feedback_no_polling): at start, when the person changes the find, after a removal, and when Kūaha comes to the front again.

use ferros_items::{Card, Find, Found, ORDER_LARGEST, ORDER_NEWEST, ORDER_OLDEST, ORDER_TITLE};
use vsf::schema::{SectionBuilder, SectionSchema, TypeConstraint};
use vsf::VsfType;

use crate::source::{HoldSource, KistId, KistStore, Problem, Totals, Usage};

pub type Handle = [u8; 32];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    /// The person's items.
    Things,
    /// The device's storage.
    Device,
}

/// How far back a find looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    Any,
    Day,
    Week,
    Month,
    Year,
}

impl When {
    pub const ALL: [When; 5] = [When::Any, When::Day, When::Week, When::Month, When::Year];

    /// Its span in days (calendar facts, not knobs); None for any time.
    fn days(self) -> Option<i64> {
        match self {
            When::Any => None,
            When::Day => Some(1),
            When::Week => Some(7),
            When::Month => Some(30),
            When::Year => Some(365),
        }
    }

    pub fn words(self) -> ferros_lang::Msg {
        use ferros_lang::Msg::*;
        match self {
            When::Any => KuahaAnyTime,
            When::Day => KuahaPastDay,
            When::Week => KuahaPastWeek,
            When::Month => KuahaPastMonth,
            When::Year => KuahaPastYear,
        }
    }

    fn index(self) -> usize {
        When::ALL.iter().position(|w| *w == self).unwrap_or(0)
    }
}

pub const ORDERS: [u64; 4] = [ORDER_NEWEST, ORDER_OLDEST, ORDER_TITLE, ORDER_LARGEST];

pub fn order_words(order: u64) -> ferros_lang::Msg {
    use ferros_lang::Msg::*;
    match order {
        ORDER_OLDEST => KuahaOldest,
        ORDER_TITLE => KuahaByTitle,
        ORDER_LARGEST => KuahaLargest,
        _ => KuahaNewest,
    }
}

/// The find the person built, every condition optional (STORE.md "How a person finds things").
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    /// One kind, or every kind Kūaha may open.
    pub kind: Option<[u8; 32]>,
    pub when: When,
    /// A maker's tag (ferros_items `maker_tag`), or any.
    pub maker: Option<[u8; 32]>,
    pub words: String,
    pub order: u64,
}

impl Default for Query {
    fn default() -> Query {
        Query { kind: None, when: When::Any, maker: None, words: String::new(), order: ORDER_NEWEST }
    }
}

impl Query {
    /// The hold's find for a page from `skip`, made before `now` (Eagle time).
    pub fn find(&self, now: i64, skip: u64, count: u64) -> Find {
        let osc_day = 86_400 * vsf::types::eagle_time::OSCILLATIONS_PER_SECOND as i64;
        let words = self.words.trim();
        Find {
            kinds: self.kind.into_iter().collect(),
            maker: self.maker,
            from: self.when.days().map(|d| now - d * osc_day),
            words: (!words.is_empty()).then(|| words.to_string()),
            order: self.order,
            skip,
            count,
            ..Default::default()
        }
    }

    /// Whether any condition narrows the find (an empty answer then means "nothing matches", not an empty hold).
    pub fn narrowed(&self) -> bool {
        self.kind.is_some() || self.when != When::Any || self.maker.is_some() || !self.words.trim().is_empty()
    }
}

/// A page of cards a find asks for at once (2^6): more come a page at a time from the row under them.
pub const PAGE: u64 = 1 << 6;

/// A call for the hold, queued by the model and run by the platform.
#[derive(Clone, Debug, PartialEq)]
pub enum Ask {
    /// A page of the find; `more` appends to what is shown.
    Find { generation: u64, find: Find, more: bool },
    Totals,
    Usage,
    /// One kist's own storage sized: slow (its every value's head), so one a call, after everything else.
    SizeKist(KistId),
    Remove(Handle),
}

impl Ask {
    /// Sizing a kist waits behind every other ask, so a find the person just asked for never queues behind Photon's thousands of values.
    pub fn slow(&self) -> bool {
        matches!(self, Ask::SizeKist(_))
    }
}

/// A call's answer, back to the model.
#[derive(Clone, Debug)]
pub enum Answer {
    Found { generation: u64, more: bool, result: Result<Found, Problem> },
    Totals(Result<Totals, Problem>),
    Usage(Result<Option<Usage>, Problem>),
    Sized(Result<Vec<KistStore>, Problem>),
    Removed(Handle, Result<(), Problem>),
}

/// One ask run on a source: the platform's hold thread calls this for each.
pub fn serve(source: &mut dyn HoldSource, ask: Ask) -> Answer {
    match ask {
        Ask::Find { generation, find, more } => Answer::Found { generation, more, result: source.find(&find) },
        Ask::Totals => Answer::Totals(source.totals()),
        Ask::Usage => Answer::Usage(source.usage()),
        Ask::SizeKist(id) => Answer::Sized(source.size_kists(&[id])),
        Ask::Remove(h) => Answer::Removed(h, source.remove(&h)),
    }
}

/// Something asked for and not yet in hand, in hand, or refused.
#[derive(Clone, Debug, PartialEq)]
pub enum Load<T> {
    Asking,
    Ready(T),
    Failed(Problem),
}

impl<T> Load<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Load::Ready(t) => Some(t),
            _ => None,
        }
    }
}

pub struct Model {
    pub view: View,
    pub query: Query,
    /// Moves with every change to the find; an answer under an older one is dropped.
    generation: u64,
    /// The cards shown, in the find's order, with Kūaha's handles.
    pub found: Load<Vec<(Handle, Card)>>,
    /// How many match in all (the find's `total`).
    pub total: u64,
    pub totals: Load<Totals>,
    pub usage: Load<Option<Usage>>,
    /// The card opened to its facts and actions.
    pub open: Option<Handle>,
    /// The card whose remove was tapped once: the next tap on it removes (the WiFi page's forget, armed).
    pub armed: Option<Handle>,
    /// A removal the hold refused, under that card.
    pub refused: Option<(Handle, Problem)>,
    /// Kūaha's own choices changed since the platform last kept them (`take_prefs`).
    prefs_changed: bool,
    asks: Vec<Ask>,
    /// Eagle time now, as the platform last said (a find's "past day" counts back from it).
    pub now: i64,
}

impl Model {
    pub fn new(prefs: Option<Prefs>, now: i64) -> Model {
        let p = prefs.unwrap_or_default();
        let mut m = Model {
            view: p.view,
            query: p.query,
            generation: 0,
            found: Load::Asking,
            total: 0,
            totals: Load::Asking,
            usage: Load::Asking,
            open: None,
            armed: None,
            refused: None,
            prefs_changed: false,
            asks: Vec::new(),
            now,
        };
        m.refresh();
        m
    }

    /// Everything asked again: at start, and when Kūaha comes to the front (its view of the hold may be old; nothing watches it yet, STORE.md op 7).
    pub fn refresh(&mut self) {
        self.ask_find();
        self.asks.push(Ask::Totals);
        self.asks.push(Ask::Usage);
    }

    fn ask_find(&mut self) {
        self.generation += 1;
        self.open = None;
        self.armed = None;
        self.refused = None;
        let find = self.query.find(self.now, 0, PAGE);
        self.asks.retain(|a| !matches!(a, Ask::Find { .. }));
        self.asks.push(Ask::Find { generation: self.generation, find, more: false });
    }

    /// The asks queued since the last call, for the platform to run in order: the quick ones first, the kists' sizes last.
    pub fn take_asks(&mut self) -> Vec<Ask> {
        let mut asks = core::mem::take(&mut self.asks);
        asks.sort_by_key(|a| a.slow());
        asks
    }

    pub fn answer(&mut self, a: Answer) {
        match a {
            Answer::Found { generation, more, result } => {
                if generation != self.generation {
                    return;
                }
                match result {
                    Ok(f) => {
                        self.total = f.total;
                        match (&mut self.found, more) {
                            (Load::Ready(shown), true) => shown.extend(f.items),
                            (found, _) => *found = Load::Ready(f.items),
                        }
                    }
                    Err(p) => self.found = Load::Failed(p),
                }
            }
            Answer::Totals(r) => {
                self.totals = match r {
                    Ok(t) => Load::Ready(t),
                    Err(p) => Load::Failed(p),
                }
            }
            Answer::Usage(r) => {
                // Sizes already in hand stay on their kists while each is sized again, so a refresh does not blank the rows.
                let before: Vec<KistStore> = self.usage.ready().and_then(|u| u.as_ref()).map(|u| u.kists.clone()).unwrap_or_default();
                self.usage = match r {
                    Ok(Some(mut u)) => {
                        for k in u.kists.iter_mut().filter(|k| k.bytes.is_none()) {
                            if let Some(b) = before.iter().find(|b| b.developer == k.developer && b.option == k.option) {
                                (k.values, k.bytes) = (b.values, b.bytes);
                            }
                            self.asks.push(Ask::SizeKist((k.developer, k.option)));
                        }
                        Load::Ready(Some(u))
                    }
                    Ok(None) => Load::Ready(None),
                    Err(p) => Load::Failed(p),
                }
            }
            Answer::Sized(r) => {
                // A kist that could not be sized keeps its row with no size; nothing else is lost.
                if let (Ok(sized), Load::Ready(Some(u))) = (r, &mut self.usage) {
                    for s in sized {
                        if let Some(k) = u.kists.iter_mut().find(|k| k.developer == s.developer && k.option == s.option) {
                            *k = s;
                        }
                    }
                }
            }
            Answer::Removed(h, r) => match r {
                Ok(()) => {
                    if let Load::Ready(shown) = &mut self.found {
                        let before = shown.len();
                        shown.retain(|(x, _)| *x != h);
                        // WHY saturating: the total is the hold's count from the last find; a removal it no longer counted (two Kūaha windows, a lock) must not wrap it.
                        self.total = self.total.saturating_sub((before - shown.len()) as u64);
                    }
                    if self.open == Some(h) {
                        self.open = None;
                    }
                    self.armed = None;
                    self.asks.push(Ask::Totals);
                    self.asks.push(Ask::Usage);
                }
                Err(p) => {
                    self.armed = None;
                    self.refused = Some((h, p));
                }
            },
        }
    }

    /// The next page of the find, after what is shown.
    pub fn more(&mut self) {
        let shown = self.found.ready().map_or(0, |v| v.len() as u64);
        if shown < self.total {
            let find = self.query.find(self.now, shown, PAGE);
            self.asks.push(Ask::Find { generation: self.generation, find, more: true });
        }
    }

    fn changed(&mut self) {
        self.prefs_changed = true;
        self.ask_find();
    }

    pub fn set_view(&mut self, v: View) {
        if self.view != v {
            self.view = v;
            self.prefs_changed = true;
            self.open = None;
            self.armed = None;
        }
    }

    /// A kind's circle: narrows to it, a second tap widens again (STORE.md §4: "A tap narrows; a second tap widens again").
    pub fn tap_kind(&mut self, kind: Option<[u8; 32]>) {
        self.query.kind = if self.query.kind == kind { None } else { kind };
        self.changed();
    }

    pub fn next_when(&mut self) {
        self.query.when = When::ALL[(self.query.when.index() + 1) % When::ALL.len()];
        self.changed();
    }

    pub fn next_order(&mut self) {
        let i = ORDERS.iter().position(|o| *o == self.query.order).unwrap_or(0);
        self.query.order = ORDERS[(i + 1) % ORDERS.len()];
        self.changed();
    }

    /// The maker condition steps thru any maker, then each kist that made something (from the totals), in name order.
    pub fn next_maker(&mut self) {
        let makers: Vec<[u8; 32]> = self.totals.ready().map(|t| t.makers.iter().map(|m| m.tag).collect()).unwrap_or_default();
        self.query.maker = match self.query.maker {
            None => makers.first().copied(),
            Some(m) => makers.iter().position(|x| *x == m).and_then(|i| makers.get(i + 1)).copied(),
        };
        self.changed();
    }

    /// Words typed into the search line: the find follows every change.
    pub fn type_words(&mut self, s: &str) {
        self.query.words.push_str(s);
        self.changed();
    }

    pub fn delete_word_char(&mut self) {
        if self.query.words.pop().is_some() {
            self.changed();
        }
    }

    /// A card tapped: opened to its facts and actions, or closed again.
    pub fn tap_item(&mut self, h: Handle) {
        self.open = if self.open == Some(h) { None } else { Some(h) };
        self.armed = None;
        self.refused = None;
    }

    /// A card's remove tapped: armed, then removed by the second tap (two taps, as the WiFi page forgets a network).
    pub fn tap_remove(&mut self, h: Handle) {
        self.refused = None;
        if self.armed == Some(h) {
            self.asks.push(Ask::Remove(h));
        } else {
            self.armed = Some(h);
        }
    }

    /// A tap anywhere but an armed remove disarms it.
    pub fn disarm(&mut self) {
        self.armed = None;
    }

    /// The card of an item shown, by its handle.
    pub fn card(&self, h: &Handle) -> Option<&Card> {
        self.found.ready()?.iter().find(|(x, _)| x == h).map(|(_, c)| c)
    }

    /// Kūaha's choices, when they changed since last taken: the platform keeps them at once (AGENT.md "Persistence Rule").
    pub fn take_prefs(&mut self) -> Option<Prefs> {
        if !self.prefs_changed {
            return None;
        }
        self.prefs_changed = false;
        Some(Prefs { view: self.view, query: self.query.clone() })
    }
}

/// What Kūaha keeps of its own, in its own domain: the view it was on and the find the person built.
#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    pub view: View,
    pub query: Query,
}

impl Default for Prefs {
    fn default() -> Prefs {
        Prefs { view: View::Things, query: Query::default() }
    }
}

/// Its name in the kist's domain.
pub const PREFS_NAME: &str = "kuaha";

fn prefs_schema() -> SectionSchema {
    SectionSchema::new("kuaha")
        .field("view", TypeConstraint::AnyUnsigned)
        .field("kind", TypeConstraint::Idea)
        .field("when", TypeConstraint::AnyUnsigned)
        .field("maker", TypeConstraint::Blake3Rolling)
        .field("words", TypeConstraint::Utf8Text)
        .field("order", TypeConstraint::AnyUnsigned)
}

fn unsigned(v: &VsfType) -> Option<u64> {
    match v {
        VsfType::u(n, _) => Some(*n as u64),
        VsfType::u3(n) => Some(*n as u64),
        VsfType::u4(n) => Some(*n as u64),
        VsfType::u5(n) => Some(*n as u64),
        VsfType::u6(n) => Some(*n),
        _ => None,
    }
}

fn h32(v: &VsfType) -> Option<[u8; 32]> {
    match v {
        VsfType::hi(b) | VsfType::hb(b) => b.as_slice().try_into().ok(),
        _ => None,
    }
}

impl Prefs {
    /// A complete VSF document stamped `now` (Eagle time), with its provenance hash.
    pub fn build(&self, now: i64) -> Result<Vec<u8>, String> {
        let e = |e: vsf::schema::ValidationError| format!("{e:?}");
        let mut b = prefs_schema().build();
        b = b.set("view", VsfType::u((self.view == View::Device) as usize, false)).map_err(e)?;
        if let Some(k) = self.query.kind {
            b = b.set("kind", VsfType::hi(k.to_vec())).map_err(e)?;
        }
        b = b.set("when", VsfType::u(self.query.when.index(), false)).map_err(e)?;
        if let Some(m) = self.query.maker {
            b = b.set("maker", VsfType::hb(m.to_vec())).map_err(e)?;
        }
        if !self.query.words.is_empty() {
            b = b.set("words", VsfType::x(self.query.words.clone())).map_err(e)?;
        }
        b = b.set("order", VsfType::u(self.query.order as usize, false)).map_err(e)?;
        let section = b.encode().map_err(e)?;
        vsf::VsfBuilder::new().creation_time_oscillations(now).provenance_only().add_unboxed("kuaha", section).build()
    }

    /// Read back, checked like any input (the document came from the vault); a field that does not read is its default.
    pub fn parse(doc: &[u8]) -> Result<Prefs, String> {
        let sections = ferros_items::sections(doc, "kuaha").map_err(|e| format!("{e:?}"))?;
        let s = SectionBuilder::parse(prefs_schema(), sections.first().ok_or("no kuaha section")?).map_err(|e| format!("{e:?}"))?;
        let one = |name: &str| s.get_fields(name).first().and_then(|f| f.values.first()).cloned();
        let mut p = Prefs::default();
        p.view = if one("view").as_ref().and_then(unsigned) == Some(1) { View::Device } else { View::Things };
        p.query.kind = one("kind").as_ref().and_then(h32);
        // WHY .get: the index is the vault's word; one this build has no time window for reads as any time.
        p.query.when = one("when").as_ref().and_then(unsigned).and_then(|i| When::ALL.get(i as usize).copied()).unwrap_or(When::Any);
        p.query.maker = one("maker").as_ref().and_then(h32);
        p.query.words = match one("words") {
            Some(VsfType::x(t)) => t,
            _ => String::new(),
        };
        p.query.order = one("order").as_ref().and_then(unsigned).filter(|o| ORDERS.contains(o)).unwrap_or(ORDER_NEWEST);
        Ok(p)
    }
}
