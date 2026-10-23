//! The fake helper itself (VT78): the test binary, started as the
//! manifest's interpreter, reads its "script" for its scenario and speaks
//! FH's frames on its stdout. It answers with the replies [`super::Replaying`]
//! left under each request's digest, and breaks them as its scenario says.
//! A fake never outlives a test by long: every wait of its own ends within
//! a minute.

// The fake is a process of its own: it prints, exits and aborts.
#![allow(
    clippy::print_stderr,
    clippy::exit,
    clippy::expect_used,
    clippy::disallowed_methods
)]

use std::ffi::OsString;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use fitment_core::sha256_hex;
use serde_json::{Value, json};

use super::{Death, FAKE_PYTHON, FAKE_SCRIPT, Scenario};
use crate::frame::{self, Caps, Frame, FrameError, MAGIC};
use crate::{BINDING, FIT_PROTOCOL, KERNEL_NAME, LOCK};

/// The longest any fake waits by itself.
const AT_MOST: Duration = Duration::from_secs(60);

/// VT78: if this process was started as a fake helper
/// (`<exe> -I <script> --site <site>`), be one, and exit; else return.
pub fn fake_main() {
    let args: Vec<OsString> = std::env::args_os().collect();
    if args.get(1).is_none_or(|a| a != "-I") {
        return;
    }
    let code = match Fake::from_args(&args) {
        Ok(fake) => fake.run(&args),
        Err(why) => {
            eprintln!("fake helper: {why}");
            2
        }
    };
    std::process::exit(code);
}

struct Fake {
    scenario: Scenario,
    state: PathBuf,
    script: PathBuf,
    site: String,
    launched: bool,
}

