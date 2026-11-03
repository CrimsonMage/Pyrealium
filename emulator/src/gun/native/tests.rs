use super::frame::*;
use super::receive::Receiver;

#[test]
fn native_xml_envelope_preserves_trailer_and_retail_schema() {
    use super::xml;
    use crate::gun::Elem;
    let element = Elem::new("GetFriendsListRes")
        .attr("Cx", "0x23")
        .attr("HR", "0x0")
        .attr("Cnt", "1")
        .child(
            Elem::new("FriendStatus")
                .attr("User", "Test & Friend")
                .attr("Mode", "1")
                .attr("Location", "Town"),
        );
    let mut bytes = xml::encode(&element).unwrap();
    assert_eq!(xml::decode(&bytes).unwrap().element, element);
    // Synthetic trailer deliberately contains '>' to ensure it is never XML.
    let trailer = *b"private>";
    bytes.extend(trailer);
    let parsed = xml::decode(&bytes).unwrap();
    assert_eq!(parsed.element, element);
    assert_eq!(parsed.trailer, Some(trailer));
    bytes.push(0);
    assert_eq!(xml::decode(&bytes).err(), Some(xml::Error::Trailer));
    assert_eq!(
        xml::decode(b"\x50\xc3<X/>").err(),
        Some(xml::Error::Terminator)
    );
    assert_eq!(
        xml::decode(b"\x50\xc3<X/><Y/>\0").err(),
        Some(xml::Error::Xml)
    );
    assert_eq!(
        xml::decode(b"\x50\xc3<X>\xff</X>\0").err(),
        Some(xml::Error::Utf8)
    );
}
use super::{Error, MAX_MESSAGE};

fn data(bytes: &[u8]) -> DataFrame<'_> {
    match decode(bytes, VERSION_16).unwrap() {
        Frame::Data(frame) => frame,
        _ => panic!("not data"),
    }
}

#[test]
fn connection_layout_and_response_correlation() {
    // Synthetic identifiers; packet structure matches the archived login trace.
    let bytes = [
        0x88, 1, 3, 0, 6, 0, 1, 0, 0x44, 0x33, 0x22, 0x11, 9, 0, 0, 0,
    ];
    let Frame::Connection(connect) = decode(&bytes, VERSION_16).unwrap() else {
        panic!()
    };
    assert_eq!(connect.kind, ConnectionKind::Connect);
    assert_eq!(connect.version, VERSION_16);
    assert_eq!(connect.session, 0x11223344);
    assert_eq!(connect.encode().unwrap(), bytes);
    let reply = Connection {
        kind: ConnectionKind::Connected,
        message_id: 0,
        response_id: connect.message_id,
        version: VERSION_15,
        ..connect
    };
    assert_eq!(&reply.encode().unwrap()[..8], &[0x88, 2, 0, 3, 5, 0, 1, 0]);
    let confirm = Connection {
        poll: false,
        message_id: 4,
        response_id: reply.message_id,
        version: VERSION_16,
        ..reply
    };
    assert_eq!(confirm.encode().unwrap()[0], 0x80);
    assert!(matches!(
        decode(&bytes[..15], VERSION_16),
        Err(Error::Length)
    ));
    assert!(matches!(
        decode(&[0x88, 3, 0, 0], VERSION_16),
        Err(Error::UnsupportedCommand)
    ));
    let invalid = Connection {
        session: 0,
        ..connect
    };
    assert_eq!(invalid.encode(), Err(Error::Session));
}

#[test]
fn sack_is_not_a_sequenced_keepalive_and_preserves_optional_masks() {
    let mut bytes = vec![0x80, 6, 0x1f, 7, 255, 0, 0x11, 0x22, 4, 3, 2, 1];
    for value in [1_u32, 2, 4, 8] {
        bytes.extend(value.to_le_bytes());
    }
    let Frame::Sack(sack) = decode(&bytes, VERSION_16).unwrap() else {
        panic!()
    };
    assert!(sack.response);
    assert_eq!(sack.retry, 7); // nonzero, not a specific bit pattern
    assert_eq!(sack.next_send, 255);
    assert_eq!(sack.next_receive, 0);
    assert_eq!(sack.masks.sack, [Some(1), Some(2)]);
    assert_eq!(sack.masks.send, [Some(4), Some(8)]);
    bytes[6..8].fill(0); // received padding is ignored, emitted padding is zero
    assert_eq!(sack.encode(), bytes);
    for end in 0..bytes.len() {
        assert!(decode(&bytes[..end], VERSION_16).is_err());
    }
}

