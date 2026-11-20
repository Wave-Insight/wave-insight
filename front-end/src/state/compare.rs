//! Waveform comparison: the extra dumps a tab draws underneath its own rows.
//!
//! One dump is the *base* (slot 1); up to [`MAX_COMPARE`]-1 further dumps can
//! be loaded into the tab as comparison *members* (slots 2..=8). A member is not
//! a tab: it never appears in the dump strip, it has no rows of its own and it
//! is never persisted. The set lives on the tab's [`DumpEntry`](super::DumpEntry),
//! so every dump compares against its own members and closing the tab drops
//! them with it.
//!
//! What a member contributes is a *series*: the value history of the same signal
//! path, drawn on the same row in the member's colour. That is why rows stay
//! keyed by their hierarchical path — adding one signal adds it from every dump
//! that has it, and a member that lacks the signal simply draws nothing.
//!
//! **Slot numbers are stable.** A member keeps the slot it was given until it is
//! removed: removing slot 3 leaves 2, 4, 5 as they are (no renumbering), so the
//! coloured badges in the sidebar never move underneath the user. The next add
//! takes the smallest free slot.
//!
//! This module is pure data and pure decisions (no `web_sys`, no persistence), so
//! the assignment rules, the mask and the status texts are unit tested on the
//! host target.

use wave_insight_lib::waveform::{TimeRange, Timescale};

use super::types::DumpHandle;
use crate::file_load::progress::format_size;

/// Slots 1 ..= 8: slot 1 is the base dump, slots 2 ..= 8 are the comparison
/// members (at most 7 of them).
pub const MAX_COMPARE: usize = 8;

/// One colour per slot. Slot 1's entry is the palette's base colour; a row still
/// draws its *own* `setting.color` when it has no comparison overlay, so the
/// entry is only used for the base chip of the toolbar and the sidebar badges.
pub const COMPARE_COLORS: [(u8, u8, u8); MAX_COMPARE] = [
    (76, 194, 255),  // 1  base
    (255, 159, 67),  // 2
    (126, 217, 87),  // 3
    (255, 99, 132),  // 4
    (191, 128, 255), // 5
    (255, 205, 86),  // 6
    (0, 214, 199),   // 7
    (233, 30, 99),   // 8
];

/// The message the toolbar shows where comparison is not available at all.
pub const NATIVE_REFUSAL: &str = "native (read-only, mmap) mode: comparison is not available yet — reopen the dump through the wasm path";

/// The colour a slot draws in; slots outside `1..=8` fall back to the base.
pub fn color_of(slot: u8) -> (u8, u8, u8) {
    let index = usize::from(slot).saturating_sub(1);
    COMPARE_COLORS
        .get(index)
        .copied()
        .unwrap_or(COMPARE_COLORS[0])
}

/// The bit a slot contributes to a [`SignalRow`](crate::nav::SignalRow) mask;
/// `0` for a slot that does not exist.
pub fn slot_bit(slot: u8) -> u8 {
    if (1..=MAX_COMPARE as u8).contains(&slot) {
        1 << (slot - 1)
    } else {
        0
    }
}

/// `rgb(255,159,67)`, the form the canvas and the css custom property take.
pub fn rgb(color: (u8, u8, u8)) -> String {
    format!("rgb({},{},{})", color.0, color.1, color.2)
}

/// The slots a mask names, as the parenthesised tail of a status line:
/// `waveform 1`, `waveforms 1, 2`.
pub fn waveforms_label(mask: u8) -> String {
    let slots: Vec<String> = (1..=MAX_COMPARE as u8)
        .filter(|slot| mask & slot_bit(*slot) != 0)
        .map(|slot| slot.to_string())
        .collect();
    match slots.as_slice() {
        [] => "no waveform".to_string(),
        [one] => format!("waveform {one}"),
        _ => format!("waveforms {}", slots.join(", ")),
    }
}

/// `compare 2: run2.vcd (12.4 MB, range 0..250000)`; the parts the loader did
/// not know are left out.
pub fn loaded_status(slot: u8, name: &str, bytes: Option<u64>, range: Option<TimeRange>) -> String {
    let mut detail: Vec<String> = Vec::new();
    if let Some(bytes) = bytes {
        detail.push(format_size(bytes));
    }
    detail.push(match range {
        Some(range) => format!("range {}..{}", range.start, range.end),
        None => "range unknown".to_string(),
    });
    format!("compare {slot}: {name} ({})", detail.join(", "))
}

