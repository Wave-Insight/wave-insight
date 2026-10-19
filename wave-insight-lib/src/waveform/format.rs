//! File format and timescale descriptions.
//!
//! These are intentionally *not* re-exports of `wellen` types: the rest of the
//! viewer should not have to care which backend library is used, and swapping
//! the backend later must not ripple through the UI.

use std::fmt;

/// Waveform file format that was detected (or explicitly requested).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WaveFormat {
    /// Value Change Dump, the IEEE 1364 text format.
    Vcd,
    /// Fast Signal Trace, GTKWave's binary format.
    Fst,
    /// GHDL Wave, GHDL's binary format.
    Ghw,
}

impl WaveFormat {
    /// Map wellen's format enum, `None` for "unknown".
    pub fn from_wellen(format: wellen::FileFormat) -> Option<Self> {
        match format {
            wellen::FileFormat::Vcd => Some(WaveFormat::Vcd),
            wellen::FileFormat::Fst => Some(WaveFormat::Fst),
            wellen::FileFormat::Ghw => Some(WaveFormat::Ghw),
            wellen::FileFormat::Unknown => None,
        }
    }

    /// Upper case short name, e.g. `"VCD"`.
    pub fn name(self) -> &'static str {
        match self {
            WaveFormat::Vcd => "VCD",
            WaveFormat::Fst => "FST",
            WaveFormat::Ghw => "GHW",
        }
    }

    /// Lower case file extension, e.g. `"vcd"`.
    pub fn extension(self) -> &'static str {
        match self {
            WaveFormat::Vcd => "vcd",
            WaveFormat::Fst => "fst",
            WaveFormat::Ghw => "ghw",
        }
    }
}

impl fmt::Display for WaveFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Unit of a [`Timescale`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TimeUnit {
    /// `zs`
    ZeptoSeconds,
    /// `as`
    AttoSeconds,
    /// `fs`
    FemtoSeconds,
    /// `ps`
    PicoSeconds,
    /// `ns`
    NanoSeconds,
    /// `us`
    MicroSeconds,
    /// `ms`
    MilliSeconds,
    /// `s`
    Seconds,
}

impl TimeUnit {
    /// Power of ten relative to one second.
    pub fn exponent(self) -> i8 {
        match self {
            TimeUnit::ZeptoSeconds => -21,
            TimeUnit::AttoSeconds => -18,
            TimeUnit::FemtoSeconds => -15,
            TimeUnit::PicoSeconds => -12,
            TimeUnit::NanoSeconds => -9,
            TimeUnit::MicroSeconds => -6,
            TimeUnit::MilliSeconds => -3,
            TimeUnit::Seconds => 0,
        }
    }

    /// `"ns"`, `"ps"`, ...
    pub fn symbol(self) -> &'static str {
        match self {
            TimeUnit::ZeptoSeconds => "zs",
            TimeUnit::AttoSeconds => "as",
            TimeUnit::FemtoSeconds => "fs",
            TimeUnit::PicoSeconds => "ps",
            TimeUnit::NanoSeconds => "ns",
            TimeUnit::MicroSeconds => "us",
            TimeUnit::MilliSeconds => "ms",
            TimeUnit::Seconds => "s",
        }
    }
}

impl fmt::Display for TimeUnit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.symbol())
    }
}

/// The time unit of all time stamps of a waveform, e.g. `1 ns`.
///
/// All times in this module are raw ticks (like the old `Module::end_clock`);
/// multiply by [`Timescale::seconds_per_tick`] to get seconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timescale {
    /// Multiplier, usually `1`.
    pub factor: u32,
    /// Unit of one tick.
    pub unit: TimeUnit,
}

impl Timescale {
    /// Build from a factor and a unit.
    pub fn new(factor: u32, unit: TimeUnit) -> Self {
        Self { factor, unit }
    }

    /// Duration of one tick in seconds.
    pub fn seconds_per_tick(self) -> f64 {
        self.factor as f64 * 10f64.powi(self.unit.exponent() as i32)
    }

    /// Map wellen's timescale, `None` if the unit is unknown.
    pub fn from_wellen(timescale: wellen::Timescale) -> Option<Self> {
        let unit = match timescale.unit {
            wellen::TimescaleUnit::ZeptoSeconds => TimeUnit::ZeptoSeconds,
            wellen::TimescaleUnit::AttoSeconds => TimeUnit::AttoSeconds,
            wellen::TimescaleUnit::FemtoSeconds => TimeUnit::FemtoSeconds,
            wellen::TimescaleUnit::PicoSeconds => TimeUnit::PicoSeconds,
            wellen::TimescaleUnit::NanoSeconds => TimeUnit::NanoSeconds,
            wellen::TimescaleUnit::MicroSeconds => TimeUnit::MicroSeconds,
            wellen::TimescaleUnit::MilliSeconds => TimeUnit::MilliSeconds,
            wellen::TimescaleUnit::Seconds => TimeUnit::Seconds,
            wellen::TimescaleUnit::Unknown => return None,
        };
        Some(Timescale {
            factor: timescale.factor,
            unit,
        })
    }
}

impl fmt::Display for Timescale {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.factor, self.unit)
    }
}
