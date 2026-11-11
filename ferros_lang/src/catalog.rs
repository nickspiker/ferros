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
    /// The installed kists' page (kist: the capsule as the device checked, compiled and sealed it; a ferros word, kept as "kist" unless a language has a reason).
    PageKists,
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

    // ---- Fleet: ferros's own lines (the fleet's and the identity's words are fgtw's, shared with Photon: fgtw::text) ----
    FleetServiceNotRunning,
    HomeFetching,
    /// Also the checkbox's label.
    HomeSame,
    HomeOwn,
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

    // ---- Kists, Permissions, a kist's first launch ----
    NoKistsInstalled,
    AsksForNothing,
    NewKistsStartOn,
    /// The first-launch question's line over the wants.
    AsksForOnce,
    /// The first-launch question's button.
    Start,
    /// What each right is, in ferros_kist::RIGHTS order: display, touch, surface, ira, domain, entropy, gps, udp.
    RightDisplay,
    RightTouch,
    RightSurface,
    RightIra,
    RightDomain,
    RightEntropy,
    RightGps,
    RightUdp,

    // ---- Developer ----
    TemporalDither,
    LightScope,
    /// The light scope's channels (IR stays; "off" is `Off`).
    ScopeRed,
    ScopeVisible,
    ScopeBlue,
    ScopeGreen,
    ScopeClear,

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
    /// The hive's own line over its keyboard (payloads/hive).
    HiveHint,
}

/// A message with holes: names (a kist's, a device's, a network's: never translated), words typed (kept as typed), and numbers (in the person's base: a count is written by `num`, a value the caller formats arrives as a `&str` already written in it).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Say<'a> {
    // ---- a link's page ----
    /// Where a satellite stands and how strong it is: its elevation and azimuth (degrees) and carrier to noise (dB-Hz), each written already in the person's base; `used` whether the fix uses it.
    SatPlace { up: &'a str, round: &'a str, cn0: &'a str, used: bool },
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