/// `compare 2 removed`.
pub fn removed_status(slot: u8) -> String {
    format!("compare {slot} removed")
}

/// Say it once when the member's timescale differs from the base's: the two
/// traces share the base's time axis, so they are aligned by tick, not by
/// seconds.
pub fn timescale_note(slot: u8, member: Timescale, base: Timescale) -> Option<String> {
    (member != base).then(|| {
        format!(
            "compare {slot}: timescale {member} differs from {base} — traces are aligned by tick"
        )
    })
}

/// One loaded comparison dump.
#[derive(Clone, Debug)]
pub struct CompareMember {
    /// `2 ..= MAX_COMPARE`; never 1 (that is the base dump).
    pub slot: u8,
    /// The colour the member's series draw in (see [`COMPARE_COLORS`]).
    pub color: (u8, u8, u8),
    /// The file name the loader read (the tab strip never sees it).
    pub name: String,
    /// File size, when the transport reported one.
    pub bytes: Option<u64>,
    /// The parsed dump: the only copy. Members are not tabs, so nothing else
    /// owns their handle.
    pub handle: DumpHandle,
    /// The member's own time range. It is *shown*, not enforced: the trace stops
    /// at its end, it does not stretch the shared axis.
    pub time_range: Option<TimeRange>,
}

/// The comparison set of one tab: the base dump is implicit (slot 1).
///
/// Compared by [`CompareSet::version`], so a slice subscription compares one
/// integer and a pointer move that touches nothing cannot re-render the shell.
#[derive(Clone, Debug, Default)]
pub struct CompareSet {
    pub members: Vec<CompareMember>,
    /// Bumped on every change; what subscribers compare.
    pub version: u64,
}

impl PartialEq for CompareSet {
    fn eq(&self, other: &Self) -> bool {
        self.version == other.version
    }
}

impl CompareSet {
    /// The smallest free slot in `2 ..= MAX_COMPARE`, or the refusal.
    ///
    /// Slots are not renumbered on removal, so the free slot is the first gap:
    /// removing slot 2 of three makes the next add take 2 again.
    pub fn free_slot(&self) -> Result<u8, String> {
        (2..=MAX_COMPARE as u8)
            .find(|slot| self.members.iter().all(|member| member.slot != *slot))
            .ok_or_else(|| "at most 7 comparison waveforms".to_string())
    }

    /// Load one comparison dump: it takes the smallest free slot and the slot's
    /// colour. `Err` is the status line text when the set is full.
    pub fn add(
        &mut self,
        name: impl Into<String>,
        bytes: Option<u64>,
        handle: DumpHandle,
        time_range: Option<TimeRange>,
    ) -> Result<u8, String> {
        let slot = self.free_slot()?;
        self.members.push(CompareMember {
            slot,
            color: color_of(slot),
            name: name.into(),
            bytes,
            handle,
            time_range,
        });
        self.members.sort_by_key(|member| member.slot);
        self.version = self.version.wrapping_add(1);
        Ok(slot)
    }

    /// Drop the member in `slot`; the others keep their numbers and colours.
    /// Returns the member that was removed, if any.
    pub fn remove(&mut self, slot: u8) -> Option<CompareMember> {
        let index = self.members.iter().position(|member| member.slot == slot)?;
        let removed = self.members.remove(index);
        self.version = self.version.wrapping_add(1);
        Some(removed)
    }

    /// Forget every member (the tab was reloaded into another dump).
    pub fn clear(&mut self) {
        if !self.members.is_empty() {
            self.members.clear();
            self.version = self.version.wrapping_add(1);
        }
    }

    /// A [`SignalRow`](crate::nav::SignalRow) mask for `path`: bit 0 is the base
    /// dump (always set, the row came from it) and bit `slot - 1` is set for
    /// every member whose dump has the signal.
    ///
    /// One path walk per member, which is the documented cost: the sidebar only
    /// looks its ~40 rendered rows up, and the tree build does one lookup per
    /// signal while a comparison is loaded.
    pub fn bit(&self, path: &str) -> u8 {
        let mut mask = 1u8;
        for member in &self.members {
            if member.handle.waveform().signal_by_path(path).is_some() {
                mask |= slot_bit(member.slot);
            }
        }
        mask
    }

    /// The members, in slot order.
    pub fn members(&self) -> impl Iterator<Item = &CompareMember> {
        self.members.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.members.len() >= MAX_COMPARE - 1
    }
}

/// A member's open failure, worded for the status line (it names the file).
pub fn open_error(name: &str, error: &str) -> String {
    format!("{name}: the comparison waveform could not be opened ({error}) — check that it is an intact VCD/FST/GHW dump")
}

