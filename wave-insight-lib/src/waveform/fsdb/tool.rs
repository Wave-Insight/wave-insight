//! The `fsdb2vcd` command line converter.
//!
//! Verdi ships `fsdb2vcd <in.fsdb> [-o out.vcd]`; it reads a trace without a
//! licence and is the only FSDB reader this build can rely on.  The program is
//! taken from `WAVE_INSIGHT_FSDB2VCD` when that variable names one, otherwise
//! from `PATH`.
//!
//! The spawn is deliberately thin: [`ToolConverter::convert`] runs the command,
//! keeps the tail of its output for the error message and lets the shared
//! validation in [`super::validate_output`] decide whether the produced file is
//! usable.  Nothing here is a parser.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use super::Converter;

/// The executable Verdi installs next to its other FSDB utilities.
const PROGRAM: &str = "fsdb2vcd";
/// Environment override for the program path.
pub(crate) const ENV_PROGRAM: &str = "WAVE_INSIGHT_FSDB2VCD";
/// How much of the tool's output an error carries.
const OUTPUT_TAIL: usize = 2 * 1024;

/// Runs the vendor CLI for one conversion.
#[derive(Debug)]
pub(crate) struct ToolConverter {
    program: PathBuf,
}

impl ToolConverter {
    /// The tool as configured on this machine, `None` when it is not there.
    pub(crate) fn discover() -> Option<Self> {
        Self::discover_with(std::env::var_os(ENV_PROGRAM), &path_dirs())
    }

    /// Discovery with the environment injected, so the lookup is testable
    /// without touching the process environment or the real `PATH`.
    pub(crate) fn discover_with(program: Option<OsString>, dirs: &[PathBuf]) -> Option<Self> {
        if let Some(program) = program {
            let configured = PathBuf::from(program);
            if configured.is_file() {
                return Some(Self {
                    program: configured,
                });
            }
            // A bare name is a request for that command on PATH; anything with
            // a directory component has to exist as given.
            if configured.components().count() == 1 {
                return find_on_path(&configured, dirs).map(|program| Self { program });
            }
            // Explicitly configured but missing: never silently fall back to a
            // different install, the error tells the user what to fix.
            return None;
        }
        find_on_path(Path::new(PROGRAM), dirs).map(|program| Self { program })
    }

    /// A converter for exactly this program (the tests spawn a known binary).
    #[cfg(test)]
    pub(crate) fn with_program(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
        }
    }
}

impl Converter for ToolConverter {
    fn name(&self) -> &'static str {
        PROGRAM
    }

    fn convert(&self, input: &Path, output: &Path) -> Result<(), String> {
        let outcome = Command::new(&self.program)
            .arg(input)
            .arg("-o")
            .arg(output)
            .output();
        let run = match outcome {
            Ok(run) => run,
            Err(error) => return Err(format!("could not run {}: {error}", self.program.display())),
        };
        if !run.status.success() {
            let stderr = tail(&run.stderr);
            let stdout = tail(&run.stdout);
            let detail = if stderr.is_empty() { stdout } else { stderr };
            let _ = std::fs::remove_file(output);
            let detail = if detail.is_empty() {
                "no output".to_string()
            } else {
                detail
            };
            return Err(format!(
                "{} exited with {}: {detail}",
                self.program.display(),
                describe_status(&run.status)
            ));
        }
        if let Err(detail) = super::validate_output(output) {
            let _ = std::fs::remove_file(output);
            return Err(format!("{}: {detail}", self.program.display()));
        }
        Ok(())
    }
}

fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default()
}

