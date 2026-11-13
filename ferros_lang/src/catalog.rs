//! The catalog: every message the shell draws (`Msg`, words alone; `Say`, words with holes), and each language's one exhaustive match over them (catalog/en.rs is the reference; the rest are its translations).
//!
//! Where Photon says the same thing, a language here uses Photon's words for it (photon src/ui/lang/*.rs), in the shell's lower case where the shell writes lower case.

use crate::Lang;
use core::fmt::{self, Write};

mod de;
mod en;
mod es;
mod fil;
mod fr;
mod hi;
mod id;
mod it;
mod mi;
mod pl;
mod pt;
mod ru;
mod sw;
// Turkish's code is `tr`; the module is named for it like every other language here.
mod tr;
mod uk;
mod vi;

/// The words-alone messages, declared once: the enum and `Msg::ALL` (every message, for the tests that walk them) come from the same list, so neither can miss one.
macro_rules! messages {
    ($($(#[$m:meta])* $v:ident,)*) => {
        /// A message that is words alone. Grouped by where it shows; each group's comment says the screen.
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        pub enum Msg {
            $($(#[$m])* $v,)*
        }
        impl Msg {
            /// Every message, in declaration order.
            pub const ALL: &'static [Msg] = &[$(Msg::$v,)*];
        }
    };
}

messages! {
    // ---- the map: the settings circle and its pages' titles (payloads/map PAGES; WiFi, Bluetooth, NFC, GPS and USB are names and stay) ----
    /// The settings circle's name under it, and a link page's "settings" heading over its knobs.
    Settings,
    PageAppearance,
    /// The number base's page; also the clock page's fact naming the base in use.
    PageUnits,
    PageKeyboard,
    PageSecurity,
    /// The page listing everything on the deck as a circle, by kind (owner, 2026-11-12: "I want to make sure it says items in settings, not kists"): kists, pucks, later contacts; each row opens its card.
    PageDeck,
    PagePermissions,
    PageFleet,
    PagePower,
    PageConnections,
    /// The cellular radio's page (and its row on Connections).
    PageCellular,
    PageClock,
    PageLanguage,
    PageAbout,
    PageDeveloper,

    // ---- a link's state (Connections, a link's page) ----
    /// Switched off by its status circle.
    SwitchedOff,
    /// Also: a value knob's "off", the light scope's "off".
    Off,
    /// Also: a link's switch, a knob's "on".
    On,
    OnNoLink,
    /// Also the attest screen's network row: the kept network is joined.
    Connected,
    /// A link's state: the name of the fact whose value is one of the above.
    State,

    // ---- a link's measures: each one's name (a link's page, the name left of its value) ----
    Technology,
    /// Also the group heading "signal".
    Signal,
    NoiseFloor,
    SignalToNoise,
    ReferencePower,
    ReferenceQuality,
    SignalToInterference,
    /// "antenna match (VSWR)": VSWR stays.
    AntennaMatch,
    TransmitPower,
    Frequency,
    ChannelWidth,
    /// Also the group heading "channel".
    Channel,
    RateDown,
    RateUp,
    RoundTrip,
    Loss,
    /// The spatial streams of a WiFi link.
    Streams,
    SatellitesSeen,
    SatellitesInFix,
    BestCarrierToNoise,
    /// "accuracy (±)".
    Accuracy,
    Registration,
    LargestPacket,
    PacketsIn,
    PacketsOut,
    BadPacketsIn,
    FailedPacketsOut,
    DroppedIn,
    RefusedOut,
    /// How often the link went to sleep between packets.
    LinkNaps,
    PowerSave,
    LinkPower,
    Altitude,
    Speed,
    Heading,
    /// A measure of a kind this page has no name for.
    Measure,
    // Registration's values.
    NotRegistered,
    /// Registered on the home network.
    RegHome,
    Searching,
    Denied,
    Roaming,
    EmergencyOnly,
    Unknown,
    // Power save's values (beside Off and On).
    Fast,
    /// WiFi power save following the screen (on while it is dark).
    WithTheScreen,
    // Link power's values.
    Working,
    LightSleep,
    Sleep,
    Suspended,
    // The measures' group headings (signal and channel are `Signal`, `Channel`).
    GroupLink,
    GroupCounters,
    GroupMotion,
    /// The heading over the satellites tracked.
    Satellites,
    /// "measures: none yet": the fact's name, then its value.
    Measures,
    NoneYet,
    NamesAndAddresses,
    // A link's names and addresses (ferros_detail): each field's name. DNS stays.
    Carrier,
    /// "data APN": APN stays.
    DataApn,
    /// "attach APN".
    AttachApn,
    Address,
    /// "address (IPv6)".
    AddressIpv6,
    /// "DNS (IPv6)".
    DnsIpv6,
    /// The network a WiFi link is on (its name).
    Network,
    AccessPoint,
    /// The device's own hardware address on that network.
    ThisDeviceOnIt,
    Router,
    /// A DHCP lease's length.
    Lease,
    Detail,
    /// A satellite system the page has no name for (GPS, GLONASS, Galileo, BeiDou, QZSS, NavIC stay).
    OtherSystem,
    // The links' knobs.
    ReRegisterNow,
    ReRegisterNote,
    Band,
    /// "any (5 GHz first)".
    BandAny,
    /// "5 GHz only".
    Band5Only,
    /// "2.4 GHz only".
    Band24Only,
    /// Two lines.
    BandNote,
    NothingToSetYet,
    // The GPS page (owner, 2026-11-10): where the device is (under `RightGps`, "where you are"), and who is using GPS now.
    /// The seven Dymaxion words (vsf WorldCoord) that name the place.
    PositionWords,
    /// "latitude, longitude", in decimal degrees.
    LatLon,
    /// "degrees, minutes, seconds".
    DegMinSec,
    NoFixYet,
    /// A GPS link's state once its driver has a fix (its state row; owner, 2026-11-10: `Searching` until then).
    HasFix,
    /// The GPS driver started for the page and not answering yet.
    StartingUp,
    /// The heading over who is using GPS now.
    UsingNow,

    // ---- the number bases (Units, the clock's "units" fact) ----
    BaseArabic,
    BaseHex,
    /// "dozenal linear (DLS)".
    BaseDls,
    /// "dozenal magnitude (DMS)".
    BaseDms,

    // ---- Appearance ----
    Brightness,
    /// The UI scale's slider.
    Scale,
    NamesUnderCircles,
    /// The themes' names, in ferros_sys::theme::THEMES order: grey, slate, ember, night.
    ThemeGrey,
    ThemeSlate,
    ThemeEmber,
    ThemeNight,

    // ---- the quick row (payloads/display, QUICK.md): each circle's word; brightness is `Brightness` ----
    /// The torch (the camera's flash held on).
    Torch,
    /// Every radio switched off at once (a tap puts back the ones that were on).
    RadiosOff,
    /// The sound mode's three: sounds play, the device buzzes instead, neither.
    SoundOn,
    Vibrate,
    Silent,
    /// The person's do not disturb (follows them across their devices).
    DoNotDisturb,
    /// A capture of the screen.
    Screencap,
    /// The app in front to the glass's edge, no circle and no bars (the quick row's middle circle).
    FullScreen,

    // ---- the torch's page (its title is `Torch`; its first slider is `Brightness`, its on and off `On`) ----
    /// The torch flashing on and off: the name of its slider, whose value is `Off` or how long each flash and its gap take together (a time).
    Strobe,
    /// How much of each flash's period the torch is lit, a share (the duty cycle).
    LitPerFlash,
    /// The torch turns itself off after this long, a time or `Never`.
    TurnsOffAfter,
    /// The timeout's "off": the torch stays lit until switched off.
    Never,

    // ---- the battery's page (the console's battery circle; its title is `Battery`) ----
    /// The state of charge.
    Charge,
    /// The cell's voltage.
    Voltage,
    /// The current in or out of the cell.
    Current,
    /// The power in or out of the cell (voltage times current).
    PowerDraw,
    /// The cell's temperature.
    Temperature,
    /// The current's direction: into the cell, or out of it.
    Charging,
    Discharging,

    // ---- Keyboard ----
    /// The keyboard layout's heading (Dvorak, QWERTY stay).
    Layout,

    // ---- Security ----
    /// The checkbox: the device attests at boot without the handle typed ("attest", as the attest screen says it; owner, 2026-11-10: "always attest").
    AttestAtBoot,
    /// The same checkbox armed by its first tap.
    AttestAtBootArmed,
    /// Two lines.
    AttestAtBootNote,

    // ---- the NFC page: attest with a tag (AUTH.md "Attest with a tag"; owner, 2026-11-11) ----
    /// The heading over the tags kept to attest with.
    TagAttest,
    /// The button that keeps the next tag held to the device.
    TagAdd,
    /// While it waits for that tag (the button then cancels: `Cancel`).
    TagHold,
    /// What the last add did: kept, kept already, not kept.
    TagKeptNow,
    TagKeptAlready,
    TagNotKept,
    /// Two lines, under the list: what a kept tag does (the warning, as attest at boot's).
    TagNote,
    /// A kept tag's button.
    TagRemove,

    // ---- Fleet: ferros's own lines (the fleet's and the identity's words are fgtw's, shared with Photon: fgtw::text) ----
    FleetServiceNotRunning,
    DeckFetching,
    /// Also the checkbox's label.
    DeckSame,
    DeckOwn,
    /// Two lines: what removing this device wipes from it.
    RemoveNote,

    // ---- Power ----
    ProximityWake,
    /// Two lines.
    ProximityWakeNote,

    // ---- the clock page ----
    AtUtc,
    Time,
    NotReadYet,
    /// The zone picker's title and the button that opens it.
    ChooseAZone,
    Back,
    /// No zone: the clock keeps a fixed offset.
    NoneFixedOffset,
    Zone,
    NoZonesInKernel,
    /// The zone's standard offset (all year) and its legal one (with daylight time when in it).
    Standard,
    Legal,
    /// Where the zone in effect comes from: the fact's name, then one of the four below.
    From,
    FromNetwork,
    FromPosition,
    FromHand,
    FromOffset,
    /// Two lines: a zone found from a position is the nearest principal place's.
    NearestPlaceNote,
    /// The checkbox on the clock and language pages: this device follows the identity's choice.
    FollowMyIdentity,
    /// Under it, unchecked (the clock page).
    ZoneOwn,
    /// Under it, checked, before the identity has a zone.
    ZoneIdentityNone,
    /// Under it, checked, on the clock and language pages.
    SetHereAllFollow,
    Automatic,
    ByHand,
    /// Two lines, under automatic: nothing found yet.
    ZoneAutoNothingFound,
    /// Two lines, under automatic: a zone was found.
    ZoneAutoStandsIn,
    QuarterHourEast,
    QuarterHourWest,
    Sources,
    ThePmicClock,
    /// When the clock is read.
    Read,
    AtStartThenHourly,
    NetworkZone,
    NotHeardYet,
    NetworkTime,
    GpsTime,
    NotUsedYet,
    // The page's notes after a choice.
    ServiceDidNotAnswer,
    FleetNotAnsweredYet,
    NotAZone,
    KernelDidNotTake,
    NoZonesYet,

    // ---- the language page ----
    /// Under "follow my identity", unchecked.
    LangOwn,
    /// Under it, checked, before the identity has a language.
    LangIdentityNone,
    /// Under the list: the languages the face cannot draw yet (`Lang::renders`).
    MoreLanguagesNeedFace,
    NotALanguage,

    // ---- About ----
    /// "version Zil, development build": Zil (the dozenal version name) stays.
    VersionDev,
    /// "version Zil".
    Version,
    DeveloperAlready,
    NotRead,
    Battery,
    /// The time from the power key's press to the attest screen.
    PressToReady,
    SincePowerOn,
    Processes,
    InTheVault,
    /// The chip's temperature.
    Chip,
    /// The bootloader's lock state (ferros_sys::BootLock; FLASHER.md step 7; owner, 2026-11-13: shown, never a nag). One sentence the page wraps: unlocked, what it exposes and what it does not.
    BootloaderUnlocked,
    /// One sentence the page wraps: unlocked while attest at boot is on, so the kept seed (the device class) is exposed too.
    BootloaderUnlockedAttest,
    /// Locked to the custom key, the one this ferros is signed with (verified boot yellow).
    BootloaderLockedFerros,
    /// Locked to the maker's key (verified boot green).
    BootloaderLockedMaker,
    /// No word from the bootloader this boot (a reload).
    BootloaderNotRead,

    // ---- Kists, Permissions, a kist's first launch ----
    NoKistsStowed,
    AsksForNothing,
    NewKistsStartOn,
    /// The first-launch question's line over the wants.
    AsksForOnce,
    /// The first-launch question's button.
    Start,
    /// A live note's page (docs/NOTIFY.md "The live takeover"): take it (open the kist where the note points), or say no (the note goes; its kist tells the caller).
    Answer,
    Decline,
    /// A live note's heading when its kist gave no words: something for the person, now.
    ForYouNow,
    /// What each right is, in ferros_kist::RIGHTS order: display, touch, surface, ira, domain, entropy, gps, udp, audio.
    RightDisplay,
    RightTouch,
    RightSurface,
    RightIra,
    RightDomain,
    RightEntropy,
    RightGps,
    RightUdp,
    RightAudio,
    /// The cameras (KISTS.md, the asked rights: frames from the camera driver; built 2026-11-12 for Lumis).
    RightCamera,
    /// Why a kist the person tapped did not start (ferros_kist::launch::why), under `Say::CouldNotStart` on its open circle: no room in the memory pool.
    NotEnoughMemory,
    /// Not stowed on this device: no capsule here, or no seal.
    NotStowedHere,
    /// Its checks refused it: its signature, the gate, a right it does not declare.
    FailedItsChecks,
    /// Its capsule could not be read from the vault.
    CouldNotBeRead,
    /// Anything else.
    SomethingWentWrong,
    /// Why a kist that started then stopped, under `Say::Stopped`: a fault or a panic.
    ItCrashed,
    /// Ended from outside (the bridge's `diag kill`), not by the person's own relaunch.
    ClosedFromOutside,
    /// Stowing a kist (owner, 2026-11-11: "the proper term for installing a kist is stow"; docs/KISTS.md "Stowing"): the Kists page's heading over a kist offered over USB, checked and waiting on the person.
    OfferedOverUsb,
    /// Its button that puts it in the person's list (a verb: put away, store aboard).
    Stow,
    /// Its end: in the person's list.
    Stowed,
    /// Its end refused (the reason after it).
    NotStowed,
    /// An offer of native, unchecked machine code, which only a development build stows (SANDBOX.md), said plainly on the page.
    UncheckedDevOnly,
    /// Over an offer's wants: each is asked once, at its first start.
    AsksAtFirstStart,
    /// The stower's stages on the Deck page (payloads/kists `STAGE_*`; until 2026-11-12 English alone): a job queued behind another.
    WaitingItsTurn,
    /// Its signature, every egg against its developer's bundle.
    CheckingSignature,
    /// The kernel reading it back and running the checker over its code.
    CheckingCode,
    /// Its seal, keyed to this device.
    SealingToDevice,
    /// Into the person's list.
    StoringIt,
    /// An unstow's end (a stow's is `Stowed`).
    Unstowed,
    /// An unstow refused (the reason after it).
    NotUnstowed,
    /// A restore's end: a capsule the person's list names, brought back to this device.
    Restored,
    /// A restore refused (the reason after it).
    NotRestored,

    // ---- Deck: what is on deck, and each thing's card (owner, 2026-11-12) ----
    /// A row's kind: a kist (opened, an app; a ferros word, kept unless a language has a reason).
    KindKist,
    /// A row's kind: a puck (a live circle never opened, docs/PUCKS.md; a ferros word, never "widget").
    KindPuck,
    /// The heading over the pandok's kists on the Deck (the pandok: the place with the things, the kist catalog; a ferros name, kept).
    Pandok,
    /// The Deck with nothing on deck but settings.
    NothingStowed,
    /// A card's fact: who made it (the developer's name, else their key's first bytes).
    MadeBy,
    /// A card's fact: its version (`Version` is About's line).
    FactVersion,
    /// A card's fact: its capsule's size.
    Size,
    /// A card's fact: its identity, the developer's key and its option, shortened.
    Identity,
    /// A kist's state on its row and card: a process of it alive.
    Running,
    /// Its last process ended by a fault, a panic or a kill from outside, or its last launch did not start (the reason under it).
    Stopped,
    /// Stowed and neither: not launched since boot, or ended by its own exit or the person's stop.
    NotRunning,
    /// A card's fact: where it keeps its data.
    Keeps,
    /// Its value: the kist's own vault domain.
    OwnDomain,
    /// A card's button: a running kist ended, one shot dead.
    Stop,
    /// A card's and the pandok's button: a newer version from the pandok.
    Update,
    /// A kist's card: out of the person's list (two taps), the opposite of stow.
    Unstow,
    /// Under a puck's armed remove: its answers stay in the person's list.
    AnswersKept,
    // The unstow sheet, in a kist's card's place (docs/KISTS.md "The unstow sheet"): a tick keeps a kind of thing, unticked it is erased at once.
    /// Its heading over the ticks.
    UnstowWhatToKeep,
    /// A tick: the kist's own vault domain (caches, downloaded areas, its settings).
    KeptForItself,
    /// Under it while its size is not known.
    UselessWithoutIt,
    /// In its size's place when it kept nothing.
    NothingYet,
    /// A tick: the kist's items in the person's hold (docs/STORE.md).
    ThingsMadeForYou,
    /// Under it.
    ThingsMadeYours,
    /// A tick: the person's answers to its rights.
    YourAnswers,
    /// Under it, ticked.
    AnswersAskNothing,
    /// Under it, unticked.
    AnswersAskAgain,
    /// Under it, fixed, when it goes from this device only.
    AnswersOtherDevices,
    /// Its notes, always cleared.
    NotesGoWithIt,
    /// The heading over where it goes from.
    UnstowFrom,
    AllYourDevices,
    ThisDeviceOnly,
    /// Over the sheet's Unstow: nothing unticked comes back.
    GoneMeansGone,
    /// An unstow's stage while its own data is erased.
    ClearingWhatItKept,
    /// A puck's card: off the deck (two taps; an unstow underneath).
    RemoveFromDeck,
    TapAgainRemove,
    /// A puck's card: the fact naming what its circle shows.
    Shows,
    /// The clock puck's: the time.
    TheTime,

    // ---- Developer ----
    TemporalDither,
    LightScope,
    /// The light scope's channels (IR stays; "off" is `Off`); red, green and blue name the panel's channels too.
    ScopeRed,
    ScopeVisible,
    ScopeBlue,
    ScopeGreen,
    ScopeClear,
    /// The display's hit map shown in every fourth frame (development builds).
    HitMap,
    /// The heading over the panel's red, green and blue checkboxes (development builds).
    PanelChannels,
    /// The display's release look (no orange) for the rest of this boot (development builds).
    ReleaseLook,
    /// Under the panel's channels and the release look: nothing there is kept past a reboot.
    UntilReboot,

    // ---- the attest screen (payloads/ahau): its button ----
    /// The button that attests the typed handle (owner, 2026-11-10: "needs to say attest, not unlock. always attest").
    Attest,
    Mint,
    StartOver,
    Cancel,
    /// The button while a network is being joined.
    JoiningButton,
    Join,
    // Its prompts (owner, 2026-10-31: simple and direct; sentences, capitalised).
    TypeYourHandle,
    /// Two lines.
    UnboundTypeToBind,
    WaitingConnectivity,
    FindingIdentity,
    /// Three lines.
    MintPrompt,
    NoAnswerYet,
    /// Two lines.
    RecordMismatch,
    Minting,
    MintingFailed,
    /// A tag held to the attest screen that does not open the device: one refusal, no detail (AUTH.md "Attest with a tag").
    TagDenied,
    ComputingKeys,
    IdentityServiceNotAnswering,
    // Its network page.
    NotSeenLastScan,
    Wpa2Personal,
    OpenNotJoined,
    Wpa3NotJoined,
    KeptOnDevice,
    /// Beside the network the device keeps, when it is not joined (short: it sits at a row's right).
    KeptTag,
    WifiStarting,
    LookingForNetworks,
    NoNetworksFound,
    NoNetworkMatches,
    TypeToNarrow,
    WifiNotAnswering,
    AlreadyOnNetwork,
    CouldNotKeepNetwork,

    // ---- the honeycomb keyboard's bar (attest screen, hive): short, they sit on keys ----
    KeyMap,
    KeyShift,
    KeySpace,
    KeyDel,
    KeyEnter,
    /// The key that turns to the next page of symbols.
    KeyPage,
    /// The attest screen's left key: clears what is typed.
    KeyClear,
    /// The system keyboard's left key (over an app): puts the keyboard away.
    KeyHide,
    /// The hive's own line over its keyboard (payloads/hive).
    HiveHint,
}

/// A message with holes: names (a kist's, a device's, a network's: never translated), words typed (kept as typed), and numbers (in the person's base: a count is written by `num`, a value the caller formats arrives as a `&str` already written in it).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Say<'a> {
    // ---- a link's page ----
    /// Where a satellite stands and how strong it is: its elevation and azimuth (degrees) and carrier to noise (dB-Hz), each written already in the person's base; `used` whether the fix uses it.
    SatPlace { up: &'a str, round: &'a str, cn0: &'a str, used: bool },
    /// The GPS page before a fix (2026-11-11): what the chip has sent so far, its frames and how many kinds of them (payloads/gps op 4), and that satellites come with its first fix.
    ChipHeard { messages: u64, kinds: u64 },
    // ---- the clock page ----
    /// The legal offset (written already: "UTC + 1 h") while daylight time is in it.
    DaylightTime(&'a str),
    /// The network's mobile country code heard (written already in the person's base).
    HeardMcc(&'a str),
    /// The same, with the country it names (ISO 3166 alpha-2, stays).
    HeardMccIn { mcc: &'a str, country: &'a str },
    // ---- About ----
    /// Taps left on the version line until Developer is a page.
    TapsToDeveloper(u64),
    /// The battery's level (written already) while it charges.
    Charging(&'a str),
    /// The battery's level (written already) while the device runs on it.
    OnBattery(&'a str),
    /// The chip's temperature (written already, with its unit) while its clock is held down for heat.
    Throttled(&'a str),
    // ---- Kists, Permissions ----
    /// A right (or a kist's name) whose question was never asked: "the screen (not asked yet)".
    NotAskedYet(&'a str),
    /// A kist using a right now (it holds a claim on it, ferros_sys::claim): "Mahere (using now)".
    UsingNow(&'a str),
    /// A kist the person tapped that did not start (its name): "Mahere could not start", the reason under it (`Msg::NotEnoughMemory` and its neighbours).
    CouldNotStart(&'a str),
    /// A kist that started and then stopped abnormally (its name): "Mahere stopped", the reason under it (`Msg::ItCrashed`, `Msg::ClosedFromOutside`).
    Stopped(&'a str),
    /// Who made a kist offered to stow (its developer's name as the capsule gives it): "by Nick".
    ByDeveloper(&'a str),
    /// The unstow sheet: what a kist kept for itself, its size written already in the person's base: "12 MiB, of no use without it".
    KeptForItself(&'a str),
    /// The stower fetching a kist (its name), the bytes so far written already in the person's base: "fetching Mahere: 3 MiB".
    Fetching { name: &'a str, got: &'a str },
    /// The same with the whole size known: "fetching Mahere: 3 MiB of 12 MiB".
    FetchingOf { name: &'a str, got: &'a str, total: &'a str },
    // ---- the attest screen ----
    /// A network's signal, its dBm written already in the person's base.
    Signal(&'a str),
    JoiningNet(&'a str),
    PassphraseFor(&'a str),
    OutOfReach(&'a str),
    SecurityUnsupported(&'a str),
    DidNotAnswer(&'a str),
    RefusedPassphrase(&'a str),
    /// A passphrase's length, in characters.
    PassphraseLength { min: u64, max: u64 },
}

/// A message in language `l`.
pub fn text(l: Lang, m: Msg) -> &'static str {
    match l {
        Lang::En => en::text(m),
        Lang::Es => es::text(m),
        Lang::Mi => mi::text(m),
        Lang::Pt => pt::text(m),
        Lang::Fr => fr::text(m),
        Lang::Id => id::text(m),
        Lang::De => de::text(m),
        Lang::Sw => sw::text(m),
        Lang::Vi => vi::text(m),
        Lang::Tr => tr::text(m),
        Lang::Ru => ru::text(m),
        Lang::Hi => hi::text(m),
        Lang::Uk => uk::text(m),
        Lang::It => it::text(m),
        Lang::Pl => pl::text(m),
        Lang::Fil => fil::text(m),
    }
}

/// A message with holes in language `l`, written into `o`, its counts in `f` (the person's base: ferros_sys::numbers).
pub fn say(l: Lang, m: Say, f: tatau::Fmt, o: &mut dyn Write) -> fmt::Result {
    match l {
        Lang::En => en::say(m, f, o),
        Lang::Es => es::say(m, f, o),
        Lang::Mi => mi::say(m, f, o),
        Lang::Pt => pt::say(m, f, o),
        Lang::Fr => fr::say(m, f, o),
        Lang::Id => id::say(m, f, o),
        Lang::De => de::say(m, f, o),
        Lang::Sw => sw::say(m, f, o),
        Lang::Vi => vi::say(m, f, o),
        Lang::Tr => tr::say(m, f, o),
        Lang::Ru => ru::say(m, f, o),
        Lang::Hi => hi::say(m, f, o),
        Lang::Uk => uk::say(m, f, o),
        Lang::It => it::say(m, f, o),
        Lang::Pl => pl::say(m, f, o),
        Lang::Fil => fil::say(m, f, o),
    }
}

/// A count in the person's base, for a language's `say` (zero, which DMS has no doublings for, as 0).
fn num(o: &mut dyn Write, f: tatau::Fmt, n: u64) -> fmt::Result {
    let mut o = o;
    if let tatau::Out::Nothing = tatau::count(&mut o, f, n, 1)? {
        o.write_str("0")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