#[cfg(test)]
mod tests {
    use super::*;
    use wave_insight_lib::waveform::Waveform;

    fn handle_of_bytes(bytes: Vec<u8>) -> DumpHandle {
        DumpHandle::new(Waveform::open_bytes(bytes).expect("the test dump opens"))
    }

    /// The QA fixture: `top.clk` (1 bit) and `top.pc` (8 bits).
    fn wave_vcd() -> DumpHandle {
        handle_of_bytes(include_bytes!("../../../qa/fixtures/wave.vcd").to_vec())
    }

    /// A second dump with `top.clk` and one signal the fixture does not have.
    fn other_vcd() -> DumpHandle {
        let text = "$timescale 1ns $end\n\
                    $scope module top $end\n\
                    $var wire 1 ! clk $end\n\
                    $var wire 4 # only_here [3:0] $end\n\
                    $upscope $end\n\
                    $enddefinitions $end\n\
                    $dumpvars\n0!\nb0 #\n$end\n#10\n1!\n#20\nb1010 #\n";
        handle_of_bytes(text.as_bytes().to_vec())
    }

    #[test]
    fn the_first_member_takes_slot_two_in_its_colour() {
        let mut set = CompareSet::default();
        let slot = set
            .add("run2.vcd", Some(12), wave_vcd(), None)
            .expect("the set is empty");
        assert_eq!(slot, 2);
        assert_eq!(set.members.len(), 1);
        assert_eq!(set.members[0].slot, 2);
        assert_eq!(set.members[0].color, COMPARE_COLORS[1]);
        assert_eq!(set.members[0].name, "run2.vcd");
        assert_eq!(set.members[0].bytes, Some(12));
        // every change bumps the version the subscribers compare
        let version = set.version;
        set.add("run3.vcd", None, wave_vcd(), None)
            .expect("room for a second member");
        assert_ne!(set.version, version);
    }

    #[test]
    fn the_set_refuses_more_than_seven_members() {
        let mut set = CompareSet::default();
        for index in 0..MAX_COMPARE - 1 {
            let slot = set
                .add(format!("run{index}.vcd"), None, wave_vcd(), None)
                .expect("the cap allows seven");
            assert_eq!(usize::from(slot), index + 2);
        }
        assert!(set.is_full());
        let error = set
            .add("eighth.vcd", None, wave_vcd(), None)
            .expect_err("the eighth member is refused");
        assert_eq!(error, "at most 7 comparison waveforms");
        assert_eq!(set.members.len(), MAX_COMPARE - 1);
    }

    #[test]
    fn colour_follows_the_slot_and_falls_back_to_the_base() {
        assert_eq!(color_of(1), COMPARE_COLORS[0]);
        assert_eq!(color_of(2), (255, 159, 67));
        assert_eq!(color_of(8), (233, 30, 99));
        // a slot outside the palette is not a colour of its own
        assert_eq!(color_of(0), COMPARE_COLORS[0]);
        assert_eq!(color_of(9), COMPARE_COLORS[0]);
    }

    #[test]
    fn removing_a_member_keeps_the_other_slots_and_colours() {
        let mut set = CompareSet::default();
        for name in ["a.vcd", "b.vcd", "c.vcd"] {
            set.add(name, None, wave_vcd(), None).expect("room");
        }
        let colours: Vec<(u8, (u8, u8, u8))> = set.members().map(|m| (m.slot, m.color)).collect();
        assert_eq!(
            colours,
            vec![
                (2, COMPARE_COLORS[1]),
                (3, COMPARE_COLORS[2]),
                (4, COMPARE_COLORS[3])
            ]
        );
        // removing slot 3 leaves 2 and 4 exactly where they were
        let removed = set.remove(3).expect("slot 3 exists");
        assert_eq!(removed.name, "b.vcd");
        assert_eq!(
            set.members().map(|m| m.slot).collect::<Vec<_>>(),
            vec![2, 4]
        );
        assert_eq!(
            set.members().nth(1).expect("slot 4 survives").color,
            COMPARE_COLORS[3]
        );
        // the next add takes the smallest free slot, not the highest
        assert_eq!(set.add("d.vcd", None, wave_vcd(), None), Ok(3));
        // removing a slot that is not there is a no-op and does not bump
        let version = set.version;
        assert!(set.remove(7).is_none());
        assert_eq!(set.version, version);
    }

