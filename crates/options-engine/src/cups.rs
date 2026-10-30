//! CUPS, spoken directly: a small IPP client over the local scheduler socket.
//!
//! Printing in Golem is CUPS (Golem's `desktop/printing.nix`). What the shell
//! needs from it — which printers there are and what state each is in, which
//! prints are waiting or running and how far along, and the few things a
//! person does to a print (cancel it, send it again, take a paused printer up
//! again, choose the default) — is all one IPP request away, so this speaks
//! IPP itself instead of reading `lpstat`'s English: every attribute comes
//! typed (the reasons a printer gives as keywords, a print's sheets as a
//! number), in one round trip, with no child process.
//!
//! The wire: an IPP message (RFC 8010) as the body of an HTTP POST to the
//! scheduler's Unix socket. A request CUPS wants a user for (cancelling, an
//! administrative change) is answered 401 and asked again with
//! `Authorization: PeerCred <user>` — CUPS checks that name against the
//! socket's peer credentials, the way its own commands do.
//!
//! Blocking by design (short timeouts, a local socket): the collector calls it
//! on `spawn_blocking`, the daemon on its worker threads.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::state::{
    JobState, Marker, PrintJob, Printer, PrinterProblem, PrinterState, PrintingState,
};

/// How long one request may take before it is given up.
const TIMEOUT: Duration = Duration::from_secs(5);

// --- operations (RFC 8011 + CUPS' own) --------------------------------------

pub(crate) const PRINT_JOB: u16 = 0x0002;
const CANCEL_JOB: u16 = 0x0008;
const GET_JOB_ATTRIBUTES: u16 = 0x0009;
const GET_JOBS: u16 = 0x000A;
const RESTART_JOB: u16 = 0x000E;
const GET_PRINTER_ATTRIBUTES: u16 = 0x000B;
const PAUSE_PRINTER: u16 = 0x0010;
const RESUME_PRINTER: u16 = 0x0011;
pub(crate) const CREATE_PRINTER_SUBSCRIPTIONS: u16 = 0x0016;
pub(crate) const CANCEL_SUBSCRIPTION: u16 = 0x001B;
const CUPS_GET_PRINTERS: u16 = 0x4002;
const CUPS_ADD_MODIFY_PRINTER: u16 = 0x4003;
const CUPS_DELETE_PRINTER: u16 = 0x4004;
const CUPS_SET_DEFAULT: u16 = 0x400A;

// --- tags ------------------------------------------------------------------

const OPERATION: u8 = 0x01;
const END: u8 = 0x03;
pub(crate) const SUBSCRIPTION: u8 = 0x06;

const INTEGER: u8 = 0x21;
const BOOLEAN: u8 = 0x22;
const ENUM: u8 = 0x23;
const TEXT_WITH_LANGUAGE: u8 = 0x35;
const NAME_WITH_LANGUAGE: u8 = 0x36;
const TEXT: u8 = 0x41;
const NAME: u8 = 0x42;
pub(crate) const KEYWORD: u8 = 0x44;
pub(crate) const URI: u8 = 0x45;
const CHARSET: u8 = 0x47;
const LANGUAGE: u8 = 0x48;
const MIME: u8 = 0x49;

/// `printer-type` bit: this is the default destination.
const TYPE_DEFAULT: i32 = 0x20000;

/// One attribute value, as much typed as the shell needs.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Int(i32),
    Bool(bool),
    Str(String),
    /// A value of a kind nothing here reads (dates, ranges, collections).
    Other,
}

/// A response attribute: its name and every value it has.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Attr {
    pub name: String,
    pub values: Vec<Value>,
}

/// One group of a response (an operation, printer, job or subscription
/// group), in order.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Group {
    pub tag: u8,
    pub attrs: Vec<Attr>,
}