#[test]
fn dframe_masks_are_not_mistaken_for_application_payload() {
    let mut bytes = vec![0xbf, 0xf1, 10, 11]; // USER_2 is still a DFRAME
    for mask in [7_u32, 8, 9, 10] {
        bytes.extend(mask.to_le_bytes());
    }
    bytes.extend([0x50, 0xc3, b'<', b'X', b'/', b'>']);
    let frame = data(&bytes);
    assert_eq!(frame.masks.sack, [Some(7), Some(8)]);
    assert_eq!(frame.masks.send, [Some(9), Some(10)]);
    assert_eq!(frame.payload, b"\x50\xc3<X/>");
    assert_eq!(frame.control & RETRY, RETRY);
    assert!(matches!(
        decode(&bytes[..19], VERSION_16),
        Err(Error::Truncated)
    ));
}

#[test]
fn keepalive_requires_matching_session_and_is_never_application_data() {
    let bytes = [0x3f, KEEPALIVE, 0, 2, 0x44, 0x33, 0x22, 0x11];
    let frame = data(&bytes);
    assert_eq!(frame.session, Some(0x11223344));
    assert!(frame.payload.is_empty());
    let mut receive = Receiver::new(0, Some(0x11223344));
    assert!(receive.accept(frame).unwrap().messages.is_empty());
    assert_eq!(receive.next_receive(), 1);
    assert_eq!(
        Receiver::new(0, Some(1)).accept(frame).err(),
        Some(Error::Session)
    );
    assert!(decode(&bytes[..4], VERSION_16).is_err());
    let Frame::Data(legacy) = decode(&bytes[..4], 0x00010004).unwrap() else {
        panic!()
    };
    assert_eq!(legacy.session, None); // pre-1.5 bit means correlate, not keepalive
}

#[test]
fn fragmented_message_survives_reordering_retry_and_sequence_rollover() {
    let message = b"\x50\xc3<GetFriendsList Cx=\"0x1\"/>\0";
    let frames = fragment(message, 0, 16, 254, 0).unwrap();
    assert_eq!(frames.len(), 3);
    let mut receive = Receiver::new(254, None);
    assert!(
        receive
            .accept(data(&frames[2]))
            .unwrap()
            .messages
            .is_empty()
    );
    assert!(
        receive
            .accept(data(&frames[0]))
            .unwrap()
            .messages
            .is_empty()
    );
    assert!(receive.accept(data(&frames[0])).unwrap().duplicate);
    let mut retry = frames[2].clone();
    retry[1] |= RETRY;
    retry[3] = 99; // changed ACK on retransmit is legitimate
    assert!(receive.accept(data(&retry)).unwrap().duplicate);
    let delivered = receive.accept(data(&frames[1])).unwrap();
    assert_eq!(delivered.messages.len(), 1);
    assert_eq!(delivered.messages[0].payload, message);
    assert_eq!(delivered.messages[0].fragments, 3);
    assert_eq!(receive.next_receive(), 1);
    assert!(receive.accept(data(&frames[2])).unwrap().duplicate);
}

#[test]
fn no_delivery_across_a_missing_fragment_or_new_message_boundary() {
    let frames = fragment(b"abcdefghijklmnop", 0, 10, 0, 0).unwrap();
    let mut receive = Receiver::new(0, None);
    receive.accept(data(&frames[0])).unwrap();
    receive.accept(data(&frames[2])).unwrap();
    assert_eq!(receive.next_receive(), 1);
    assert_eq!(receive.buffered_frames(), 1);
    assert!(receive.incomplete_message());
    let mut replacement = frames[1].clone();
    replacement[0] |= NEW;
    assert_eq!(
        receive.accept(data(&replacement)).err(),
        Some(Error::MissingEnd)
    );
    assert_eq!(
        receive.accept(data(&frames[1])).err(),
        Some(Error::FailedStream)
    );
    assert_eq!(
        Receiver::new(1, None).accept(data(&frames[1])).err(),
        Some(Error::MissingStart)
    );
}

