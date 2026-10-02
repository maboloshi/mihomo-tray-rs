//! Minimal mihomo external-controller client built on WinHTTP (a system DLL, so
//! no HTTP crate and no TLS machinery is linked in; the controller is local).

use serde_json::Value;
use windows_sys::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_NO_PROXY, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
    WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable,
    WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetTimeouts,
};

use crate::i18n;
use crate::state::{Group, Provider};

/// How long a liveness probe gets. Probing is not a request: a local controller
/// answers in milliseconds, and a machine where a refused loopback connection
/// takes seconds to come back must not turn every candidate into a stall.
const PROBE_TIMEOUT_MS: u32 = 800;

/// The budget a group test is asked for (`timeout` query parameter). mihomo
/// applies it as one deadline to the whole group, which it tests concurrently,
/// so this is the total the request needs — see `DELAY_TEST_TIMEOUT_MS` in
/// `src/app.rs`, which is the larger one the HTTP side waits with.
const DELAY_QUERY_TIMEOUT_MS: u32 = 3000;

/// The dashboard used when `tray.yml` configures none: the kernel's own
/// `external-ui`, which mihomo serves under this path of the controller.
const DEFAULT_WEB_UI: &str = "http://{host}:{port}/ui/";

#[derive(Clone, Debug)]
pub struct Client {
    pub host: String,
    pub port: u16,
    pub secret: String,
    pub timeout_ms: u32,
}

/// Why an address in `tray.yml` (or in the kernel's own settings) cannot be used.
///
/// The two cases are separate because they ask the user for different things: one
/// is a typo, the other is a scheme this program deliberately does not speak.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientError {
    /// Not a `host:port` this program can build a request for.
    NotAnAddress,
    /// Written with an `https://` scheme. There is no TLS client here, so the
    /// address is refused instead of being silently downgraded to a plaintext
    /// request against a port that would not answer one.
    TlsUnsupported,
}

/// What `GET /proxies` answers with: the groups the menu is built from, and the
/// latency of every entry it listed.
#[derive(Debug, Default)]
pub struct Proxies {
    pub groups: Vec<Group>,
    /// `(name, last measured delay)` for every entry, groups included. A `Vec`
    /// rather than a map: the menu looks a handful of names up per submenu, and a
    /// `HashMap` would link the hashing machinery back into a binary measured in
    /// kilobytes (`docs/DESIGN.md` §11).
    pub latency: Vec<(String, Option<u32>)>,
}

/// What `GET /providers/proxies` answers with: the subscription providers, and
/// the latency of every node they list.
#[derive(Debug, Default)]
pub struct Providers {
    pub providers: Vec<Provider>,
    pub latency: Vec<(String, Option<u32>)>,
}

impl Client {
    /// Accepts `host:port` or `http://host:port`; rejects anything else, `https://`
    /// included.
    pub fn new(address: &str, secret: &str, timeout_ms: u32) -> Result<Self, ClientError> {
        let written = address.trim();
        // Checked before anything else, and by prefix rather than after stripping a
        // scheme: without this the `://` would simply survive into the host, and a
        // TLS controller would be talked to in plaintext.
        if written.to_ascii_lowercase().starts_with("https://") {
            return Err(ClientError::TlsUnsupported);
        }
        let trimmed = written.trim_start_matches("http://").trim_end_matches('/');
        let (host, port) = trimmed.rsplit_once(':').ok_or(ClientError::NotAnAddress)?;
        let host = match host {
            "" | "0.0.0.0" | "::" | "[::]" => "127.0.0.1",
            other => other.trim_matches(|c| c == '[' || c == ']'),
        };
        Ok(Self {
            host: host.to_string(),
            port: port.parse().map_err(|_| ClientError::NotAnAddress)?,
            secret: secret.to_string(),
            timeout_ms: timeout_ms.max(200),
        })
    }

    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    /// The dashboard the tray's "open web UI" entry opens.
    ///
    /// An empty `template` means the kernel's own UI: mihomo mounts the
    /// directory from its `external-ui` setting under `/ui/` of the external
    /// controller, and that is plain http even when the address was written
    /// with a scheme. A template is used as it is, except that `{host}`,
    /// `{port}` and `{secret}` are replaced with this controller's — which is
    /// how a panel hosted elsewhere (zashboard, metacubexd) opens already
    /// pointed at this kernel instead of asking for the address by hand.
    pub fn web_ui_url(&self, template: &str) -> String {
        let template = match template.trim() {
            "" => DEFAULT_WEB_UI,
            template => template,
        };
        template
            .replace("{host}", &self.host)
            .replace("{port}", &self.port.to_string())
            .replace("{secret}", &self.secret)
    }

    // --- raw HTTP ---------------------------------------------------------