    #[test]
    fn clearing_drops_every_member_and_nothing_else() {
        let mut set = CompareSet::default();
        set.add("a.vcd", None, wave_vcd(), None).expect("room");
        set.add("b.vcd", None, wave_vcd(), None).expect("room");
        let version = set.version;
        set.clear();
        assert!(set.is_empty());
        assert_ne!(set.version, version);
        // an empty set has nothing to clear: the version stays put
        let version = set.version;
        set.clear();
        assert_eq!(set.version, version);
    }

    #[test]
    fn the_mask_names_every_dump_that_has_the_signal() {
        let mut set = CompareSet::default();
        // the fixture has clk and pc; the second dump has clk and only_here
        set.add("other.vcd", None, other_vcd(), None).expect("room");
        // both dumps have clk: base + slot 2
        assert_eq!(set.bit("top.clk"), 0b11);
        // only the base has pc
        assert_eq!(set.bit("top.pc"), 0b1);
        // only the member has only_here. Bit 0 means "the row came from the base
        // tree" and is always set; a caller adding a member-only signal clears
        // it (see App::waveform_mask).
        assert_eq!(set.bit("top.only_here"), 0b11);
        // nowhere: just the base bit, because the row came from the base tree
        assert_eq!(set.bit("top.nothing"), 0b1);
        // a second member that does not have clk either
        set.add("third.vcd", None, other_vcd(), None).expect("room");
        assert_eq!(set.bit("top.pc"), 0b1);
        assert_eq!(set.bit("top.only_here"), 0b111);
    }

    #[test]
    fn an_empty_set_means_bit_zero_only() {
        let set = CompareSet::default();
        assert_eq!(set.bit("top.pc"), 1);
        assert_eq!(set.bit(""), 1);
        assert_eq!(set.bit("does.not.exist"), 1);
        assert_eq!(slot_bit(1), 1);
        assert_eq!(slot_bit(8), 0b1000_0000);
        assert_eq!(slot_bit(0), 0);
        assert_eq!(slot_bit(9), 0);
    }

    #[test]
    fn the_status_texts_name_the_file_and_the_numbers() {
        assert_eq!(
            loaded_status(
                2,
                "run2.vcd",
                Some(13_002_342),
                Some(TimeRange {
                    start: 0,
                    end: 250_000
                })
            ),
            // the size formatter rounds to whole megabytes above 10 MB
            "compare 2: run2.vcd (12 MB, range 0..250000)"
        );
        assert_eq!(
            loaded_status(3, "run3.fst", None, None),
            "compare 3: run3.fst (range unknown)"
        );
        assert_eq!(removed_status(4), "compare 4 removed");
    }

    #[test]
    fn waveforms_are_listed_in_slot_order() {
        assert_eq!(waveforms_label(0b1), "waveform 1");
        assert_eq!(waveforms_label(0b11), "waveforms 1, 2");
        assert_eq!(waveforms_label(0b1010), "waveforms 2, 4");
        assert_eq!(waveforms_label(0), "no waveform");
    }

    #[test]
    fn the_toolbar_colour_is_the_css_form() {
        assert_eq!(rgb((255, 159, 67)), "rgb(255,159,67)");
        assert_eq!(rgb(COMPARE_COLORS[0]), "rgb(76,194,255)");
    }

    #[test]
    fn a_different_timescale_is_reported_once() {
        use wave_insight_lib::waveform::{TimeUnit, Timescale};
        let ns = Timescale::new(1, TimeUnit::NanoSeconds);
        let ps = Timescale::new(1, TimeUnit::PicoSeconds);
        assert_eq!(timescale_note(2, ns, ns), None);
        assert_eq!(
            timescale_note(2, ps, ns).as_deref(),
            Some("compare 2: timescale 1 ps differs from 1 ns — traces are aligned by tick")
        );
        // the refusal for the native read-only mode names the way out
        assert!(NATIVE_REFUSAL.contains("native (read-only, mmap) mode"));
        assert!(NATIVE_REFUSAL.contains("reopen the dump through the wasm path"));
    }

    #[test]
    fn an_open_failure_names_the_file() {
        let text = open_error("run2.vcd", "unknown waveform format");
        assert!(text.contains("run2.vcd"));
        assert!(text.contains("unknown waveform format"));
        // what a transport does with its parsed dump
        let handle = DumpHandle::new(
            Waveform::open_bytes(include_bytes!("../../../qa/fixtures/wave.vcd").to_vec())
                .expect("the fixture opens"),
        );
        assert!(handle.waveform().signal_by_path("top.clk").is_some());
    }
}