impl Fake {
    fn from_args(args: &[OsString]) -> Result<Self, String> {
        let [_, _, script, site_flag, site, rest @ ..] = args else {
            return Err("not started as `-I <script> --site <site>`".to_owned());
        };
        if site_flag != "--site" {
            return Err("no --site".to_owned());
        }
        let script = PathBuf::from(script);
        let text = fs::read_to_string(&script).map_err(|e| e.to_string())?;
        if !text.starts_with(FAKE_SCRIPT) {
            return Err("not a fake script".to_owned());
        }
        let value = |key: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(key)?.trim_start().strip_prefix('='))
                .map(|v| v.trim().to_owned())
        };
        let scenario = value("scenario")
            .and_then(|n| Scenario::named(&n))
            .ok_or("no scenario")?;
        let state = PathBuf::from(value("state").ok_or("no state")?);
        Ok(Self {
            scenario,
            state,
            script,
            site: site.to_string_lossy().into_owned(),
            launched: rest.first().is_some_and(|a| a == "--launched"),
        })
    }

    /// This helper's index among the scenario's spawns, from 0.
    fn number(&self) -> usize {
        (0..10_000)
            .find(|n| {
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(self.state.join(format!("spawn-{n}")))
                    .is_ok()
            })
            .unwrap_or(0)
    }

    fn run(&self, args: &[OsString]) -> i32 {
        if self.scenario == Scenario::Report {
            self.report(args);
        }
        match self.scenario {
            Scenario::CrashBeforeHello => die(Death::Segv),
            Scenario::IgnoresStdin => linger(),
            Scenario::Launcher if !self.launched => self.launch(args),
            _ => self.serve(),
        }
    }

    /// FY2: what the process was started with.
    fn report(&self, args: &[OsString]) {
        let fds: Vec<String> = fs::read_dir("/dev/fd")
            .map(|d| {
                d.filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        let report = json!({
            "args": args.iter().skip(1).map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>(),
            "env": std::env::vars_os().map(|(k, _)| k.to_string_lossy().into_owned()).collect::<Vec<_>>(),
            "cwd": std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default(),
            "fds": fds,
        });
        let _ = fs::write(
            self.state
                .join(format!("report-{}.json", std::process::id())),
            report.to_string(),
        );
    }

    /// A launcher (FL2's reason): its own child serves, with the pipes it
    /// inherits; the launcher waits.
    fn launch(&self, args: &[OsString]) -> i32 {
        let exe = std::env::current_exe().expect("the test binary");
        let child = std::process::Command::new(exe)
            .args(&args[1..])
            .arg("--launched")
            .spawn();
        if let Ok(child) = child {
            let _ = fs::write(self.state.join("grandchild.pid"), child.id().to_string());
        }
        linger()
    }

    fn serve(&self) -> i32 {
        let number = self.number();
        let mut stdin = io::stdin().lock();
        let mut stdout = io::stdout().lock();
        let mut seen_hello = false;
        loop {
            if seen_hello && self.scenario == Scenario::StopsReading {
                linger();
            }
            let request = match frame::read_frame(&mut stdin, Caps::REQUEST) {
                Ok(request) => request,
                Err(FrameError::Closed) => return self.at_end(),
                Err(e) => {
                    eprintln!("fake helper: {e}");
                    return 3;
                }
            };
            for (i, payload) in request.payloads.iter().enumerate() {
                let _ = fs::write(format!("payload-{i}.bin"), payload);
            }
            let Ok(header) = serde_json::from_slice::<Value>(&request.header) else {
                return 3;
            };
            let op = header["op"].as_str().unwrap_or_default().to_owned();
            if op == "bye" {
                return self.at_end();
            }
            seen_hello |= op == "hello";
            if self.scenario == Scenario::NoiseOnStderr {
                eprintln!("**** ERR StepFile : Undefined Parsing: Line 1");
            }
            let reply = match op.as_str() {
                "hello" => Some(self.hello(&header)),
                "scene" | "place" | "check" => self.reply(&op, &header, &request, number),
                _ => None,
            };
            let bytes = match reply {
                Some(reply) => self.envelope(&op, &request.header, &header, &reply),
                None => error_frame(&header),
            };
            if self.write(&mut stdout, &op, &bytes).is_err() {
                return 4;
            }
        }
    }

    fn at_end(&self) -> i32 {
        if self.scenario == Scenario::IgnoresBye {
            linger();
        }
        if self.launched {
            // The kernel's process, busy for a while after its launcher's
            // kill: it outlives it (FL2).
            std::thread::sleep(Duration::from_secs(3));
        }
        0
    }

    fn hello(&self, header: &Value) -> Value {
        let script = fs::read(&self.script).unwrap_or_default();
        let lock = self
            .script
            .parent()
            .map(|d| fs::read(d.join(LOCK)).unwrap_or_default())
            .unwrap_or_default();
        let mut hello = json!({
            "protocol": FIT_PROTOCOL,
            "pid": std::process::id(),
            "helper_sha256": sha256_hex(&script),
            "lock_sha256": sha256_hex(&lock),
            "python": FAKE_PYTHON,
            "binding": BINDING,
            "kernel": KERNEL_NAME,
            "site": self.site,
            "platform": "fake",
            "limits": [{"name": "RLIMIT_CORE", "took": true, "value": 0}],
            "unavailable": null,
        });
        let _ = header;
        let (key, value) = match self.scenario {
            Scenario::OtherProtocol => ("protocol", json!(FIT_PROTOCOL + 1)),
            Scenario::OtherPid => ("pid", json!(std::process::id() + 1)),
            Scenario::OtherScript => ("helper_sha256", json!(sha256_hex(b"another script"))),
            Scenario::OtherLock => ("lock_sha256", json!(sha256_hex(b"another lock"))),
            Scenario::OtherBinding => ("binding", json!("cadquery-ocp-novtk 7.8.1.1.0")),
            Scenario::OtherKernel => ("kernel", json!("Open CASCADE 7.8.1")),
            Scenario::OtherSite => ("site", json!("/elsewhere/site-packages")),
            Scenario::OtherPython => ("python", json!("3.13.9")),
            Scenario::ImportError => (
                "unavailable",
                json!("libGL.so.1: cannot open shared object file: No such file or directory"),
            ),
            _ => return hello,
        };
        hello[key] = value;
        hello
    }

    /// The reply [`super::Replaying`] left for `header`'s digest, broken
    /// as the scenario says; `None` for an `error` frame.
    fn reply(&self, op: &str, header: &Value, request: &Frame, number: usize) -> Option<Value> {
        let digest = header["digest"].as_str()?;
        let mut body: Value =
            serde_json::from_slice(&fs::read(self.state.join(format!("{digest}.json"))).ok()?)
                .ok()?;
        match op {
            "scene" => {
                let sent = header["request"]["scene"].as_str();
                if request.payloads.first().map(|p| sha256_hex(p)).as_deref() != sent {
                    return None;
                }
                self.at_scene(&mut body, number);
            }
            "place" => {
                body["sha256"] = request
                    .payloads
                    .first()
                    .map_or(Value::Null, |p| json!(sha256_hex(p)));
                self.at_place(number);
            }
            _ => self.at_check(&mut body),
        }
        Some(body)
    }

    fn at_scene(&self, body: &mut Value, number: usize) {
        match self.scenario {
            Scenario::CrashAfterHello => die(Death::Segv),
            Scenario::Hang => linger(),
            Scenario::ExtraKey => body["bodies"][0]["extra"] = json!(1),
            Scenario::NegativeVolume => body["bodies"][0]["volume_mm3"] = json!(-1.0),
            Scenario::NanInReply => body["bodies"][0]["volume_mm3"] = json!("__NAN__"),
            Scenario::CrashOnceThenAnotherScene if number > 0 => {
                if let Some(v) = body["bodies"][0]["volume_mm3"].as_f64() {
                    body["bodies"][0]["volume_mm3"] = json!(f64::from_bits(v.to_bits() + 1));
                }
            }
            _ => {}
        }
    }

    fn at_place(&self, number: usize) {
        match self.scenario {
            Scenario::Dies(death) => die(death),
            Scenario::CrashOnce | Scenario::CrashOnceThenAnotherScene if number == 0 => {
                die(Death::Segv);
            }
            _ => {}
        }
    }

    fn at_check(&self, body: &mut Value) {
        let Some(pairs) = body["pairs"].as_array_mut() else {
            return;
        };
        match self.scenario {
            Scenario::MissingKey => {
                if let Some(first) = pairs.first_mut().and_then(Value::as_object_mut) {
                    first.remove("distance_mm");
                }
            }
            Scenario::MissingPair => {
                pairs.pop();
            }
            Scenario::ExtraPair => {
                if let Some(first) = pairs.first().cloned() {
                    pairs.push(first);
                }
            }
            Scenario::PairNotAsked => {
                if let Some(first) = pairs.first_mut() {
                    first["body"] = json!([999_999]);
                }
            }
            Scenario::ResidualAboveCommon => {
                if let Some(first) = pairs.first_mut() {
                    first["common_mm3"] = json!(1.0);
                    first["residual_mm3"] = json!(2.0);
                    first["distance_mm"] = json!(0.0);
                }
            }
            Scenario::ConservationMiss => {
                // A zoned pair, in contact: some of the Common is lost.
                let zoned = pairs.iter_mut().find(|p| {
                    p["zones"].as_array().is_some_and(|z| !z.is_empty())
                        && p["distance_mm"].as_f64() == Some(0.0)
                });
                if let Some(pair) = zoned {
                    pair["common_mm3"] = json!(2e-4);
                    pair["inside_mm3"] = json!(1e-4);
                    pair["residual_mm3"] = json!(0.0);
                }
            }
            _ => {}
        }
    }

    /// FH2's reply header around `reply`, broken as the scenario says.
    fn envelope(&self, op: &str, raw: &[u8], header: &Value, reply: &Value) -> Vec<u8> {
        let mut envelope = json!({
            "op": op,
            "nonce": header["nonce"],
            "request_sha256": sha256_hex(raw),
            "reply": reply,
        });
        if let Some(digest) = header.get("digest") {
            envelope["digest"] = digest.clone();
        }
        if op == "scene" {
            match self.scenario {
                Scenario::WrongOp => envelope["op"] = json!("place"),
                Scenario::WrongNonce => envelope["nonce"] = json!("0".repeat(32)),
                Scenario::WrongRequestSha256 => envelope["request_sha256"] = json!("0".repeat(64)),
                Scenario::WrongDigest => envelope["digest"] = json!("0".repeat(64)),
                Scenario::AnnouncedPayload => envelope["payloads"] = json!([1000]),
                Scenario::ErrorFrame => return error_frame(header),
                _ => {}
            }
        }
        envelope
            .to_string()
            .replace("\"__NAN__\"", "NaN")
            .into_bytes()
    }

    /// Writes one reply frame, or breaks it as the scenario says.
    fn write(&self, out: &mut impl Write, op: &str, header: &[u8]) -> io::Result<()> {
        let frame = frame::encode(header, &[])?;
        if op != "scene" {
            return out.write_all(&frame).and_then(|()| out.flush());
        }
        match self.scenario {
            Scenario::ExitsMidReply => {
                out.write_all(&frame[..frame.len() / 2])?;
                out.flush()?;
                std::process::exit(0);
            }
            Scenario::Drips => {
                for byte in &frame {
                    out.write_all(&[*byte])?;
                    out.flush()?;
                    std::thread::sleep(Duration::from_secs(1));
                }
                Ok(())
            }
            Scenario::Garbage => {
                out.write_all(b"this is not a frame\n")?;
                out.flush()?;
                linger()
            }
            Scenario::NoiseOnFrames => {
                out.write_all(b"**** ERR StepFile : Undefined Parsing: Line 1\n")?;
                out.write_all(&frame)?;
                out.flush()
            }
            Scenario::ShortFrame => {
                let cut = &header[..header.len() / 2];
                out.write_all(&frame::encode(cut, &[])?)?;
                out.flush()
            }
            Scenario::OversizedHeader => {
                out.write_all(&MAGIC)?;
                out.write_all(&((16u32 << 20) + 1).to_be_bytes())?;
                out.flush()?;
                linger()
            }
            _ => out.write_all(&frame).and_then(|()| out.flush()),
        }
    }
}

/// FH5: the `error` frame for a request the fake does not answer.
fn error_frame(header: &Value) -> Vec<u8> {
    json!({
        "op": "error",
        "nonce": header.get("nonce").cloned().unwrap_or(Value::Null),
        "request_sha256": null,
        "error": {"code": "unreadable", "sentence": "the request cannot be answered"},
    })
    .to_string()
    .into_bytes()
}

/// Waits for fitment to end this process, at most a minute.
fn linger() -> ! {
    std::thread::sleep(AT_MOST);
    std::process::exit(5);
}

/// Ends this process as `death` says (FV8): by its signal on Unix, by an
/// NTSTATUS code on Windows.
fn die(death: Death) -> ! {
    if death == Death::Abrt {
        std::process::abort();
    }
    #[cfg(unix)]
    {
        let kill = ["/bin/kill", "/usr/bin/kill"]
            .into_iter()
            .find(|k| Path::new(k).exists())
            .unwrap_or("kill");
        // Rust's runtime catches the first SIGSEGV or SIGBUS to tell a
        // stack overflow, and puts the default back: the next one ends it.
        for _ in 0..3 {
            let _ = std::process::Command::new(kill)
                .args(["-s", death.signal(), &std::process::id().to_string()])
                .status();
            std::thread::sleep(Duration::from_millis(100));
        }
        linger();
    }
    #[cfg(not(unix))]
    {
        let code: u32 = match death {
            Death::Segv => 0xC000_0005,
            Death::Bus => 0xC000_0006,
            Death::Fpe => 0xC000_0094,
            _ => 1,
        };
        #[allow(clippy::cast_possible_wrap)]
        std::process::exit(code as i32);
    }
}