    fn http(&self, method: &str, path: &str, body: Option<&str>) -> Result<(u16, String), String> {
        let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
        let method_w = wide(method);
        let path_w = wide(path);
        let host_w = wide(&self.host);
        let agent_w = wide("mihomo-tray");
        let mut headers = String::from("Content-Type: application/json\r\n");
        if !self.secret.is_empty() {
            headers.push_str(&format!("Authorization: Bearer {}\r\n", self.secret));
        }
        let headers_w = wide(&headers);

        unsafe {
            let session = WinHttpOpen(
                agent_w.as_ptr(),
                WINHTTP_ACCESS_TYPE_NO_PROXY,
                std::ptr::null(),
                std::ptr::null(),
                0,
            );
            if session.is_null() {
                return Err(i18n::t().error_winhttp_open.to_string());
            }
            let t = self.timeout_ms as i32;
            WinHttpSetTimeouts(session, t, t, t, t);
            let mut result = Err(i18n::t().error_request_failed.to_string());

            let connect = WinHttpConnect(session, host_w.as_ptr(), self.port, 0);
            if !connect.is_null() {
                let request = WinHttpOpenRequest(
                    connect,
                    method_w.as_ptr(),
                    path_w.as_ptr(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    0,
                );
                if !request.is_null() {
                    let (ptr, len) = match body {
                        Some(b) => (b.as_ptr() as *const core::ffi::c_void, b.len() as u32),
                        None => (std::ptr::null(), 0),
                    };
                    if WinHttpSendRequest(request, headers_w.as_ptr(), u32::MAX, ptr, len, len, 0)
                        != 0
                        && WinHttpReceiveResponse(request, std::ptr::null_mut()) != 0
                    {
                        let mut status: u32 = 0;
                        let mut size = std::mem::size_of::<u32>() as u32;
                        let have_status = WinHttpQueryHeaders(
                            request,
                            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
                            std::ptr::null(),
                            &mut status as *mut u32 as *mut core::ffi::c_void,
                            &mut size,
                            std::ptr::null_mut(),
                        ) != 0;

                        // Bound the body: a wrong or wedged listener on the
                        // controller port must not be able to exhaust memory.
                        const MAX_BODY: usize = 8 * 1024 * 1024;
                        let mut buf: Vec<u8> = Vec::new();
                        let mut overflow = false;
                        loop {
                            let mut available: u32 = 0;
                            if WinHttpQueryDataAvailable(request, &mut available) == 0
                                || available == 0
                            {
                                break;
                            }
                            let remaining = MAX_BODY.saturating_sub(buf.len());
                            if remaining == 0 {
                                overflow = true;
                                break;
                            }
                            let want = (available as usize).min(remaining) as u32;
                            let mut chunk = vec![0u8; want as usize];
                            let mut read: u32 = 0;
                            if WinHttpReadData(
                                request,
                                chunk.as_mut_ptr() as *mut core::ffi::c_void,
                                want,
                                &mut read,
                            ) == 0
                                || read == 0
                            {
                                break;
                            }
                            buf.extend_from_slice(&chunk[..read as usize]);
                        }
                        result = if !have_status {
                            // No status line to report: saying "HTTP 0" would be a
                            // made-up answer about a request that did not complete.
                            Err(i18n::t().error_http_request_failed.to_string())
                        } else if overflow {
                            Err(i18n::t().error_response_too_large.to_string())
                        } else {
                            Ok((status as u16, String::from_utf8_lossy(&buf).into_owned()))
                        };
                    } else {
                        result = Err(i18n::t().error_http_request_failed.to_string());
                    }
                    WinHttpCloseHandle(request);
                }
                WinHttpCloseHandle(connect);
            }
            WinHttpCloseHandle(session);
            result
        }
    }

    fn body_of(&self, method: &str, path: &str, body: Option<&str>) -> Result<String, String> {
        let (status, text) = self.http(method, path, body)?;
        if !(200..300).contains(&status) {
            let detail = text.trim();
            return Err(if detail.is_empty() {
                format!("HTTP {status}")
            } else {
                format!(
                    "HTTP {status}: {}",
                    detail.chars().take(120).collect::<String>()
                )
            });
        }
        Ok(text)
    }

    fn json(&self, path: &str) -> Result<Value, String> {
        let text = self.body_of("GET", path, None)?;
        serde_json::from_str(&text).map_err(|e| i18n::t().error_json_parse(&e.to_string()))
    }

    // --- endpoints -------------------------------------------------------

    /// `GET /` — the cheapest liveness probe.
    ///
    /// It asks its own short question rather than using the timeout real requests
    /// get: a local controller answers in milliseconds, while an address that
    /// accepts a connection and then stalls — or that takes its time to refuse —
    /// must not cost a caller that is waiting for a kernel its request timeout.
    pub fn alive(&self) -> bool {
        let probe = Client {
            timeout_ms: self.timeout_ms.min(PROBE_TIMEOUT_MS),
            ..self.clone()
        };
        matches!(probe.http("GET", "/", None), Ok((200, text)) if text.contains("mihomo"))
    }

    pub fn version(&self) -> Result<String, String> {
        let v = self.json("/version")?;
        Ok(v["version"].as_str().unwrap_or_default().to_string())
    }

    /// `(mode, mixed_port, tun_enabled)`; TUN is the effective value.
    pub fn configs(&self) -> Result<(String, u16, bool), String> {
        let v = self.json("/configs")?;
        let mode = v["mode"].as_str().unwrap_or_default().to_string();
        let mixed_port = v["mixed-port"]
            .as_u64()
            .filter(|port| *port <= u16::MAX as u64)
            .unwrap_or(0) as u16;
        let tun = v["tun"]["enable"].as_bool().unwrap_or(false);
        Ok((mode, mixed_port, tun))
    }

    /// `GET /proxies`: the groups to build the menu from, and the latency of
    /// every entry the controller listed.
    pub fn proxies(&self) -> Result<Proxies, String> {
        let v = self.json("/proxies")?;
        let Some(map) = v["proxies"].as_object() else {
            return Ok(Proxies::default());
        };
        let mut result = Proxies {
            latency: map
                .iter()
                .map(|(name, entry)| (name.clone(), entry_delay(entry)))
                .collect(),
            ..Proxies::default()
        };
        for (name, entry) in map {
            if entry["hidden"].as_bool().unwrap_or(false) {
                continue;
            }
            let Some(members) = entry["all"].as_array() else {
                continue; // a plain node, not a group
            };
            let kind = entry["type"].as_str().unwrap_or_default().to_string();
            result.groups.push(Group {
                name: name.clone(),
                switchable: is_switchable(&kind),
                kind,
                now: entry["now"].as_str().unwrap_or_default().to_string(),
                fixed: entry["fixed"].as_str().unwrap_or_default().to_string(),
                members: members
                    .iter()
                    .filter_map(|m| m.as_str().map(str::to_string))
                    .collect(),
            });
        }
        Ok(result)
    }

    /// The providers behind the subscriptions and the latency of their nodes
    /// (`GET /providers/proxies`).
    ///
    /// These nodes are **not** in `/proxies`: that map carries the built-in
    /// adapters and the groups, while everything a subscription brought lives
    /// here. A menu built only from `/proxies` therefore had no latency to show
    /// for the nodes a user actually picks between.
    ///
    /// The providers themselves are listed in the order the JSON map hands them
    /// over (serde_json's map is sorted by key), not in the configuration order
    /// mihomo read them in — the name is the only key either side agrees on.
    pub fn providers(&self) -> Result<Providers, String> {
        let v = self.json("/providers/proxies")?;
        let mut result = Providers::default();
        let Some(providers) = v["providers"].as_object() else {
            return Ok(result);
        };
        for (name, provider) in providers {
            result.providers.push(Provider {
                name: name.clone(),
                vehicle: provider["vehicleType"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                // A provider that never updated reports Go's zero time, which is
                // two millennia of "not yet" rather than a date to show.
                updated_at: provider["updatedAt"]
                    .as_str()
                    .and_then(rfc3339_epoch)
                    .filter(|seconds| *seconds > 0),
            });
            let Some(nodes) = provider["proxies"].as_array() else {
                continue;
            };
            for node in nodes {
                if let Some(name) = node["name"].as_str() {
                    result.latency.push((name.to_string(), entry_delay(node)));
                }
            }
        }
        Ok(result)
    }

    /// Ask mihomo to fetch one subscription provider again
    /// (`PUT /providers/proxies/{name}`).
    ///
    /// The kernel re-reads the subscription inside the request handler and
    /// answers when it is done (`hub/route/provider.go` calls
    /// `provider.Update()`), so this is an ordinary request that happens to be
    /// slow: mihomo gives its own download a 20 s budget
    /// (`resource.DefaultHttpTimeout`), which is far more than the configured
    /// request timeout. `204` means the provider now holds what the
    /// subscription serves; a source that cannot be read answers `503` with
    /// mihomo's own reason. There is nothing to hand back to the UI: the new
    /// nodes and the new `updatedAt` arrive with the next
    /// [`Client::providers`].
    pub fn refresh_provider(&self, name: &str) -> Result<(), String> {
        let path = format!("/providers/proxies/{}", percent_encode(name));
        self.body_of("PUT", &path, None).map(|_| ())
    }

    /// Let a pinned `URLTest`/`Fallback` group choose for itself again
    /// (`DELETE /proxies/{name}`). `Selector` groups have no pinned state.
    pub fn unfix(&self, group: &str) -> Result<(), String> {
        let path = format!("/proxies/{}", percent_encode(group));
        self.body_of("DELETE", &path, None).map(|_| ())
    }

    pub fn set_mode(&self, mode: &str) -> Result<(), String> {
        let body = format!("{{\"mode\":\"{mode}\"}}");
        self.body_of("PATCH", "/configs", Some(&body)).map(|_| ())
    }

    pub fn set_tun(&self, enable: bool) -> Result<(), String> {
        let body = format!("{{\"tun\":{{\"enable\":{enable}}}}}");
        self.body_of("PATCH", "/configs", Some(&body)).map(|_| ())
    }

    pub fn select(&self, group: &str, member: &str) -> Result<(), String> {
        let path = format!("/proxies/{}", percent_encode(group));
        let body = format!("{{\"name\":\"{}\"}}", escape_json(member));
        self.body_of("PUT", &path, Some(&body)).map(|_| ())
    }

    /// Let mihomo reload its own configuration file: an empty `path` makes the
    /// core fall back to the file it was started with, so we never need to know
    /// where that file is. `force=true` re-creates the inbound listeners.
    pub fn reload(&self) -> Result<(), String> {
        self.body_of("PUT", "/configs?force=true", Some("{\"path\":\"\"}"))
            .map(|_| ())
    }

    /// Ask the kernel to restart itself (`POST /restart`).
    ///
    /// The route answers before it acts: mihomo sends `{"status":"ok"}`, flushes,
    /// and only then shuts the process down in a goroutine (`hub/route/restart.go`).
    /// So a success here says "the request was accepted", never "a kernel is
    /// running again" — the caller has to watch the controller for that.
    ///
    /// On Windows the process re-creates itself with its own image and argv
    /// (`exec.Command` + `os.Exit`), so everything the old process was started
    /// with — the administrator token included — carries over to the new one.
    pub fn restart(&self) -> Result<(), String> {
        self.body_of("POST", "/restart", None).map(|_| ())
    }

    /// Ask the kernel to test every node of a group (`GET /group/{name}/delay`).
    ///
    /// mihomo runs the members **concurrently** under a single deadline of
    /// `timeout` milliseconds (`adapter/outboundgroup/groupbase.go`), so the call
    /// takes about that long however large the group is — and an ordinary request
    /// timeout, which is shorter, would cut it off. It answers `504` when no
    /// member came back at all.
    ///
    /// What this method does not do is pretend the answer is the result: mihomo
    /// writes every measurement into the node's own `history` (delay `0` for a
    /// test that failed), and the next `GET /proxies` is where the numbers come
    /// from, so there is nothing here to hand to the UI.
    ///
    /// `url` is the probe target. A `generate_204`-style endpoint answers with an
    /// empty body; `https` is what mihomo itself defaults to and what its own
    /// source recommends, because a plain-http test address can be hijacked by a
    /// provider and then fails for reasons that have nothing to do with latency.
    pub fn delay(&self, group: &str, url: &str) -> Result<(), String> {
        let path = format!(
            "/group/{}/delay?timeout={}&url={}",
            percent_encode(group),
            DELAY_QUERY_TIMEOUT_MS,
            percent_encode(url)
        );
        // Parsed rather than discarded: a controller that answers 200 with
        // something that is not JSON is worth reporting instead of being read as
        // "the group was tested".
        self.json(&path).map(|_| ())
    }

    /// Close every connection the kernel is proxying (`DELETE /connections`).
    ///
    /// mihomo walks its own connection table, closes each entry and answers `204`
    /// (`hub/route/connections.go`). A connection is not a setting: the next
    /// request through a proxy opens a new one, so this is "drop what is open
    /// now", not a state that stays off — which is also why it needs no
    /// confirmation.
    pub fn close_connections(&self) -> Result<(), String> {
        self.body_of("DELETE", "/connections", None).map(|_| ())
    }
}

/// `/proxies` only exposes the adapter's type name, so mihomo's `SelectAble`
/// set is mirrored here. It is exactly the set `updateProxy` accepts (see
/// `hub/route/proxies.go`): `Selector`, `URLTest` and `Fallback`. `LoadBalance`
/// and the plain node types answer `400 Must be a Selector`.
fn is_switchable(kind: &str) -> bool {
    matches!(kind, "Selector" | "URLTest" | "Fallback")
}

/// The latency mihomo last measured for a node, out of one `/proxies` or
/// `/providers/proxies` entry.
///
/// `history` is a list of samples and only the last one describes the node as it
/// is now; a node that was never tested carries an empty list. mihomo records `0`
/// when the test timed out or was refused, which reaches the menu as "timeout"
/// rather than as a suspicious 0 ms.
fn entry_delay(entry: &Value) -> Option<u32> {
    let delay = entry["history"].as_array()?.last()?["delay"].as_u64()?;
    Some(delay.min(u32::MAX as u64) as u32)
}

/// Seconds since the Unix epoch of an RFC 3339 timestamp, `None` for anything
/// that is not one.
///
/// Hand-written, like the rest of the JSON reading: a date library would dwarf a
/// client measured in kilobytes (`docs/DESIGN.md` §11). mihomo marshals its
/// `updatedAt` with Go's default layout — `2026-10-01T16:40:02.7094989+08:00` —
/// and the offset is subtracted here, so the caller compares two instants and
/// not two wall clocks: the kernel's time zone never enters the answer.
fn rfc3339_epoch(text: &str) -> Option<i64> {
    let (date, rest) = text.split_once('T')?;
    let date = date.as_bytes();
    if date.len() != 10 || date[4] != b'-' || date[7] != b'-' {
        return None;
    }
    let year = digits(&date[0..4])?;
    let month = digits(&date[5..7])?;
    let day = digits(&date[8..10])?;

    let time = rest.as_bytes();
    if time.len() < 8 || time[2] != b':' || time[5] != b':' {
        return None;
    }
    let hour = digits(&time[0..2])?;
    let minute = digits(&time[3..5])?;
    let second = digits(&time[6..8])?;

    // Fractional seconds are not part of the answer and are dropped.
    let mut tail = &time[8..];
    if tail.first() == Some(&b'.') {
        let fraction = tail[1..].iter().take_while(|b| b.is_ascii_digit()).count();
        if fraction == 0 {
            return None;
        }
        tail = &tail[1 + fraction..];
    }
    let offset = match tail {
        [b'Z' | b'z'] => 0,
        [
            sign @ (b'+' | b'-'),
            hours_1,
            hours_2,
            b':',
            minutes_1,
            minutes_2,
        ] => {
            let hours = digits(&[*hours_1, *hours_2])?;
            let minutes = digits(&[*minutes_1, *minutes_2])?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            let seconds = hours as i64 * 3_600 + minutes as i64 * 60;
            if *sign == b'-' { -seconds } else { seconds }
        }
        _ => return None,
    };

    if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year as i64, month) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    Some(
        days_from_civil(year as i64, month, day) * 86_400
            + hour as i64 * 3_600
            + minute as i64 * 60
            + second as i64
            - offset,
    )
}

/// The number an all-digit run spells, `None` for an empty run or any non-digit.
fn digits(bytes: &[u8]) -> Option<u32> {
    if bytes.is_empty() {
        return None;
    }
    let mut value: u32 = 0;
    for byte in bytes {
        value = value * 10 + (*byte as char).to_digit(10)?;
    }
    Some(value)
}

/// Days from 1970-01-01 to `year-month-day` in the proleptic Gregorian calendar
/// (Howard Hinnant's `days_from_civil`: no lookup table, and leap years fall out
/// of the arithmetic instead of being special-cased).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    // March is month 0 here, which is what puts the leap day at the end.
    let month_prime = (month as i64 + 9) % 12;
    let day_of_year = (153 * month_prime + 2) / 5 + day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Percent-encode everything outside the RFC 3986 unreserved set (group names
/// are frequently non-ASCII).
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        let c = *byte as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
            out.push(c);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Escape a member name for embedding into a JSON string literal.
fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn parses_addresses() {
        let c = Client::new("127.0.0.1:9090", "", 2000).unwrap();
        assert_eq!((c.host.as_str(), c.port), ("127.0.0.1", 9090));
        let c = Client::new("http://localhost:9098/", "s", 500).unwrap();
        assert_eq!(
            (c.host.as_str(), c.port, c.secret.as_str()),
            ("localhost", 9098, "s")
        );
        let c = Client::new("0.0.0.0:9090", "", 2000).unwrap();
        assert_eq!(c.host, "127.0.0.1");
        assert_eq!(
            Client::new("no-port", "", 2000).unwrap_err(),
            ClientError::NotAnAddress
        );
    }

    #[test]
    fn an_https_address_is_refused_rather_than_downgraded() {
        // There is no TLS client here, and silently dropping the scheme would send
        // a plaintext request to a port that answers TLS only.
        assert_eq!(
            Client::new("https://127.0.0.1:9090", "", 2000).unwrap_err(),
            ClientError::TlsUnsupported
        );
        assert_eq!(
            Client::new("  HTTPS://127.0.0.1:9090/", "", 2000).unwrap_err(),
            ClientError::TlsUnsupported
        );
        // A scheme this program does speak is still accepted.
        assert!(Client::new("http://127.0.0.1:9090", "", 2000).is_ok());
    }

    #[test]
    fn the_web_ui_url_follows_the_configured_template() {
        let c = Client::new("0.0.0.0:9090", "s3cret", 2000).unwrap();
        // No template: the kernel's own dashboard, next to the controller.
        assert_eq!(c.web_ui_url(""), "http://127.0.0.1:9090/ui/");
        assert_eq!(c.web_ui_url("  "), "http://127.0.0.1:9090/ui/");
        // A hosted panel is pointed at this kernel through the placeholders.
        assert_eq!(
            c.web_ui_url(
                "https://board.zash.run.place/#/setup?hostname={host}&port={port}&secret={secret}"
            ),
            "https://board.zash.run.place/#/setup?hostname=127.0.0.1&port=9090&secret=s3cret"
        );
        // A template without placeholders is opened as it is.
        assert_eq!(
            c.web_ui_url("http://localhost:8080/"),
            "http://localhost:8080/"
        );
    }

    #[test]
    fn encodes_non_ascii_paths() {
        assert_eq!(
            percent_encode("自动选择"),
            "%E8%87%AA%E5%8A%A8%E9%80%89%E6%8B%A9"
        );
        assert_eq!(percent_encode("A B&C"), "A%20B%26C");
        assert_eq!(percent_encode("plain-1_2.3~"), "plain-1_2.3~");
    }

    #[test]
    fn escapes_json_members() {
        assert_eq!(escape_json("a\"b\\c"), "a\\\"b\\\\c");
    }

    // --- fake HTTP server -------------------------------------------------

    /// One request exactly as the fake server received it.
    struct SentRequest {
        method: String,
        path: String,
        /// Raw header block, request line included.
        headers: String,
        body: String,
    }

    /// A single-shot HTTP/1.1 server bound to an ephemeral loopback port.
    struct FakeServer {
        port: u16,
        rx: mpsc::Receiver<SentRequest>,
    }

    impl FakeServer {
        /// `host:port` for `Client::new`.
        fn address(&self) -> String {
            format!("127.0.0.1:{}", self.port)
        }

        /// The request the client sent; panics when none ever arrives.
        fn request(&self) -> SentRequest {
            self.rx
                .recv_timeout(Duration::from_secs(10))
                .expect("fake server received no request")
        }
    }

    /// Serves exactly one request, then answers with `status`/`reason`/`payload`
    /// and closes the connection (`Connection: close`).
    fn spawn_server(status: u16, reason: &str, payload: &str) -> FakeServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
        let port = listener.local_addr().expect("fake server address").port();
        let reason = reason.to_string();
        let payload = payload.to_string();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            stream.set_read_timeout(Some(Duration::from_secs(10))).ok();

            // Read up to and including the blank line that ends the headers.
            let mut raw: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 1024];
            let head_end = loop {
                let read = stream.read(&mut chunk).unwrap_or(0);
                if read == 0 {
                    break 0;
                }
                raw.extend_from_slice(&chunk[..read]);
                if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    break pos + 4;
                }
            };

            let head = String::from_utf8_lossy(&raw[..head_end]).into_owned();
            let mut lines = head.split("\r\n");
            let mut request_line = lines.next().unwrap_or_default().split(' ');
            let method = request_line.next().unwrap_or_default().to_string();
            let path = request_line.next().unwrap_or_default().to_string();
            let content_length = lines
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .unwrap_or(0);

            // Drain the body announced by Content-Length, if any.
            while raw.len() - head_end < content_length {
                let read = stream.read(&mut chunk).unwrap_or(0);
                if read == 0 {
                    break;
                }
                raw.extend_from_slice(&chunk[..read]);
            }
            let body = String::from_utf8_lossy(&raw[head_end..]).into_owned();

            let _ = tx.send(SentRequest {
                method,
                path,
                headers: head,
                body,
            });

            let response = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
                 Content-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
            // Dropping `stream` closes the connection.
        });
        FakeServer { port, rx }
    }

    // --- live WinHTTP round trips -----------------------------------------

    #[test]
    fn configs_parses_mode_port_and_tun() {
        let server = spawn_server(
            200,
            "OK",
            r#"{"mode":"rule","mixed-port":7890,"tun":{"enable":true}}"#,
        );
        let client = Client::new(&server.address(), "", 2000).unwrap();
        assert_eq!(client.configs().unwrap(), ("rule".to_string(), 7890, true));
        let sent = server.request();
        assert_eq!(
            (sent.method.as_str(), sent.path.as_str()),
            ("GET", "/configs")
        );
    }

    #[test]
    fn only_switchable_group_types_are_flagged_as_such() {
        // Mirrors mihomo's `outboundgroup.SelectAble`: `updateProxy` accepts a
        // PUT for these and answers `400 Must be a Selector` for the rest.
        for kind in ["Selector", "URLTest", "Fallback"] {
            assert!(is_switchable(kind), "{kind} accepts PUT /proxies/<name>");
        }
        for kind in ["LoadBalance", "Relay", "Direct", "Reject", ""] {
            assert!(!is_switchable(kind), "{kind} must stay read-only");
        }
    }

    #[test]
    fn latency_comes_from_the_last_history_sample_of_each_entry() {
        let server = spawn_server(
            200,
            "OK",
            r#"{"proxies":{
                "A":{"type":"Shadowsocks","history":[{"time":"t1","delay":90},{"time":"t2","delay":42}]},
                "B":{"type":"Shadowsocks","history":[{"time":"t1","delay":0}]},
                "C":{"type":"Shadowsocks"},
                "Group":{"type":"Selector","all":["A","B","C","Missing"],"now":"A"}
            }}"#,
        );
        let client = Client::new(&server.address(), "", 2000).unwrap();
        let proxies = client.proxies().unwrap();
        let delay = |name: &str| {
            proxies
                .latency
                .iter()
                .find(|(node, _)| node == name)
                .map(|(_, delay)| *delay)
                .expect("entry is listed")
        };

        assert_eq!(
            delay("A"),
            Some(42),
            "the newest sample wins, not the first or the average"
        );
        assert_eq!(delay("B"), Some(0), "a failed test is 0, not a latency");
        assert_eq!(delay("C"), None, "an empty history is not 0ms");
        assert_eq!(
            delay("Group"),
            None,
            "groups carry their own history slot and it is empty here"
        );
        // The groups the menu is built from are still there.
        assert_eq!(proxies.groups.len(), 1);
        assert_eq!(proxies.groups[0].name, "Group");
    }

    #[test]
    fn provider_nodes_carry_the_latency_of_their_last_sample() {
        // These nodes are not in `/proxies` at all: on a current kernel that map
        // holds the built-in adapters and the groups, while a subscription's nodes
        // are only listed here. Reading `/proxies` alone is why the menu had no
        // latency to show.
        let server = spawn_server(
            200,
            "OK",
            r#"{"providers":{
                "MyProvider":{"type":"Proxy","vehicleType":"HTTP","updatedAt":"2026-10-01T16:40:02.7094989+08:00","proxies":[
                    {"name":"tokyo","type":"Trojan","history":[{"delay":125},{"delay":88}]},
                    {"name":"osaka","type":"Trojan","history":[{"delay":0}]},
                    {"name":"nowhere","type":"Trojan"}
                ]},
                "PROXY":{"type":"Proxy","vehicleType":"Compatible","updatedAt":"0001-01-01T00:00:00Z"}
            }}"#,
        );
        let client = Client::new(&server.address(), "", 2000).unwrap();
        let providers = client.providers().unwrap();
        assert_eq!(
            providers.latency,
            vec![
                ("tokyo".to_string(), Some(88)),
                ("osaka".to_string(), Some(0)),
                ("nowhere".to_string(), None),
            ]
        );
        // The provider list rides along on the same request, so the menu can offer
        // a refresh per provider without a second call.
        assert_eq!(
            providers
                .providers
                .iter()
                .map(|provider| (
                    provider.name.as_str(),
                    provider.vehicle.as_str(),
                    provider.updated_at,
                    provider.refreshable()
                ))
                .collect::<Vec<_>>(),
            vec![
                (
                    "MyProvider",
                    "HTTP",
                    Some(1_790_844_002),
                    // 2026-10-01T16:40:02+08:00 == 08:40:02Z on the same day.
                    true
                ),
                // The zero time is "never updated", not a date to render, and a
                // Compatible provider has no source to read again.
                ("PROXY", "Compatible", None, false),
            ]
        );
        let sent = server.request();
        assert_eq!(sent.path, "/providers/proxies");
    }

    #[test]
    fn refreshing_a_provider_puts_its_encoded_name() {
        // mihomo re-downloads the subscription inside the request and answers 204.
        let server = spawn_server(204, "No Content", "");
        let client = Client::new(&server.address(), "", 2000).unwrap();
        client.refresh_provider("我的订阅").unwrap();

        let sent = server.request();
        assert_eq!(sent.method, "PUT");
        assert_eq!(
            sent.path,
            "/providers/proxies/%E6%88%91%E7%9A%84%E8%AE%A2%E9%98%85"
        );
        assert_eq!(sent.body, "");
    }

    #[test]
    fn a_provider_that_cannot_be_read_reports_the_kernels_reason() {
        // The kernel answers 503 with its own error text when the subscription
        // cannot be fetched; that text is the whole answer and must survive.
        let server = spawn_server(
            503,
            "Service Unavailable",
            r#"{"message":"Get \"https://example.invalid/sub\": dial tcp: timeout"}"#,
        );
        let client = Client::new(&server.address(), "", 2000).unwrap();
        let error = client.refresh_provider("MyProvider").unwrap_err();
        assert!(error.contains("503"), "unexpected error: {error}");
        assert!(error.contains("timeout"), "unexpected error: {error}");
    }

    #[test]
    fn parsed_timestamps_are_instants_not_wall_clocks() {
        // The offset is applied, so the same instant written in three zones is
        // the same second: this is what keeps "3 minutes ago" independent of the
        // time zone the kernel runs in.
        assert_eq!(
            rfc3339_epoch("2026-10-01T16:40:02+08:00"),
            Some(1_790_844_002)
        );
        assert_eq!(rfc3339_epoch("2026-10-01T08:40:02Z"), Some(1_790_844_002));
        assert_eq!(
            rfc3339_epoch("2026-10-01T03:40:02-05:00"),
            Some(1_790_844_002)
        );
        // Fractions are dropped, not rounded into the next second.
        assert_eq!(
            rfc3339_epoch("2026-10-01T16:40:02.7094989+08:00"),
            Some(1_790_844_002)
        );
        // Go's zero time, and the epoch itself: both mean "no update ever".
        assert_eq!(rfc3339_epoch("0001-01-01T00:00:00Z"), Some(-62_135_596_800));
        assert_eq!(rfc3339_epoch("1970-01-01T00:00:00Z"), Some(0));
        // A leap day is a real date, and the day before it is one day earlier.
        assert_eq!(rfc3339_epoch("2024-02-29T00:00:00Z"), Some(1_709_164_800));
        assert_eq!(
            rfc3339_epoch("2024-03-01T00:00:00Z"),
            rfc3339_epoch("2024-02-29T00:00:00Z").map(|seconds| seconds + 86_400)
        );
    }

    #[test]
    fn dates_that_are_not_timestamps_are_refused() {
        for text in [
            "",
            "not a date",
            // Missing offset: an instant cannot be read off a wall clock alone.
            "2026-10-01T16:40:02",
            "2026-10-01T16:40:02.",
            "2026-10-01T16:40:02+08",
            "2026-10-01T16:40:02+0800",
            // Out of range fields, including a day February does not have.
            "2026-13-01T00:00:00Z",
            "2026-10-32T00:00:00Z",
            "2023-02-29T00:00:00Z",
            "2026-10-01T24:00:00Z",
            "2026-10-01T16:60:02Z",
            "2026-10-01T16:40:60Z",
            "2026-10-01T16:40:02+24:00",
            // Separators in the wrong places.
            "2026/10/01T16:40:02Z",
            "2026-10-01 16:40:02Z",
            "2026-10-01T16-40-02Z",
        ] {
            assert_eq!(rfc3339_epoch(text), None, "{text:?} is not a timestamp");
        }
    }

    #[test]
    fn delay_asks_the_controller_to_test_the_group() {
        // mihomo answers with a per-member delay map; the tray only needs to know
        // the request was accepted, and reads the numbers back afterwards.
        let server = spawn_server(200, "OK", r#"{"A":42,"B":0}"#);
        let client = Client::new(&server.address(), "", 2000).unwrap();
        client
            .delay("自动选择", "https://www.gstatic.com/generate_204")
            .unwrap();

        let sent = server.request();
        assert_eq!(sent.method, "GET");
        assert_eq!(
            sent.path,
            format!(
                "/group/%E8%87%AA%E5%8A%A8%E9%80%89%E6%8B%A9/delay?timeout={DELAY_QUERY_TIMEOUT_MS}\
                 &url=https%3A%2F%2Fwww.gstatic.com%2Fgenerate_204"
            )
        );
    }

    #[test]
    fn a_group_test_that_does_not_answer_is_reported() {
        // The point of the test: a 200 whose body is not the expected object must
        // not be read as "the group was measured".
        let server = spawn_server(200, "OK", "not json");
        let client = Client::new(&server.address(), "", 2000).unwrap();
        assert!(client.delay("Auto", "http://example.invalid/").is_err());
    }

    #[test]
    fn proxies_returns_only_groups_and_respects_hidden() {
        let server = spawn_server(
            200,
            "OK",
            r#"{"proxies":{
                "DIRECT":{"type":"Direct"},
                "Hidden Group":{"type":"Selector","all":["DIRECT"],"now":"DIRECT","hidden":true},
                "Auto":{"type":"Selector","all":["A","B"],"now":"B"},
                "Fallback":{"type":"URLTest","all":["A","B"],"now":"A","fixed":"A"},
                "Balanced":{"type":"LoadBalance","all":["A","B"]}
            }}"#,
        );
        let client = Client::new(&server.address(), "", 2000).unwrap();
        let groups = client.proxies().unwrap().groups;

        // The plain node and the hidden group are dropped.
        assert_eq!(groups.len(), 3);
        assert!(groups.iter().all(|g| g.name != "DIRECT"));
        assert!(groups.iter().all(|g| g.name != "Hidden Group"));

        let auto = groups
            .iter()
            .find(|g| g.name == "Auto")
            .expect("Auto group");
        assert_eq!(auto.kind, "Selector");
        assert!(auto.switchable);
        assert_eq!(auto.now, "B");
        assert!(auto.fixed.is_empty(), "a Selector has no pinned member");
        assert_eq!(auto.members, vec!["A".to_string(), "B".to_string()]);

        // A URLTest group is selectable too; `fixed` says it is currently pinned.
        let fallback = groups
            .iter()
            .find(|g| g.name == "Fallback")
            .expect("Fallback group");
        assert_eq!(fallback.kind, "URLTest");
        assert!(fallback.switchable);
        assert_eq!(fallback.now, "A");
        assert_eq!(fallback.fixed, "A");
        assert_eq!(fallback.members, vec!["A".to_string(), "B".to_string()]);

        let balanced = groups
            .iter()
            .find(|g| g.name == "Balanced")
            .expect("Balanced group");
        assert!(!balanced.switchable);

        let sent = server.request();
        assert_eq!(
            (sent.method.as_str(), sent.path.as_str()),
            ("GET", "/proxies")
        );
    }

    #[test]
    fn unfix_deletes_the_encoded_group() {
        let server = spawn_server(204, "No Content", "");
        let client = Client::new(&server.address(), "", 2000).unwrap();
        client.unfix("自动选择").unwrap();

        let sent = server.request();
        assert_eq!(sent.method, "DELETE");
        assert_eq!(sent.path, "/proxies/%E8%87%AA%E5%8A%A8%E9%80%89%E6%8B%A9");
    }

    #[test]
    fn reload_forces_the_own_config_path() {
        let server = spawn_server(204, "No Content", "");
        let client = Client::new(&server.address(), "", 2000).unwrap();
        client.reload().unwrap();

        let sent = server.request();
        assert_eq!(sent.method, "PUT");
        assert_eq!(sent.path, "/configs?force=true");
        assert_eq!(sent.body, r#"{"path":""}"#);
    }

    #[test]
    fn restart_posts_to_the_restart_route() {
        // mihomo answers `{"status":"ok"}` with 200 before it restarts.
        let server = spawn_server(200, "OK", r#"{"status":"ok"}"#);
        let client = Client::new(&server.address(), "", 2000).unwrap();
        client.restart().unwrap();

        let sent = server.request();
        assert_eq!(sent.method, "POST");
        assert_eq!(sent.path, "/restart");
        assert_eq!(sent.body, "");
    }

    #[test]
    fn close_connections_deletes_the_connections_route() {
        // mihomo answers `204 No Content` after closing its whole connection table.
        let server = spawn_server(204, "No Content", "");
        let client = Client::new(&server.address(), "", 2000).unwrap();
        client.close_connections().unwrap();

        let sent = server.request();
        assert_eq!(sent.method, "DELETE");
        assert_eq!(sent.path, "/connections");
        assert_eq!(sent.body, "");
    }

    #[test]
    fn select_percent_encodes_the_group_and_escapes_the_member() {
        let server = spawn_server(204, "No Content", "");
        let client = Client::new(&server.address(), "", 2000).unwrap();
        let member = "A \"B\" & C";
        client.select("自动选择", member).unwrap();

        let sent = server.request();
        assert_eq!(sent.method, "PUT");
        assert_eq!(sent.path, "/proxies/%E8%87%AA%E5%8A%A8%E9%80%89%E6%8B%A9");
        let body: serde_json::Value = serde_json::from_str(&sent.body).expect("valid JSON body");
        assert_eq!(body["name"].as_str(), Some(member));
    }

    #[test]
    fn authorization_header_is_sent_only_with_a_secret() {
        let secured = spawn_server(204, "No Content", "");
        let client = Client::new(&secured.address(), "tok", 2000).unwrap();
        client.set_mode("rule").unwrap();
        assert!(
            secured
                .request()
                .headers
                .contains("Authorization: Bearer tok")
        );

        let open = spawn_server(204, "No Content", "");
        let client = Client::new(&open.address(), "", 2000).unwrap();
        client.set_mode("rule").unwrap();
        assert!(!open.request().headers.contains("Authorization"));
    }

    #[test]
    fn http_error_is_reported_with_status_and_body() {
        let server = spawn_server(400, "Bad Request", "Must be a Selector");
        let client = Client::new(&server.address(), "", 2000).unwrap();
        let err = client.set_mode("rule").unwrap_err();
        assert!(err.contains("400"), "unexpected error: {err}");
        assert!(
            err.contains("Must be a Selector"),
            "unexpected error: {err}"
        );
    }
}