#[test]
fn conflicting_retries_and_mixed_message_flags_fail_closed() {
    let frames = fragment(b"abcdefghijklmnop", 0x40, 10, 0, 0).unwrap();
    let mut receive = Receiver::new(0, None);
    receive.accept(data(&frames[1])).unwrap();
    let mut conflict = frames[1].clone();
    conflict[4] ^= 1;
    assert_eq!(
        receive.accept(data(&conflict)).err(),
        Some(Error::ConflictingRetry)
    );
    let mut receive = Receiver::new(0, None);
    receive.accept(data(&frames[0])).unwrap();
    let mut changed = frames[1].clone();
    changed[0] ^= 0x40;
    assert_eq!(
        receive.accept(data(&changed)).err(),
        Some(Error::FragmentFlags)
    );
}

#[test]
fn coalesced_payload_alignment_sizes_and_user_flags() {
    // Three headers require two padding bytes; first payload needs one byte.
    let payload = [
        3, 0x46, 4, 6, 1, 7, 0, 0, b'a', b'b', b'c', 0, b'd', b'e', b'f', b'g', b'h',
    ];
    let parts = coalesced(&payload).unwrap();
    assert_eq!(
        parts,
        vec![(0x46, &b"abc"[..]), (6, &b"defg"[..]), (7, &b"h"[..])]
    );
    let mut bytes = vec![0x37, COALESCE, 0, 0];
    bytes.extend(payload);
    let delivered = Receiver::new(0, None).accept(data(&bytes)).unwrap();
    assert_eq!(delivered.messages.len(), 3);
    assert_eq!(delivered.messages[0].user_flags, 0x40);
    let mut large = vec![1, 0x0f, 0, 0];
    large.extend(vec![0; 257]);
    assert_eq!(coalesced(&large).unwrap()[0].1.len(), 257);
    assert!(coalesced(&payload[..payload.len() - 1]).is_err());
    assert!(coalesced(&[0; 64]).is_err());
    assert!(decode(&bytes, 0x00010004).is_err());
}

#[test]
fn receive_window_and_message_memory_are_bounded() {
    let frames = fragment(b"ok", 0, 64, 64, 0).unwrap();
    let mut receive = Receiver::new(0, None);
    assert_eq!(receive.accept(data(&frames[0])).err(), Some(Error::Window));
    assert_eq!(receive.buffered_frames(), 0);
    assert_eq!(
        fragment(&vec![0; MAX_MESSAGE + 1], 0, 1360, 0, 0),
        Err(Error::Limit)
    );
    assert_eq!(fragment(&vec![0; 1000], 0, 5, 0, 0), Err(Error::Limit));
    for sequence in 0..17 {
        let mut frame = vec![0x07 | if sequence == 0 { NEW } else { 0 }, 0, sequence, 0];
        frame.extend(vec![0; 4092]);
        let result = receive.accept(data(&frame));
        if sequence == 16 {
            assert_eq!(result.err(), Some(Error::Limit));
        } else {
            assert!(result.unwrap().messages.is_empty());
        }
    }
}

#[test]
fn close_and_empty_frames_advance_sequence_without_inventing_xml() {
    let mut receive = Receiver::new(0, None);
    let empty = receive.accept(data(&[0x3f, 0, 0, 0])).unwrap();
    assert_eq!(empty.messages.len(), 1);
    assert!(empty.messages[0].payload.is_empty());
    let end = receive
        .accept(data(&[0x3f, KEEPALIVE | END_STREAM, 1, 0, 1, 0, 0, 0]))
        .unwrap();
    assert!(end.closed);
    assert!(end.messages.is_empty());
    assert_eq!(
        receive.accept(data(&[0x3f, 0, 2, 0])).err(),
        Some(Error::FailedStream)
    );
}
