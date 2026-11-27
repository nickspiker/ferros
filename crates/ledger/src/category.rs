//! The categories an entry is filed under, and the class each one is sealed under (LEDGER.md "Classes").
//!
//! The category decides the class (owner, 2026-11-17): a system entry (boot steps, clocks, lock holds, process ends, anything not about the person) is sealed under a key from the ira and opens at any boot on this device; a personal entry (which kists were opened, aliases, peers, places, anything about the person) is sealed under the attested identity's key and opens only while that person is present.
//! LEDGER.md keeps the one table of every category and its class; this file is that table in code, and a test holds the two names to VSF's rules for a name.

/// Which key an entry is sealed under.
/// Its number is what the entry carries in the open (`class`), so a reader knows which key to try without opening anything.
/// The numbers are on the flash: append new classes, never renumber.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Sealed under a key derived from the device's ira: readable by this device at any boot, with the vault broken or nobody attested; unreadable off it.
    System = 0,
    /// Sealed under the attested identity's ledger key: readable only while that person is present.
    Personal = 1,
}

impl Class {
    pub fn number(self) -> u64 {
        self as u64
    }

    pub fn from_number(n: u64) -> Option<Class> {
        match n {
            0 => Some(Class::System),
            1 => Some(Class::Personal),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Class::System => "system",
            Class::Personal => "personal",
        }
    }
}

/// A category: its name (a VSF name, sealed inside the entry) and the class it is sealed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Category {
    pub name: &'static str,
    pub class: Class,
}

/// A line of the kernel's words, as `flashlog::line` has always taken it (stage 0: every existing caller).
pub const TEXT: Category = Category { name: "text", class: Class::System };
/// A line of words about the person (which kist was given an alias, and the like): sealed under the person's key.
pub const PERSON_TEXT: Category = Category { name: "person.text", class: Class::Personal };
/// The ledger's own notes: the pre-boot buffer overrun, and what the open found wrong (a torn last write, skipped).
pub const LEDGER: Category = Category { name: "ledger", class: Class::System };
/// `diag "log bench"`'s timed entries.
pub const BENCH: Category = Category { name: "bench", class: Class::System };

/// A process's end (`flashlog::death`, process.rs `end_process`): its exit value, and for a panic or a fault the message, the place, the thread and the call chain as typed fields (death.rs).
pub const PROCESS: Category = Category { name: "kernel.process", class: Class::System };
/// The kernel's own panic or early fault, found at the next boot in its record and written then (service.rs, platform.rs `record_panic`).
pub const PANIC: Category = Category { name: "kernel.panic", class: Class::System };

/// Every category this build writes.
/// A name read back that is not here is shown as it is: a newer writer's category is still a category.
pub const ALL: [Category; 6] = [TEXT, PERSON_TEXT, LEDGER, BENCH, PROCESS, PANIC];

/// The category of a name read back, if this build knows it.
pub fn find(name: &str) -> Option<Category> {
    ALL.iter().copied().find(|c| c.name == name)
}