impl Group {
    fn get(&self, name: &str) -> Option<&Attr> {
        self.attrs.iter().find(|a| a.name == name)
    }
    pub(crate) fn int(&self, name: &str) -> Option<i32> {
        match self.get(name)?.values.first()? {
            Value::Int(i) => Some(*i),
            _ => None,
        }
    }
    fn boolean(&self, name: &str) -> Option<bool> {
        match self.get(name)?.values.first()? {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }
    fn text(&self, name: &str) -> String {
        match self.get(name).and_then(|a| a.values.first()) {
            Some(Value::Str(s)) => s.clone(),
            _ => String::new(),
        }
    }
    fn texts(&self, name: &str) -> Vec<String> {
        self.get(name)
            .map(|a| {
                a.values
                    .iter()
                    .filter_map(|v| match v {
                        Value::Str(s) => Some(s.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    fn ints(&self, name: &str) -> Vec<i32> {
        self.get(name)
            .map(|a| {
                a.values
                    .iter()
                    .filter_map(|v| match v {
                        Value::Int(i) => Some(*i),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// A parsed response: the IPP status and its groups.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Response {
    pub status: u16,
    pub groups: Vec<Group>,
}

impl Response {
    /// `successful-ok` and its `…-ignored-…`/`…-conflicting-…` siblings.
    pub(crate) fn ok(&self) -> bool {
        self.status < 0x0100
    }
    fn of(&self, tag: u8) -> impl Iterator<Item = &Group> {
        self.groups.iter().filter(move |g| g.tag == tag)
    }
    fn first(&self, tag: u8) -> Option<&Group> {
        self.groups.iter().find(|g| g.tag == tag)
    }
}

/// Why a request did not get an answer.
#[derive(Debug, thiserror::Error)]
pub enum CupsError {
    #[error("CUPS is not reachable: {0}")]
    Io(#[from] std::io::Error),
    #[error("CUPS answered HTTP {0}")]
    Http(u16),
    #[error("CUPS's answer could not be read")]
    Garbled,
    #[error("CUPS refused: {0}")]
    Refused(String),
}

// --- requests --------------------------------------------------------------

/// An IPP request being built: the operation attributes come first (charset
/// and language, as the standard asks), then any other group.
pub(crate) struct Request {
    op: u16,
    body: Vec<u8>,
    data: Vec<u8>,
}

impl Request {
    pub(crate) fn new(op: u16) -> Self {
        let mut r = Request {
            op,
            body: Vec::new(),
            data: Vec::new(),
        };
        r.body.push(OPERATION);
        r.attr(CHARSET, "attributes-charset", &["utf-8"]);
        r.attr(LANGUAGE, "attributes-natural-language", &["en"]);
        r
    }

    /// One attribute with string values (the first carries the name, the rest
    /// are its additional values).
    pub(crate) fn attr(&mut self, tag: u8, name: &str, values: &[&str]) -> &mut Self {
        for (i, v) in values.iter().enumerate() {
            self.body.push(tag);
            let n = if i == 0 { name } else { "" };
            self.body.extend_from_slice(&(n.len() as u16).to_be_bytes());
            self.body.extend_from_slice(n.as_bytes());
            self.body.extend_from_slice(&(v.len() as u16).to_be_bytes());
            self.body.extend_from_slice(v.as_bytes());
        }
        self
    }

    pub(crate) fn int(&mut self, name: &str, value: i32) -> &mut Self {
        self.number(INTEGER, name, value)
    }

    /// A four-byte value of `tag` (an integer, or an enum's number).
    fn number(&mut self, tag: u8, name: &str, value: i32) -> &mut Self {
        self.body.push(tag);
        self.body
            .extend_from_slice(&(name.len() as u16).to_be_bytes());
        self.body.extend_from_slice(name.as_bytes());
        self.body.extend_from_slice(&4u16.to_be_bytes());
        self.body.extend_from_slice(&value.to_be_bytes());
        self
    }

    /// Start another group (a subscription's attributes).
    pub(crate) fn group(&mut self, tag: u8) -> &mut Self {
        self.body.push(tag);
        self
    }

    /// The document that follows the attributes (a Print-Job's file).
    pub(crate) fn document(&mut self, data: Vec<u8>) -> &mut Self {
        self.data = data;
        self
    }

    fn encode(&self, id: u32) -> Vec<u8> {
        let mut out = Vec::with_capacity(9 + self.body.len() + self.data.len());
        out.extend_from_slice(&[2, 0]);
        out.extend_from_slice(&self.op.to_be_bytes());
        out.extend_from_slice(&id.to_be_bytes());
        out.extend_from_slice(&self.body);
        out.push(END);
        out.extend_from_slice(&self.data);
        out
    }
}

/// Read an IPP response body.
pub(crate) fn decode(buf: &[u8]) -> Option<Response> {
    let status = u16::from_be_bytes([*buf.get(2)?, *buf.get(3)?]);
    let mut at = 8;
    let mut groups: Vec<Group> = Vec::new();
    loop {
        let tag = *buf.get(at)?;
        at += 1;
        if tag == END {
            break;
        }
        if tag < 0x10 {
            groups.push(Group {
                tag,
                attrs: Vec::new(),
            });
            continue;
        }
        let name_len = u16::from_be_bytes([*buf.get(at)?, *buf.get(at + 1)?]) as usize;
        at += 2;
        let name = String::from_utf8_lossy(buf.get(at..at + name_len)?).into_owned();
        at += name_len;
        let value_len = u16::from_be_bytes([*buf.get(at)?, *buf.get(at + 1)?]) as usize;
        at += 2;
        let raw = buf.get(at..at + value_len)?;
        at += value_len;
        let value = match tag {
            INTEGER | ENUM if raw.len() == 4 => {
                Value::Int(i32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]))
            }
            BOOLEAN if raw.len() == 1 => Value::Bool(raw[0] != 0),
            TEXT_WITH_LANGUAGE | NAME_WITH_LANGUAGE => {
                // language length, language, text length, text
                let ll = u16::from_be_bytes([*raw.first()?, *raw.get(1)?]) as usize;
                let tl_at = 2 + ll;
                let tl = u16::from_be_bytes([*raw.get(tl_at)?, *raw.get(tl_at + 1)?]) as usize;
                let text = raw.get(tl_at + 2..tl_at + 2 + tl)?;
                Value::Str(String::from_utf8_lossy(text).into_owned())
            }
            TEXT..=MIME => Value::Str(String::from_utf8_lossy(raw).into_owned()),
            _ => Value::Other,
        };
        let group = groups.last_mut()?;
        if name.is_empty() {
            // An additional value of the attribute before (or a collection's
            // member, which nothing here reads).
            group.attrs.last_mut()?.values.push(value);
        } else {
            group.attrs.push(Attr {
                name,
                values: vec![value],
            });
        }
    }
    Some(Response { status, groups })
}

// --- the socket ------------------------------------------------------------

/// The scheduler's socket: `CUPS_SERVER` when it names one, else the standard
/// place.
fn socket() -> PathBuf {
    match std::env::var("CUPS_SERVER") {
        Ok(s) if s.starts_with('/') => PathBuf::from(s),
        _ => PathBuf::from("/run/cups/cups.sock"),
    }
}

/// Whether the scheduler is RUNNING: its pid file names a live process
/// (or `CUPS_SERVER` points elsewhere). Not the socket: with socket
/// activation the socket is always there, and asking through it STARTS
/// the scheduler — the collector woke CUPS at every boot and kept it up
/// (1 s of a weak machine's login, a resident cupsd; 2026-10-30). A print
/// from any app starts it, and the collector finds it running within
/// ten seconds.
pub fn present() -> bool {
    if std::env::var("CUPS_SERVER").is_ok_and(|s| !s.is_empty() && !s.starts_with('/')) {
        return true;
    }
    let pid_file = socket().with_file_name("cupsd.pid");
    std::fs::read_to_string(pid_file)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .is_some_and(|pid| std::path::Path::new(&format!("/proc/{pid}")).exists())
}

/// The user this process runs as, by name (what PeerCred claims).
fn user() -> String {
    if let Ok(u) = std::env::var("USER") {
        if !u.is_empty() {
            return u;
        }
    }
    // SAFETY: getpwuid returns a pointer into static storage or null; the
    // name is copied out before anything else can call it on this thread.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_name.is_null() {
            return std::ffi::CStr::from_ptr((*pw).pw_name)
                .to_string_lossy()
                .into_owned();
        }
    }
    "nobody".to_owned()
}

/// Split an HTTP response into its status and body (Content-Length or
/// chunked, or everything to the end).
/// Read one HTTP answer: to the end of its body by its length (or its last
/// chunk), not to the connection's end — CUPS keeps a connection open after
/// a 401, waiting for the request again with who is asking.
fn read_answer(stream: &mut UnixStream) -> std::io::Result<Vec<u8>> {
    let mut raw = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = stream.read(&mut buf)?;
        if n == 0 {
            return Ok(raw);
        }
        raw.extend_from_slice(&buf[..n]);
        if answer_whole(&raw) {
            return Ok(raw);
        }
    }
}

/// Whether `raw` holds a whole HTTP answer (headers, and the body they
/// announce). An answer with neither a length nor chunks ends with the
/// connection.
pub(crate) fn answer_whole(raw: &[u8]) -> bool {
    let Some(head_end) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
        return false;
    };
    let head = String::from_utf8_lossy(&raw[..head_end]).to_ascii_lowercase();
    let body = raw.len() - head_end - 4;
    for l in head.split("\r\n").skip(1) {
        let Some((k, v)) = l.split_once(':') else { continue };
        match k.trim() {
            "content-length" => return v.trim().parse::<usize>().is_ok_and(|n| body >= n),
            "transfer-encoding" if v.trim() == "chunked" => {
                let rest = &raw[head_end + 4..];
                return rest.starts_with(b"0\r\n\r\n") || rest.ends_with(b"\r\n0\r\n\r\n");
            }
            _ => {}
        }
    }
    false
}

pub(crate) fn http_body(raw: &[u8]) -> Option<(u16, Vec<u8>)> {
    let head_end = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..head_end]).ok()?;
    let mut lines = head.split("\r\n");
    let status: u16 = lines.next()?.split_whitespace().nth(1)?.parse().ok()?;
    let mut length: Option<usize> = None;
    let mut chunked = false;
    for l in lines {
        let Some((k, v)) = l.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        if k == "content-length" {
            length = v.parse().ok();
        } else if k == "transfer-encoding" && v.eq_ignore_ascii_case("chunked") {
            chunked = true;
        }
    }
    let rest = &raw[head_end + 4..];
    let body = if chunked {
        let mut out = Vec::new();
        let mut at = 0;
        loop {
            let line_end = rest.get(at..)?.windows(2).position(|w| w == b"\r\n")? + at;
            let size_txt = std::str::from_utf8(&rest[at..line_end]).ok()?;
            let size = usize::from_str_radix(size_txt.split(';').next()?.trim(), 16).ok()?;
            at = line_end + 2;
            if size == 0 {
                break;
            }
            out.extend_from_slice(rest.get(at..at + size)?);
            at += size + 2;
        }
        out
    } else if let Some(n) = length {
        rest.get(..n)?.to_vec()
    } else {
        rest.to_vec()
    };
    Some((status, body))
}

/// Send one request to `resource` and read the answer; asked again with the
/// user's name when CUPS wants to know who is asking.
pub(crate) fn send(resource: &str, req: &Request) -> Result<Response, CupsError> {
    let body = req.encode(1);
    let mut auth: Option<String> = None;
    loop {
        let mut stream = UnixStream::connect(socket())?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        let head = format!(
            "POST {resource} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/ipp\r\n\
             Content-Length: {}\r\nConnection: close\r\n{}\r\n",
            body.len(),
            auth.as_deref()
                .map(|a| format!("Authorization: {a}\r\n"))
                .unwrap_or_default(),
        );
        stream.write_all(head.as_bytes())?;
        stream.write_all(&body)?;
        let raw = read_answer(&mut stream)?;
        let (status, ipp) = http_body(&raw).ok_or(CupsError::Garbled)?;
        if status == 401 && auth.is_none() {
            auth = Some(format!("PeerCred {}", user()));
            continue;
        }
        if status != 200 {
            return Err(CupsError::Http(status));
        }
        return decode(&ipp).ok_or(CupsError::Garbled);
    }
}

/// [`send`], and an IPP refusal is an error too (with CUPS's own words).
fn send_ok(resource: &str, req: &Request) -> Result<Response, CupsError> {
    let r = send(resource, req)?;
    if r.ok() {
        Ok(r)
    } else {
        Err(refused(&r))
    }
}

/// IPP's client-error-not-found.
const NOT_FOUND: u16 = 0x0406;

/// An IPP refusal, in CUPS's own words.
fn refused(r: &Response) -> CupsError {
    let said = r
        .first(OPERATION)
        .map(|g| g.text("status-message"))
        .unwrap_or_default();
    CupsError::Refused(if said.is_empty() {
        format!("0x{:04x}", r.status)
    } else {
        said
    })
}

// --- what CUPS says, made the shell's ---------------------------------------

const PRINTER_ATTRS: &[&str] = &[
    "printer-name",
    "printer-info",
    "printer-make-and-model",
    "printer-location",
    "device-uri",
    "printer-state",
    "printer-state-reasons",
    "printer-state-message",
    "printer-is-accepting-jobs",
    "printer-type",
    "marker-names",
    "marker-colors",
    "marker-levels",
    "marker-types",
];

const JOB_ATTRS: &[&str] = &[
    "job-id",
    "job-printer-uri",
    "job-name",
    "job-originating-user-name",
    "job-state",
    "job-state-reasons",
    "job-printer-state-message",
    "job-media-sheets-completed",
    "job-impressions",
    "time-at-creation",
    "time-at-completed",
];

/// The queue name at the end of a `job-printer-uri`.
fn queue_of(uri: &str) -> String {
    uri.rsplit('/').next().unwrap_or_default().to_owned()
}

/// What stands in a printer's way, from the reasons it gives (RFC 8011
/// keywords, with `-error`/`-warning`/`-report` severities) and the state
/// CUPS keeps. Only what stops printing is a problem: a `-warning` (ink
/// running low) or a `-report` is not.
pub(crate) fn problem_of(
    state: PrinterState,
    reasons: &[String],
    message: &str,
) -> Option<PrinterProblem> {
    let errors: Vec<&str> = reasons
        .iter()
        .map(String::as_str)
        .filter(|r| !r.ends_with("-warning") && !r.ends_with("-report"))
        .map(|r| r.trim_end_matches("-error"))
        .collect();
    let has = |k: &[&str]| errors.iter().any(|r| k.contains(r));
    if has(&["media-empty", "media-needed", "input-tray-missing"]) {
        return Some(PrinterProblem::NoPaper);
    }
    if has(&["media-jam"]) {
        return Some(PrinterProblem::PaperJam);
    }
    if has(&[
        "marker-supply-empty",
        "toner-empty",
        "marker-supply-missing",
    ]) {
        return Some(PrinterProblem::NoInk);
    }
    if has(&["cover-open", "door-open", "interlock-open"]) {
        return Some(PrinterProblem::DoorOpen);
    }
    // A backend that cannot reach its printer says `offline-report` (a report
    // by severity, but it is the one way CUPS says "not answering").
    // `connecting-to-device` alone is every print's first second.
    if has(&["offline", "shutdown"]) || reasons.iter().any(|r| r == "offline-report") {
        return Some(PrinterProblem::Offline);
    }
    if state == PrinterState::Stopped {
        // Paused. system-config-printer pauses a USB printer that went away
        // with this very message; Golem's golem-printer-back takes it up
        // again when it comes back.
        if message == "Unplugged or turned off" {
            return Some(PrinterProblem::Offline);
        }
        return Some(PrinterProblem::Paused);
    }
    // Anything else the printer calls an error (a reason with no severity is
    // CUPS's own bookkeeping, `connecting-to-device` and the like).
    reasons
        .iter()
        .find(|r| r.ends_with("-error"))
        .map(|r| PrinterProblem::Other(r.trim_end_matches("-error").to_owned()))
}

fn printer_state(n: i32) -> PrinterState {
    match n {
        4 => PrinterState::Printing,
        5 => PrinterState::Stopped,
        _ => PrinterState::Idle,
    }
}

pub(crate) fn job_state(n: i32) -> JobState {
    match n {
        3 => JobState::Pending,
        4 => JobState::Held,
        5 => JobState::Printing,
        6 => JobState::Stopped,
        7 => JobState::Canceled,
        8 => JobState::Aborted,
        _ => JobState::Completed,
    }
}

/// A printer group of a CUPS-Get-Printers answer.
pub(crate) fn printer_of(g: &Group) -> Printer {
    let state = printer_state(g.int("printer-state").unwrap_or(3));
    let reasons: Vec<String> = g
        .texts("printer-state-reasons")
        .into_iter()
        .filter(|r| r != "none")
        .collect();
    let message = g.text("printer-state-message");
    let names = g.texts("marker-names");
    let colors = g.texts("marker-colors");
    let levels = g.ints("marker-levels");
    let kinds = g.texts("marker-types");
    let markers = names
        .iter()
        .enumerate()
        .map(|(i, name)| Marker {
            name: name.clone(),
            color: colors.get(i).cloned().unwrap_or_default(),
            // -1 unknown, -2 unknown but there, -3 "some left": no number.
            level: levels
                .get(i)
                .copied()
                .filter(|l| (0..=100).contains(l))
                .map(|l| l as u8),
            kind: kinds.get(i).cloned().unwrap_or_default(),
        })
        .collect();
    Printer {
        name: g.text("printer-name"),
        info: g.text("printer-info"),
        model: g.text("printer-make-and-model"),
        location: g.text("printer-location"),
        uri: g.text("device-uri"),
        problem: problem_of(state, &reasons, &message),
        state,
        accepting: g.boolean("printer-is-accepting-jobs").unwrap_or(true),
        default: g.int("printer-type").is_some_and(|t| t & TYPE_DEFAULT != 0),
        reasons,
        message,
        markers,
    }
}

/// A job group of a Get-Jobs / Get-Job-Attributes answer.
pub(crate) fn job_of(g: &Group) -> Option<PrintJob> {
    let id = u32::try_from(g.int("job-id")?).ok()?;
    let reasons: Vec<String> = g
        .texts("job-state-reasons")
        .into_iter()
        .filter(|r| r != "none")
        .collect();
    Some(PrintJob {
        id,
        printer: queue_of(&g.text("job-printer-uri")),
        title: g.text("job-name"),
        user: g.text("job-originating-user-name"),
        state: job_state(g.int("job-state").unwrap_or(3)),
        reasons,
        message: g.text("job-printer-state-message"),
        sheets_done: g
            .int("job-media-sheets-completed")
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(0),
        pages: g
            .int("job-impressions")
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0),
        created: g
            .int("time-at-creation")
            .and_then(|n| u64::try_from(n).ok())
            .unwrap_or(0),
        finished: g
            .int("time-at-completed")
            .and_then(|n| u64::try_from(n).ok())
            .filter(|n| *n > 0),
    })
}

/// Every printer, and every print not yet finished.
pub fn snapshot() -> Result<PrintingState, CupsError> {
    let mut req = Request::new(CUPS_GET_PRINTERS);
    req.attr(NAME, "requesting-user-name", &[&user()]);
    req.attr(KEYWORD, "requested-attributes", PRINTER_ATTRS);
    // (With no printer at all CUPS answers "not found" — "No destinations
    // added." — which is an empty list, not a CUPS that does not answer.)
    let printers = match send("/", &req)? {
        r if r.ok() || r.status == NOT_FOUND => r,
        r => return Err(refused(&r)),
    };

    let mut req = Request::new(GET_JOBS);
    req.attr(URI, "printer-uri", &["ipp://localhost/"]);
    req.attr(NAME, "requesting-user-name", &[&user()]);
    req.attr(KEYWORD, "which-jobs", &["not-completed"]);
    req.attr(KEYWORD, "requested-attributes", JOB_ATTRS);
    let jobs = send_ok("/", &req)?;

    let mut state = PrintingState {
        printers: printers
            .of(0x04)
            .map(printer_of)
            .filter(|p| !p.name.is_empty())
            .map(|mut p| {
                // A USB printer that is not plugged in is off, whatever CUPS
                // still says of it: a printer taken away while the computer
                // was off (no udev "remove", so no pause) stayed "ready" — or
                // "out of paper", the last thing its last print said.
                if usb_absent(&p.uri, Path::new(USB_DEVICES)) {
                    p.problem = Some(PrinterProblem::Offline);
                }
                p
            })
            .collect(),
        jobs: jobs.of(0x02).filter_map(job_of).collect(),
    };
    state.jobs.sort_by_key(|j| j.id);
    Ok(state)
}

/// The prints that ended (printed, cancelled, failed) since `since` (Unix
/// seconds), newest first — CUPS keeps their history, and their files a
/// while (to print one again).
pub fn ended_since(since: u64) -> Result<Vec<PrintJob>, CupsError> {
    let mut req = Request::new(GET_JOBS);
    req.attr(URI, "printer-uri", &["ipp://localhost/"]);
    req.attr(NAME, "requesting-user-name", &[&user()]);
    req.attr(KEYWORD, "which-jobs", &["completed"]);
    req.attr(KEYWORD, "requested-attributes", JOB_ATTRS);
    let r = send_ok("/", &req)?;
    let mut jobs: Vec<PrintJob> = r
        .of(0x02)
        .filter_map(job_of)
        .filter(|j| j.finished.is_some_and(|t| t >= since))
        .collect();
    jobs.sort_by(|a, b| b.finished.cmp(&a.finished).then(b.id.cmp(&a.id)));
    Ok(jobs)
}

/// Where the kernel lists the USB devices plugged in.
const USB_DEVICES: &str = "/sys/bus/usb/devices";

/// A USB printer's serial number, from its queue's address
/// (`hp:/usb/DeskJet_1200_series?serial=CN17722320`,
/// `usb://HP/DeskJet%201200%20series?serial=CN17722320&interface=1`).
pub(crate) fn usb_serial(uri: &str) -> Option<&str> {
    if !(uri.starts_with("usb://") || uri.starts_with("hp:/usb/") || uri.starts_with("hpfax:/usb/")) {
        return None;
    }
    let (_, query) = uri.split_once('?')?;
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix("serial="))
        .filter(|s| !s.is_empty())
}

/// Whether the USB printer at `uri` is not plugged in: its serial is on no
/// device under `devices`. Not a USB address, no serial in it, or no list
/// to read: it cannot be told, and it is not said to be away.
pub(crate) fn usb_absent(uri: &str, devices: &Path) -> bool {
    let Some(serial) = usb_serial(uri) else { return false };
    let Ok(dir) = std::fs::read_dir(devices) else { return false };
    !dir.flatten().any(|d| {
        std::fs::read_to_string(d.path().join("serial")).is_ok_and(|s| s.trim().eq_ignore_ascii_case(serial))
    })
}

/// One print, whatever its state (how a print that left the waiting list
/// ended).
pub fn job(id: u32) -> Result<Option<PrintJob>, CupsError> {
    let mut req = Request::new(GET_JOB_ATTRIBUTES);
    req.attr(URI, "job-uri", &[&format!("ipp://localhost/jobs/{id}")]);
    req.attr(NAME, "requesting-user-name", &[&user()]);
    req.attr(KEYWORD, "requested-attributes", JOB_ATTRS);
    let r = send("/", &req)?;
    Ok(r.first(0x02).and_then(job_of))
}

fn job_op(op: u16, id: u32) -> Result<(), CupsError> {
    let mut req = Request::new(op);
    req.attr(URI, "job-uri", &[&format!("ipp://localhost/jobs/{id}")]);
    req.attr(NAME, "requesting-user-name", &[&user()]);
    send_ok("/jobs/", &req).map(|_| ())
}

/// Cancel a print (waiting or running).
pub fn cancel_job(id: u32) -> Result<(), CupsError> {
    job_op(CANCEL_JOB, id)
}

/// Send a stopped, cancelled or failed print again (CUPS keeps its file a
/// while after it ends).
pub fn restart_job(id: u32) -> Result<(), CupsError> {
    job_op(RESTART_JOB, id)
}

/// Take a paused printer up again: it prints what waits for it.
pub fn resume_printer(name: &str) -> Result<(), CupsError> {
    let mut req = Request::new(RESUME_PRINTER);
    req.attr(
        URI,
        "printer-uri",
        &[&format!("ipp://localhost/printers/{name}")],
    );
    req.attr(NAME, "requesting-user-name", &[&user()]);
    send_ok("/admin/", &req).map(|_| ())
}

/// Make `name` the printer every app prints to unless told otherwise.
pub fn set_default(name: &str) -> Result<(), CupsError> {
    let mut req = Request::new(CUPS_SET_DEFAULT);
    req.attr(
        URI,
        "printer-uri",
        &[&format!("ipp://localhost/printers/{name}")],
    );
    req.attr(NAME, "requesting-user-name", &[&user()]);
    send_ok("/admin/", &req).map(|_| ())
}

/// Print a file on `printer` (its type found by CUPS): the new print's id.
pub fn print_file(printer: &str, file: &std::path::Path, title: &str) -> Result<u32, CupsError> {
    print_bytes(printer, std::fs::read(file)?, title)
}

/// Print a document held in memory (its type found by CUPS): the new
/// print's id.
pub fn print_bytes(printer: &str, data: Vec<u8>, title: &str) -> Result<u32, CupsError> {
    let mut req = Request::new(PRINT_JOB);
    req.attr(
        URI,
        "printer-uri",
        &[&format!("ipp://localhost/printers/{printer}")],
    );
    req.attr(NAME, "requesting-user-name", &[&user()]);
    req.attr(NAME, "job-name", &[title]);
    req.attr(MIME, "document-format", &["application/octet-stream"]);
    req.document(data);
    let r = send_ok(&format!("/printers/{printer}"), &req)?;
    r.first(0x02)
        .and_then(|g| g.int("job-id"))
        .and_then(|n| u32::try_from(n).ok())
        .ok_or(CupsError::Garbled)
}

/// Take a printer away (its queue, and the prints waiting in it).
pub fn delete_printer(name: &str) -> Result<(), CupsError> {
    let mut req = Request::new(CUPS_DELETE_PRINTER);
    req.attr(URI, "printer-uri", &[&format!("ipp://localhost/printers/{name}")]);
    req.attr(NAME, "requesting-user-name", &[&user()]);
    send_ok("/admin/", &req).map(|_| ())
}

/// Stop a printer taking up its prints (they wait for it).
pub fn pause_printer(name: &str) -> Result<(), CupsError> {
    let mut req = Request::new(PAUSE_PRINTER);
    req.attr(URI, "printer-uri", &[&format!("ipp://localhost/printers/{name}")]);
    req.attr(NAME, "requesting-user-name", &[&user()]);
    send_ok("/admin/", &req).map(|_| ())
}

/// A printer's own defaults — what a print gets unless its app asks for
/// something else — and what it can do. The standard IPP job template
/// names: CUPS maps them onto a driver's own options (`OutputMode`,
/// `ColorModel`, `PageSize`…) at print time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Defaults {
    /// In colour by default (`Some(false)`: in black and white); `None`: it
    /// prints in one colour only.
    pub colour: Option<bool>,
    /// `print-quality`: 3 draft, 4 normal, 5 high — the default, and the ones
    /// it has.
    pub quality: Option<i32>,
    pub qualities: Vec<i32>,
    /// The paper by its PWG name (`na_letter_8.5x11in`), and the ones it takes.
    pub media: String,
    pub medias: Vec<String>,
    /// On both sides by default; `None`: it cannot.
    pub both_sides: Option<bool>,
}

/// Read a printer's defaults.
pub fn defaults(name: &str) -> Result<Defaults, CupsError> {
    let mut req = Request::new(GET_PRINTER_ATTRIBUTES);
    req.attr(URI, "printer-uri", &[&format!("ipp://localhost/printers/{name}")]);
    req.attr(NAME, "requesting-user-name", &[&user()]);
    req.attr(
        KEYWORD,
        "requested-attributes",
        &[
            "print-color-mode-default",
            "print-color-mode-supported",
            "print-quality-default",
            "print-quality-supported",
            "media-default",
            "media-supported",
            "sides-default",
            "sides-supported",
        ],
    );
    let r = send_ok("/", &req)?;
    Ok(r.first(0x04).map(defaults_of).unwrap_or_default())
}

pub(crate) fn defaults_of(g: &Group) -> Defaults {
    let modes = g.texts("print-color-mode-supported");
    let mode = g.text("print-color-mode-default");
    let sides = g.texts("sides-supported");
    Defaults {
        colour: modes.iter().any(|m| m == "color").then(|| mode != "monochrome"),
        quality: g.int("print-quality-default"),
        qualities: g.ints("print-quality-supported"),
        media: g.text("media-default"),
        medias: g.texts("media-supported"),
        both_sides: sides
            .iter()
            .any(|s| s.starts_with("two-sided"))
            .then(|| g.text("sides-default").starts_with("two-sided")),
    }
}

/// One of a printer's defaults changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Default {
    Colour(bool),
    /// 3 draft, 4 normal, 5 high.
    Quality(i32),
    /// A paper by its PWG name.
    Media(String),
    BothSides(bool),
}

/// Change one of a printer's defaults (kept by CUPS with the printer).
pub fn set_default_option(name: &str, value: &Default) -> Result<(), CupsError> {
    let mut req = Request::new(CUPS_ADD_MODIFY_PRINTER);
    req.attr(URI, "printer-uri", &[&format!("ipp://localhost/printers/{name}")]);
    req.attr(NAME, "requesting-user-name", &[&user()]);
    req.group(0x04);
    match value {
        Default::Colour(c) => {
            req.attr(KEYWORD, "print-color-mode-default", &[if *c { "color" } else { "monochrome" }])
        }
        Default::Quality(q) => req.number(ENUM, "print-quality-default", *q),
        Default::Media(m) => req.attr(KEYWORD, "media-default", &[m]),
        Default::BothSides(b) => req.attr(
            KEYWORD,
            "sides-default",
            &[if *b { "two-sided-long-edge" } else { "one-sided" }],
        ),
    };
    send_ok("/admin/", &req).map(|_| ())
}

/// The events a printing surface follows, as CUPS names them.
const EVENTS: &[&str] = &[
    "job-created",
    "job-completed",
    "job-state-changed",
    "job-progress",
    "printer-added",
    "printer-deleted",
    "printer-modified",
    "printer-state-changed",
];

/// Ask CUPS to say every printing event on the system bus (its `dbus`
/// notifier) for `lease` seconds: the subscription's id.
pub(crate) fn subscribe(lease: u32) -> Result<i32, CupsError> {
    let mut req = Request::new(CREATE_PRINTER_SUBSCRIPTIONS);
    req.attr(URI, "printer-uri", &["ipp://localhost/"]);
    req.attr(NAME, "requesting-user-name", &[&user()]);
    req.group(SUBSCRIPTION);
    req.attr(URI, "notify-recipient-uri", &["dbus://"]);
    req.attr(KEYWORD, "notify-events", EVENTS);
    req.int("notify-lease-duration", lease as i32);
    let r = send_ok("/", &req)?;
    r.first(SUBSCRIPTION)
        .and_then(|g| g.int("notify-subscription-id"))
        .ok_or(CupsError::Garbled)
}

/// End a subscription (one renewed, or the last one at the end).
pub(crate) fn unsubscribe(id: i32) -> Result<(), CupsError> {
    let mut req = Request::new(CANCEL_SUBSCRIPTION);
    req.attr(URI, "printer-uri", &["ipp://localhost/"]);
    req.attr(NAME, "requesting-user-name", &[&user()]);
    req.int("notify-subscription-id", id);
    send_ok("/", &req).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A response as CUPS would send it, built with the request encoder (same
    /// TLV layout) and the status patched in.
    fn response(status: u16, build: impl FnOnce(&mut Request)) -> Vec<u8> {
        let mut r = Request::new(0);
        build(&mut r);
        let mut bytes = r.encode(7);
        bytes[2..4].copy_from_slice(&status.to_be_bytes());
        bytes
    }

    #[test]
    fn a_request_is_ipp_two_with_charset_and_language_first() {
        let mut r = Request::new(GET_JOBS);
        r.attr(KEYWORD, "which-jobs", &["not-completed"]);
        let b = r.encode(9);
        assert_eq!(&b[..8], &[2, 0, 0x00, 0x0A, 0, 0, 0, 9]);
        assert_eq!(b[8], OPERATION);
        assert_eq!(b[9], CHARSET);
        assert_eq!(&b[10..12], &(18u16).to_be_bytes());
        assert_eq!(&b[12..30], b"attributes-charset");
        assert_eq!(*b.last().unwrap(), END);
    }

    #[test]
    fn a_response_reads_back_typed_with_every_value() {
        let bytes = response(0, |r| {
            r.group(0x04);
            r.attr(NAME, "printer-name", &["DeskJet"]);
            r.int("printer-state", 5);
            r.attr(
                KEYWORD,
                "printer-state-reasons",
                &["paused", "media-empty-error"],
            );
            r.int("marker-levels", 60);
        });
        let resp = decode(&bytes).unwrap();
        assert!(resp.ok());
        let p = resp.of(0x04).next().unwrap();
        assert_eq!(p.text("printer-name"), "DeskJet");
        assert_eq!(p.int("printer-state"), Some(5));
        assert_eq!(
            p.texts("printer-state-reasons"),
            vec!["paused", "media-empty-error"]
        );
        assert_eq!(p.ints("marker-levels"), vec![60]);
    }

    #[test]
    fn a_text_with_its_language_is_its_text() {
        // tag, name, value = lang len, "en", text len, "Hola"
        let mut b = vec![
            2,
            0,
            0,
            0,
            0,
            0,
            0,
            1,
            OPERATION,
            TEXT_WITH_LANGUAGE,
            0,
            1,
            b'x',
            0,
            10,
        ];
        b.extend_from_slice(&[0, 2, b'e', b'n', 0, 4, b'H', b'o', b'l', b'a', END]);
        let resp = decode(&b).unwrap();
        assert_eq!(resp.groups[0].text("x"), "Hola");
    }

    #[test]
    fn a_cut_answer_is_no_answer() {
        let bytes = response(0, |r| {
            r.attr(NAME, "printer-name", &["DeskJet"]);
        });
        assert!(decode(&bytes[..bytes.len() - 4]).is_none());
    }

    #[test]
    fn http_answers_come_whole_by_length_or_in_chunks() {
        let plain = b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabcdef";
        assert_eq!(http_body(plain), Some((200, b"abc".to_vec())));
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n";
        assert_eq!(http_body(chunked), Some((200, b"abcde".to_vec())));
        let denied = b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n";
        assert_eq!(http_body(denied), Some((401, Vec::new())));
        // Whole by its length or its last chunk; not before.
        assert!(answer_whole(denied));
        assert!(!answer_whole(&plain[..40]));
        assert!(answer_whole(plain));
        assert!(answer_whole(chunked));
        assert!(!answer_whole(&chunked[..chunked.len() - 5]));
        assert!(!answer_whole(b"HTTP/1.1 200 OK\r\n\r\nabc"));
    }

    #[test]
    fn a_usb_printer_unplugged_is_away() {
        assert_eq!(usb_serial("hp:/usb/DeskJet_1200_series?serial=CN17722320"), Some("CN17722320"));
        assert_eq!(usb_serial("usb://HP/DeskJet%201200%20series?serial=CN1772&interface=1"), Some("CN1772"));
        assert_eq!(usb_serial("ipp://printer.local/ipp/print"), None);
        assert_eq!(usb_serial("usb://HP/DeskJet"), None);
        let dir = std::env::temp_dir().join(format!("golem-usb-{}", std::process::id()));
        let dev = dir.join("3-9");
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(dev.join("serial"), "CN17722320\n").unwrap();
        assert!(!usb_absent("hp:/usb/DeskJet_1200_series?serial=cn17722320", &dir));
        assert!(usb_absent("hp:/usb/Other?serial=XYZ", &dir));
        // Not USB, no serial, no list: not said to be away.
        assert!(!usb_absent("ipp://printer.local/ipp/print", &dir));
        assert!(!usb_absent("hp:/usb/Other", &dir));
        assert!(!usb_absent("hp:/usb/Other?serial=XYZ", &dir.join("none")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_problem_is_what_stops_printing() {
        let r = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        use PrinterProblem::*;
        use PrinterState::*;
        assert_eq!(
            problem_of(Printing, &r(&["media-empty-error"]), ""),
            Some(NoPaper)
        );
        assert_eq!(
            problem_of(Printing, &r(&["media-jam-error"]), ""),
            Some(PaperJam)
        );
        assert_eq!(
            problem_of(Idle, &r(&["toner-empty-error"]), ""),
            Some(NoInk)
        );
        assert_eq!(
            problem_of(Idle, &r(&["cover-open-error"]), ""),
            Some(DoorOpen)
        );
        assert_eq!(
            problem_of(Printing, &r(&["offline-report"]), ""),
            Some(Offline)
        );
        assert_eq!(
            problem_of(Printing, &r(&["connecting-to-device"]), ""),
            None
        );
        // Low ink is said, not a stop.
        assert_eq!(
            problem_of(Idle, &r(&["marker-supply-low-warning"]), ""),
            None
        );
        // Gone away (system-config-printer's pause) vs paused by hand.
        assert_eq!(
            problem_of(Stopped, &r(&["paused"]), "Unplugged or turned off"),
            Some(Offline)
        );
        assert_eq!(problem_of(Stopped, &r(&["paused"]), "Paused"), Some(Paused));
        // A paused printer that is ALSO out of paper: the paper first.
        assert_eq!(
            problem_of(Stopped, &r(&["paused", "media-empty-error"]), ""),
            Some(NoPaper)
        );
        assert_eq!(problem_of(Idle, &[], ""), None);
        assert_eq!(
            problem_of(Idle, &r(&["fuser-over-temp-error"]), ""),
            Some(Other("fuser-over-temp".into()))
        );
        assert_eq!(
            problem_of(Idle, &r(&["cups-waiting-for-job-completed"]), ""),
            None
        );
    }

    #[test]
    fn a_printer_and_a_job_are_read_from_their_groups() {
        let bytes = response(0, |r| {
            r.group(0x04);
            r.attr(NAME, "printer-name", &["DeskJet-1200-series"]);
            r.attr(TEXT, "printer-info", &["HP DeskJet 1200 series"]);
            r.int("printer-state", 3);
            r.attr(KEYWORD, "printer-state-reasons", &["none"]);
            r.int("printer-type", 0x20000 | 0x4);
            r.attr(NAME, "marker-names", &["Black", "Tri-color"]);
            r.attr(NAME, "marker-colors", &["#000000", "#00FFFF#FF00FF#FFFF00"]);
            r.int("marker-levels", 60);
            r.group(0x02);
            r.int("job-id", 12);
            r.attr(
                URI,
                "job-printer-uri",
                &["ipp://localhost/printers/DeskJet-1200-series"],
            );
            r.attr(NAME, "job-name", &["texto.ps"]);
            r.int("job-state", 5);
            r.int("job-media-sheets-completed", 2);
            r.int("time-at-creation", 1_792_621_000);
            r.int("time-at-completed", 0);
        });
        let resp = decode(&bytes).unwrap();
        let p = printer_of(resp.of(0x04).next().unwrap());
        assert!(p.default && p.reasons.is_empty() && p.problem.is_none());
        assert_eq!(p.state, PrinterState::Idle);
        assert_eq!(p.markers.len(), 2);
        assert_eq!(p.markers[0].level, Some(60));
        assert_eq!(p.markers[1].level, None);
        let j = job_of(resp.of(0x02).next().unwrap()).unwrap();
        assert_eq!(
            (j.id, j.printer.as_str(), j.state, j.sheets_done),
            (12, "DeskJet-1200-series", JobState::Printing, 2)
        );
        assert_eq!(j.finished, None);
    }
}