/// Look `name` up in these directories, trying the `.exe` suffix on Windows.
fn find_on_path(name: &Path, dirs: &[PathBuf]) -> Option<PathBuf> {
    for dir in dirs {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        if cfg!(windows) && name.extension().is_none() {
            let candidate = dir.join(format!("{}.exe", name.display()));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn describe_status(status: &std::process::ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exit code {code}"),
        None => "a signal".to_string(),
    }
}

/// The last [`OUTPUT_TAIL`] bytes of a tool's output, lossily decoded and
/// trimmed: a compiler-style error line is at the end, not the beginning.
pub(crate) fn tail(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(OUTPUT_TAIL);
    String::from_utf8_lossy(&bytes[start..]).trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wave-insight-fsdb-tool-{}-{name}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create the test directory");
        dir
    }

    #[test]
    fn discovery_needs_a_real_program() {
        assert!(ToolConverter::discover_with(None, &[]).is_none());
        assert!(ToolConverter::discover_with(
            Some(OsString::from("definitely-missing-fsdb2vcd")),
            &[]
        )
        .is_none());

        let dir = temp_dir("discovery");
        fs::create_dir_all(&dir).expect("create the discovery dir");
        let program = dir.join(if cfg!(windows) {
            "fsdb2vcd.exe"
        } else {
            "fsdb2vcd"
        });
        fs::write(&program, b"").expect("create the fake program");

        let found = ToolConverter::discover_with(None, std::slice::from_ref(&dir))
            .expect("found on the injected PATH");
        assert_eq!(found.program, program);
        assert_eq!(found.name(), "fsdb2vcd");

        // An explicitly configured program wins over the PATH lookup.
        let configured = ToolConverter::discover_with(Some(program.clone().into_os_string()), &[])
            .expect("the configured program");
        assert_eq!(configured.program, program);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn the_output_tail_keeps_the_last_two_kibibytes() {
        let mut bytes = vec![b'a'; 3 * 1024];
        bytes.extend_from_slice(b"error: bad trace\n");
        let tail = tail(&bytes);
        assert!(tail.len() <= OUTPUT_TAIL, "{}", tail.len());
        assert!(tail.ends_with("error: bad trace"), "{tail}");
        assert!(super::tail(&[]).is_empty());
    }

    #[test]
    fn an_empty_output_is_rejected() {
        let dir = temp_dir("empty-output");
        let empty = dir.join("empty.vcd");
        fs::write(&empty, b"").expect("write an empty file");
        let error = super::super::validate_output(&empty).expect_err("an empty file is not a VCD");
        assert!(error.contains("empty"), "{error}");
        let missing = dir.join("missing.vcd");
        assert!(super::super::validate_output(&missing).is_err());
        let usable = dir.join("usable.vcd");
        fs::write(&usable, b"$enddefinitions $end\n").expect("write a usable file");
        assert!(super::super::validate_output(&usable).is_ok());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failing_tool_reports_its_stderr_tail() {
        // The test harness itself is the only executable every platform has:
        // libtest rejects the `-o` argument and exits non-zero with a message on
        // stderr. This exercises the status/stderr branch without a batch file
        // (Rust's Command cannot run one) and without a Verdi install.
        let exe = std::env::current_exe().expect("the test executable");
        let dir = temp_dir("failing-tool");
        let input = dir.join("run.fsdb");
        fs::write(&input, b"input bytes").expect("write the input");
        let output = dir.join("out.vcd");

        let converter = ToolConverter::with_program(exe);
        let error = converter
            .convert(&input, &output)
            .expect_err("the harness refuses the fsdb2vcd argument shape");
        assert!(error.contains("exited with"), "{error}");
        assert!(
            !error.ends_with("no output"),
            "expected the stderr tail in: {error}"
        );
        assert!(!output.exists(), "a failed run leaves no partial output");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_conversion_is_a_typed_fsdb_conversion_error() {
        let exe = std::env::current_exe().expect("the test executable");
        let dir = temp_dir("typed-error");
        let input = dir.join("run.fsdb");
        fs::write(&input, b"input bytes").expect("write the input");
        let converter = ToolConverter::with_program(exe);

        let error = super::super::prepare_with(&input, &converter)
            .expect_err("the failing tool is a conversion error");
        assert!(
            matches!(error, crate::waveform::WaveformError::FsdbConversion { .. }),
            "{error:?}"
        );
        let message = error.message();
        assert!(message.contains("could not convert FSDB"), "{message}");
        assert!(message.contains("run.fsdb"), "{message}");
        assert!(message.contains("exited with"), "{message}");

        let _ = fs::remove_dir_all(dir);
    }
}
