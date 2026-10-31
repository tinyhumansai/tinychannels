use super::super::envelope::MediaKind;
use super::*;

#[test]
fn content_parts_round_trip_typed_voice_and_media() {
    let part = ContentPart::VoiceNote {
        voice_note: VoiceNote {
            media: MediaReference {
                opaque_id: Some("note-1".into()),
                file_name: Some("note.ogg".into()),
                kind: MediaKind::Audio,
                ..Default::default()
            },
            transcript: Some(VoiceTranscript {
                text: "hello".into(),
                language: Some("en".into()),
                state: VoiceTranscriptState::Complete,
                source: VoiceTranscriptSource::HostStt,
            }),
            ..Default::default()
        },
    };
    let json = serde_json::to_value(&part).unwrap();
    assert_eq!(
        serde_json::from_value::<ContentPart>(json.clone()).unwrap(),
        part
    );
    assert_eq!(json["kind"], "voice_note");
    assert_eq!(json["voice_note"]["media"]["fileName"], "note.ogg");
}

#[test]
fn outbound_voice_keeps_media_metadata_typed() {
    let voice = OutboundContentPart::Voice {
        voice: OutboundVoice::Media {
            media: MediaReference::default(),
            codec: Some("opus".into()),
            duration_ms: Some(800),
        },
    };
    assert_eq!(
        serde_json::from_slice::<OutboundContentPart>(&serde_json::to_vec(&voice).unwrap())
            .unwrap(),
        voice
    );
}
