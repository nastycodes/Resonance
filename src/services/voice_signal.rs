use serde::{Deserialize, Serialize};

const VOICE_SIGNAL_PREFIX: &str = "resonance-voice-call-v1:";

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VoiceSignalAction {
    Invite,
    Accept,
    Connected,
    PeerOffer,
    Offer,
    Answer,
    Candidate,
    Roster,
    Decline,
    Leave,
    End,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct VoiceSignal {
    pub call_id: String,
    pub conversation_id: String,
    pub initiator_id: String,
    pub action: VoiceSignalAction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub participants: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<String>,
}

impl VoiceSignal {
    pub fn validate(&self) -> Result<(), String> {
        if self.call_id.len() != 32
            || !self.call_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || self.conversation_id.len() != 32
            || !self
                .conversation_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self.initiator_id.len() != 64
            || !self
                .initiator_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("The received voice call signal is invalid.".into());
        }
        match self.action {
            VoiceSignalAction::Offer | VoiceSignalAction::Answer => {
                let description = self
                    .description
                    .as_deref()
                    .ok_or_else(|| "The WebRTC session description is missing.".to_string())?;
                if description.is_empty() || description.len() > 64 * 1024 {
                    return Err("The WebRTC session description is invalid.".into());
                }
                if self.participants.is_some() {
                    return Err("The WebRTC signal contains unexpected participants.".into());
                }
                if self.candidate.is_some() {
                    return Err("The WebRTC signal contains an unexpected ICE candidate.".into());
                }
            }
            VoiceSignalAction::Candidate => {
                let candidate = self
                    .candidate
                    .as_deref()
                    .ok_or_else(|| "The WebRTC ICE candidate is missing.".to_string())?;
                if candidate.is_empty()
                    || candidate.len() > 4096
                    || serde_json::from_str::<serde_json::Value>(candidate).is_err()
                {
                    return Err("The WebRTC ICE candidate is invalid.".into());
                }
                if self.participants.is_some() || self.description.is_some() {
                    return Err("The WebRTC ICE signal contains unexpected data.".into());
                }
            }
            VoiceSignalAction::Roster => {
                if self.description.is_some() {
                    return Err(
                        "The voice roster contains an unexpected session description.".into(),
                    );
                }
                if self.candidate.is_some() {
                    return Err("The voice roster contains an unexpected ICE candidate.".into());
                }
                let participants = self
                    .participants
                    .as_ref()
                    .ok_or_else(|| "The voice call participant list is missing.".to_string())?;
                if !(2..=12).contains(&participants.len())
                    || !participants.contains(&self.initiator_id)
                    || participants.iter().any(|participant| {
                        participant.len() != 64
                            || !participant.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
                    || participants
                        .iter()
                        .collect::<std::collections::HashSet<_>>()
                        .len()
                        != participants.len()
                {
                    return Err("The voice call participant list is invalid.".into());
                }
            }
            _ if self.participants.is_some() => {
                return Err("The voice call signal contains unexpected participants.".into());
            }
            _ => {}
        }
        if !matches!(
            self.action,
            VoiceSignalAction::Offer | VoiceSignalAction::Answer
        ) && self.description.is_some()
        {
            return Err("The voice call signal contains an unexpected session description.".into());
        }
        if !matches!(
            self.action,
            VoiceSignalAction::Candidate | VoiceSignalAction::Offer | VoiceSignalAction::Answer
        ) && self.candidate.is_some()
        {
            return Err("The voice call signal contains an unexpected ICE candidate.".into());
        }
        Ok(())
    }

    pub fn encode(&self) -> Result<String, String> {
        self.validate()?;
        serde_json::to_string(self)
            .map(|payload| format!("{VOICE_SIGNAL_PREFIX}{payload}"))
            .map_err(|error| format!("Could not encode the voice call signal: {error}"))
    }

    pub fn decode(body: &str) -> Result<Option<Self>, String> {
        let Some(payload) = body.strip_prefix(VOICE_SIGNAL_PREFIX) else {
            return Ok(None);
        };
        let signal: Self = serde_json::from_str(payload)
            .map_err(|error| format!("The received voice call signal is invalid: {error}"))?;
        signal.validate()?;
        Ok(Some(signal))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_call_signals_round_trip_and_reject_invalid_identifiers() {
        let signal = VoiceSignal {
            call_id: "a".repeat(32),
            conversation_id: "c".repeat(32),
            initiator_id: "b".repeat(64),
            action: VoiceSignalAction::Invite,
            participants: None,
            description: None,
            candidate: None,
        };
        let encoded = signal.encode().unwrap();
        assert_eq!(VoiceSignal::decode(&encoded).unwrap(), Some(signal.clone()));
        assert!(VoiceSignal::decode("ordinary message").unwrap().is_none());
        let mut invalid = signal;
        invalid.call_id = "not-a-call-id".into();
        assert!(invalid.encode().is_err());
    }

    #[test]
    fn mesh_rosters_validate_and_round_trip() {
        let initiator = "a".repeat(64);
        let roster = VoiceSignal {
            call_id: "c".repeat(32),
            conversation_id: "d".repeat(32),
            initiator_id: initiator.clone(),
            action: VoiceSignalAction::Roster,
            participants: Some(vec![initiator.clone(), "b".repeat(64)]),
            description: None,
            candidate: None,
        };
        assert_eq!(
            VoiceSignal::decode(&roster.encode().unwrap()).unwrap(),
            Some(roster.clone())
        );

        let mut invalid_roster = roster;
        invalid_roster.participants = Some(vec![initiator.clone(), initiator]);
        assert!(invalid_roster.encode().is_err());
    }

    #[test]
    fn web_rtc_descriptions_are_required_only_for_negotiation_signals() {
        let mut offer = VoiceSignal {
            call_id: "a".repeat(32),
            conversation_id: "b".repeat(32),
            initiator_id: "c".repeat(64),
            action: VoiceSignalAction::Offer,
            participants: None,
            description: Some(r#"{"type":"offer","sdp":"v=0"}"#.into()),
            candidate: None,
        };
        let encoded = offer.encode().unwrap();
        assert_eq!(VoiceSignal::decode(&encoded).unwrap(), Some(offer.clone()));

        offer.description = None;
        assert!(offer.encode().is_err());

        offer.action = VoiceSignalAction::Invite;
        offer.description = Some("unexpected".into());
        assert!(offer.encode().is_err());
    }

    #[test]
    fn trickle_ice_candidates_round_trip_and_reject_invalid_payloads() {
        let candidate = VoiceSignal {
            call_id: "a".repeat(32),
            conversation_id: "b".repeat(32),
            initiator_id: "c".repeat(64),
            action: VoiceSignalAction::Candidate,
            participants: None,
            description: None,
            candidate: Some(
                r#"{"candidate":"candidate:1 1 UDP 1 192.0.2.1 5000 typ host","sdpMid":"0","sdpMLineIndex":0}"#
                    .into(),
            ),
        };
        assert_eq!(
            VoiceSignal::decode(&candidate.encode().unwrap()).unwrap(),
            Some(candidate.clone())
        );

        let mut invalid = candidate;
        invalid.candidate = Some("not-json".into());
        assert!(invalid.encode().is_err());
        invalid.candidate = None;
        assert!(invalid.encode().is_err());
    }
}
