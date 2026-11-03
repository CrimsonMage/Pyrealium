//! Offline-only bridge from capture tools to the real Rust GUN decoder.
//! stdin: JSON lines {stream, hex}, or {reset:true} between capture files.
//! stdout: aggregate allowlisted schema/counts only; never payloads or values.
use pyrealium::gun::{
    Elem,
    native::{
        Error,
        frame::{self, Frame},
        receive::Receiver,
    },
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{self, BufRead, Read};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Input {
    #[serde(default)]
    reset: bool,
    #[serde(default)]
    stream: u32,
    #[serde(default)]
    hex: String,
}

#[derive(Default, Serialize)]
struct Report {
    counts: BTreeMap<String, usize>,
    errors: BTreeMap<String, usize>,
    elements: BTreeMap<String, usize>,
    attributes: BTreeMap<String, usize>,
}
fn inc(map: &mut BTreeMap<String, usize>, key: &str) {
    *map.entry(key.to_owned()).or_default() += 1;
}

struct Stream {
    version: u32,
    session: Option<u32>,
    receiver: Option<Receiver>,
}
impl Default for Stream {
    fn default() -> Self {
        Self {
            version: frame::VERSION_16,
            session: None,
            receiver: None,
        }
    }
}

fn finish(streams: &mut BTreeMap<u32, Stream>, report: &mut Report) {
    for stream in streams.values() {
        if let Some(receive) = &stream.receiver {
            if receive.buffered_frames() > 0 {
                inc(&mut report.counts, "streams_with_sequence_gaps");
            }
            if receive.incomplete_message() {
                inc(&mut report.counts, "incomplete_messages");
            }
        }
    }
    streams.clear();
}

fn schema(elem: &Elem, report: &mut Report) {
    // Whitelist both element and attribute names: even unknown schema names can
    // contain private data. Never serialize arbitrary strings from the capture.
    let allowed: &[&str] = match elem.name.as_str() {
        "Connect" => &["Cx", "ProtoVer"],
        "ConnectRes" => &["Cx", "HR", "Random"],
        "RSAKey" | "CdKey" | "Random" | "Ticket" => &["Val"],
        "ConInfo" => &["Cx", "Locale", "AppGuid"],
        "PresentTicketCmd" => &["Cx", "Email"],
        "PresentTicketRes" => &["Cx", "HR", "Status"],
        "FStart" => &["Cnt"],
        "FriendStatus" | "FriendStat" => &["User", "Mode", "Location"],
        "FriendAdd" | "FriendDelete" | "IgnoredAdd" | "IgnoredDelete" => &["Cx", "User"],
        "GetFriendsList" | "GetIgnoredList" | "ReleaseTicketCmd" => &["Cx"],
        "GetFriendsListRes" | "GetIgnoredListRes" => &["Cx", "HR", "Cnt"],
        "Flags" => &["Cx", "Op", "Word", "Flags"],
        "FlagsRes" => &["Cx", "HR", "WhichFlag", "Flags"],
        "SetLoc" => &["Cx", "Flags", "Location"],
        "ConInfoRes" | "ReleaseTicketRes" | "FriendAddRes" | "FriendDeleteRes"
        | "IgnoredAddRes" | "IgnoredDeleteRes" | "SetLocRes" => &["Cx", "HR"],
        _ => {
            inc(&mut report.counts, "unknown_elements");
            return;
        }
    };
    inc(&mut report.elements, &elem.name);
    for (key, _) in &elem.attrs {
        if allowed.contains(&key.as_str()) {
            inc(&mut report.attributes, &format!("{}.{}", elem.name, key));
        } else {
            inc(&mut report.counts, "unknown_attributes");
        }
    }
    for child in &elem.children {
        schema(child, report);
    }
}

fn process(stream: &mut Stream, bytes: &[u8], report: &mut Report) -> Result<(), Error> {
    inc(&mut report.counts, "datagrams");
    match frame::decode(bytes, stream.version)? {
        Frame::Connection(connection) => {
            inc(
                &mut report.counts,
                match connection.kind {
                    frame::ConnectionKind::Connect => "connect_frames",
                    frame::ConnectionKind::Connected => "connected_frames",
                },
            );
            if stream.session != Some(connection.session) {
                if let Some(receive) = &stream.receiver {
                    if receive.incomplete_message() {
                        inc(&mut report.counts, "incomplete_messages");
                    }
                }
                stream.receiver = Some(Receiver::new(0, Some(connection.session)));
            }
            stream.session = Some(connection.session);
            stream.version = connection.version.min(frame::VERSION_16);
        }
        Frame::Sack(_) => inc(&mut report.counts, "sack_frames"),
        Frame::Data(data) => {
            inc(&mut report.counts, "data_frames");
            if data.control & frame::RETRY != 0 {
                inc(&mut report.counts, "retry_frames");
            }
            if data.session.is_some() {
                inc(&mut report.counts, "keepalive_frames");
            }
            if stream.receiver.is_none() {
                // A capture can begin in the middle of a fragment chain. Its
                // missing prefix is unrecoverable: never pretend it was a message.
                if data.session.is_none() && data.command & frame::NEW == 0 {
                    inc(&mut report.counts, "orphan_fragments");
                    return Ok(());
                }
                stream.receiver = Some(Receiver::new(data.sequence, stream.session));
                inc(&mut report.counts, "streams_started_mid_connection");
            }
            let delivery = stream
                .receiver
                .as_mut()
                .ok_or(Error::FailedStream)?
                .accept(data)?;
            if delivery.duplicate {
                inc(&mut report.counts, "duplicate_frames");
            }
            if delivery.closed {
                inc(&mut report.counts, "closed_streams");
            }
            for message in delivery.messages {
                inc(&mut report.counts, "delivered_messages");
                if message.fragments > 1 {
                    inc(&mut report.counts, "reassembled_messages");
                }
                if message.payload.is_empty() {
                    inc(&mut report.counts, "empty_messages");
                    continue;
                }
                if message.user_flags != 0 {
                    inc(&mut report.counts, "system_messages");
                    continue;
                }
                if !message.payload.starts_with(&[0x50, 0xc3]) {
                    inc(&mut report.counts, "other_application_messages");
                    continue;
                }
                match pyrealium::gun::native::xml::decode(&message.payload) {
                    Ok(xml) => {
                        inc(&mut report.counts, "xml_messages");
                        if xml.trailer.is_some() {
                            inc(&mut report.counts, "xml_with_opaque_trailer");
                        }
                        schema(&xml.element, report);
                    }
                    Err(_) => inc(&mut report.counts, "invalid_xml"),
                }
            }
        }
    }
    Ok(())
}

fn run() -> Result<(), &'static str> {
    let mut streams = BTreeMap::<u32, Stream>::new();
    let mut report = Report::default();
    let mut input = io::stdin().lock();
    loop {
        let mut line = Vec::new();
        let size = input
            .by_ref()
            .take(12289)
            .read_until(b'\n', &mut line)
            .map_err(|_| "input read failed")?;
        if size == 0 {
            break;
        }
        if size > 12288 {
            return Err("input line limit");
        }
        let input: Input = serde_json::from_slice(&line).map_err(|_| "invalid input record")?;
        if input.reset {
            finish(&mut streams, &mut report);
            continue;
        }
        if !streams.contains_key(&input.stream) && streams.len() >= 128 {
            return Err("stream limit");
        }
        if input.hex.len() % 2 != 0 || input.hex.len() > 8192 {
            return Err("datagram limit");
        }
        let bytes = input
            .hex
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let pair = std::str::from_utf8(pair).map_err(|_| "invalid hex")?;
                u8::from_str_radix(pair, 16).map_err(|_| "invalid hex")
            })
            .collect::<Result<Vec<_>, _>>()?;
        if let Err(error) = process(
            streams.entry(input.stream).or_default(),
            &bytes,
            &mut report,
        ) {
            inc(&mut report.errors, &error.to_string());
        }
    }
    finish(&mut streams, &mut report);
    println!(
        "{}",
        serde_json::to_string(&report).map_err(|_| "report encoding failed")?
    );
    Ok(())
}
fn main() {
    if let Err(message) = run() {
        eprintln!("{message}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pyrealium::gun::native::xml;

    #[test]
    fn replay_recovers_fragments_once_and_never_exports_values() {
        let element = Elem::new("PresentTicketCmd")
            .attr("Cx", "private-context")
            .attr("Email", "private-email")
            .child(Elem::new("Ticket").attr("Val", "private-ticket"));
        let payload = xml::encode(&element).unwrap();
        let frames = frame::fragment(&payload, 0, 50, 0, 0).unwrap();
        let mut stream = Stream {
            receiver: Some(Receiver::new(0, None)),
            ..Default::default()
        };
        let mut report = Report::default();
        for bytes in frames.iter().rev() {
            process(&mut stream, bytes, &mut report).unwrap();
        }
        process(&mut stream, &frames[0], &mut report).unwrap();
        assert_eq!(report.counts["xml_messages"], 1);
        assert_eq!(report.counts["reassembled_messages"], 1);
        assert_eq!(report.counts["duplicate_frames"], 1);
        assert_eq!(report.elements["Ticket"], 1);
        assert!(!serde_json::to_string(&report).unwrap().contains("private"));
    }

    #[test]
    fn unknown_schema_names_are_redacted_too() {
        let mut report = Report::default();
        schema(
            &Elem::new("private-element").attr("private-attribute", "private-value"),
            &mut report,
        );
        schema(
            &Elem::new("FriendStat").attr("private-attribute", "private-value"),
            &mut report,
        );
        assert_eq!(report.counts["unknown_elements"], 1);
        assert_eq!(report.counts["unknown_attributes"], 1);
        assert!(!serde_json::to_string(&report).unwrap().contains("private"));
    }

    #[test]
    fn capture_boundaries_discard_partial_messages_and_report_gaps() {
        let frames = frame::fragment(b"abcdefghijklmnop", 0, 10, 0, 0).unwrap();
        let mut streams = BTreeMap::new();
        let mut report = Report::default();
        process(streams.entry(0).or_default(), &frames[0], &mut report).unwrap();
        process(streams.entry(0).or_default(), &frames[2], &mut report).unwrap();
        finish(&mut streams, &mut report);
        assert!(streams.is_empty());
        assert_eq!(report.counts["incomplete_messages"], 1);
        assert_eq!(report.counts["streams_with_sequence_gaps"], 1);
        process(streams.entry(0).or_default(), &frames[1], &mut report).unwrap();
        assert_eq!(report.counts["orphan_fragments"], 1);
        assert!(!report.counts.contains_key("delivered_messages"));
    }
}
