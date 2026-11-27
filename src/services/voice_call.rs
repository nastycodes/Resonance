use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use opus::{Application, Channels, Decoder, Encoder};
use ring::rand::{SecureRandom, SystemRandom};
use rtc::{
    interceptor::Registry,
    media::Sample,
    peer_connection::{
        configuration::{
            RTCConfigurationBuilder,
            interceptor_registry::register_default_interceptors,
            media_engine::{MIME_TYPE_OPUS, MediaEngine},
        },
        sdp::RTCSessionDescription,
        transport::RTCIceServer,
    },
    rtp_transceiver::{
        PayloadType,
        rtp_sender::{
            RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters,
            RtpCodecKind,
        },
    },
};
use tokio::sync::{mpsc as tokio_mpsc, watch};
use webrtc::{
    media_stream::{
        MediaStreamTrack,
        track_local::{TrackLocal, static_sample::TrackLocalStaticSample},
        track_remote::{TrackRemote, TrackRemoteEvent},
    },
    peer_connection::{
        PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCIceCandidateInit,
        RTCPeerConnectionState,
    },
    runtime::{Runtime as WebRtcRuntime, default_runtime},
};
use zeroize::Zeroize;

const VOICE_SAMPLE_RATE: u32 = 16_000;
const VOICE_FRAME_SAMPLES: usize = 320;
const MAX_QUEUED_SAMPLES: usize = VOICE_SAMPLE_RATE as usize / 5;
const OPUS_MAX_PACKET_BYTES: usize = 4_000;
const OPUS_PAYLOAD_TYPE: PayloadType = 111;
struct PeerAudio {
    outgoing: Mutex<VecDeque<i16>>,
    incoming: Mutex<VecDeque<i16>>,
}

impl Drop for PeerAudio {
    fn drop(&mut self) {
        if let Ok(queue) = self.outgoing.get_mut() {
            queue.make_contiguous().zeroize();
        }
        if let Ok(queue) = self.incoming.get_mut() {
            queue.make_contiguous().zeroize();
        }
    }
}

#[derive(Default)]
struct AudioBuffers {
    peers: Mutex<HashMap<String, Arc<PeerAudio>>>,
}

struct VoicePeer {
    connection: Arc<dyn PeerConnection>,
    state: watch::Receiver<RTCPeerConnectionState>,
    ice_candidates: Option<tokio_mpsc::UnboundedReceiver<String>>,
}

pub struct VoiceCall {
    call_id: String,
    conversation_id: String,
    allowed_peers: Vec<String>,
    active: Arc<AtomicBool>,
    muted: Arc<AtomicBool>,
    buffers: Arc<AudioBuffers>,
    authorized_peers: Mutex<HashSet<String>>,
    peers: Mutex<HashMap<String, VoicePeer>>,
    pending_remote_candidates: Mutex<HashMap<String, Vec<RTCIceCandidateInit>>>,
    audio_start: Mutex<()>,
    audio_stop: Mutex<Option<mpsc::Sender<()>>>,
    audio_thread: Mutex<Option<JoinHandle<()>>>,
    runtime: Arc<tokio::runtime::Runtime>,
}

impl VoiceCall {
    pub fn call_id(&self) -> &str {
        &self.call_id
    }

    pub fn conversation_id(&self) -> &str {
        &self.conversation_id
    }

    fn allows_peer(&self, peer_id: &str) -> bool {
        self.allowed_peers.iter().any(|allowed| allowed == peer_id)
    }

    pub fn authorize_peer(&self, peer_id: &str) -> Result<(), String> {
        if !self.active.load(Ordering::Acquire) || !self.allows_peer(peer_id) {
            return Err("The peer is not authorized to join this voice call.".into());
        }
        self.authorized_peers
            .lock()
            .map_err(|_| "Voice call participants are unavailable.".to_string())?
            .insert(peer_id.to_string());
        Ok(())
    }

    pub fn is_conversation(&self, conversation_id: &str) -> bool {
        self.conversation_id == conversation_id
    }

    pub fn has_peer(&self, peer_id: &str) -> bool {
        matches!(self.peers.lock(), Ok(peers) if peers.contains_key(peer_id))
    }

    pub fn peer_state(&self, peer_id: &str) -> Option<watch::Receiver<RTCPeerConnectionState>> {
        self.peers
            .lock()
            .ok()?
            .get(peer_id)
            .map(|peer| peer.state.clone())
    }

    pub fn take_ice_candidates(
        &self,
        peer_id: &str,
    ) -> Result<tokio_mpsc::UnboundedReceiver<String>, String> {
        self.peers
            .lock()
            .map_err(|_| "WebRTC peer sessions are unavailable.".to_string())?
            .get_mut(peer_id)
            .and_then(|peer| peer.ice_candidates.take())
            .ok_or_else(|| "The WebRTC ICE candidate stream is unavailable.".to_string())
    }

    pub fn add_ice_candidate(&self, peer_id: &str, candidate_json: &str) -> Result<(), String> {
        if !self
            .authorized_peers
            .lock()
            .map_err(|_| "Voice call participants are unavailable.".to_string())?
            .contains(peer_id)
        {
            return Err("The peer has not been authorized to join this voice call.".into());
        }
        let candidate: RTCIceCandidateInit = serde_json::from_str(candidate_json)
            .map_err(|error| format!("The received ICE candidate is invalid: {error}"))?;
        if candidate.candidate.len() > 2048 {
            return Err("The received ICE candidate is too large.".into());
        }

        let connection = self
            .peers
            .lock()
            .map_err(|_| "WebRTC peer sessions are unavailable.".to_string())?
            .get(peer_id)
            .map(|peer| peer.connection.clone());
        let mut pending = self
            .pending_remote_candidates
            .lock()
            .map_err(|_| "WebRTC ICE candidates are unavailable.".to_string())?;
        if let Some(connection) = connection
            && self
                .runtime
                .block_on(connection.remote_description())
                .is_some()
        {
            drop(pending);
            return self
                .runtime
                .block_on(connection.add_ice_candidate(candidate))
                .map_err(|error| format!("Could not apply the remote ICE candidate: {error}"));
        }
        let candidates = pending.entry(peer_id.to_string()).or_default();
        if candidates.len() >= 64 {
            return Err("Too many remote ICE candidates are waiting for negotiation.".into());
        }
        candidates.push(candidate);
        Ok(())
    }

    pub fn start(
        call_id: &str,
        conversation_id: &str,
        _local_id: &str,
        allowed_peers: Vec<String>,
    ) -> Result<Arc<Self>, String> {
        if call_id.len() != 32
            || !call_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || conversation_id.len() != 32
            || !conversation_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || allowed_peers.is_empty()
            || allowed_peers.len() > 11
            || allowed_peers
                .iter()
                .any(|peer| peer.len() != 64 || !peer.bytes().all(|byte| byte.is_ascii_hexdigit()))
            || allowed_peers.iter().collect::<HashSet<_>>().len() != allowed_peers.len()
        {
            return Err("The voice call participants are invalid.".into());
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("resonance-webrtc")
            .build()
            .map_err(|error| format!("Could not start the WebRTC runtime: {error}"))?;

        Ok(Arc::new(Self {
            call_id: call_id.to_string(),
            conversation_id: conversation_id.to_string(),
            allowed_peers,
            active: Arc::new(AtomicBool::new(true)),
            muted: Arc::new(AtomicBool::new(false)),
            buffers: Arc::new(AudioBuffers::default()),
            authorized_peers: Mutex::new(HashSet::new()),
            peers: Mutex::new(HashMap::new()),
            pending_remote_candidates: Mutex::new(HashMap::new()),
            audio_start: Mutex::new(()),
            audio_stop: Mutex::new(None),
            audio_thread: Mutex::new(None),
            runtime: Arc::new(runtime),
        }))
    }

    pub fn create_offer(self: &Arc<Self>, peer_id: &str) -> Result<String, String> {
        self.create_offer_with_ice_restart(peer_id, false)
    }

    pub fn create_ice_restart_offer(self: &Arc<Self>, peer_id: &str) -> Result<String, String> {
        self.create_offer_with_ice_restart(peer_id, true)
    }

    fn create_offer_with_ice_restart(
        self: &Arc<Self>,
        peer_id: &str,
        ice_restart: bool,
    ) -> Result<String, String> {
        self.ensure_peer(peer_id)?;
        let peer = self.peer_connection(peer_id)?;
        self.runtime.block_on(async move {
            if ice_restart {
                peer.connection
                    .restart_ice()
                    .await
                    .map_err(|error| format!("Could not restart WebRTC ICE: {error}"))?;
            }
            let offer = peer
                .connection
                .create_offer(None)
                .await
                .map_err(|error| format!("Could not create the WebRTC offer: {error}"))?;
            peer.connection
                .set_local_description(offer)
                .await
                .map_err(|error| format!("Could not set the WebRTC offer: {error}"))?;
            let description = peer
                .connection
                .local_description()
                .await
                .ok_or_else(|| "WebRTC did not produce a local offer.".to_string())?;
            serde_json::to_string(&description)
                .map_err(|error| format!("Could not encode the WebRTC offer: {error}"))
        })
    }

    pub fn accept_offer(self: &Arc<Self>, peer_id: &str, offer: &str) -> Result<String, String> {
        self.ensure_peer(peer_id)?;
        let description: RTCSessionDescription = serde_json::from_str(offer)
            .map_err(|error| format!("The received WebRTC offer is invalid: {error}"))?;
        let peer = self.peer_connection(peer_id)?;
        let call = Arc::clone(self);
        let peer_id = peer_id.to_string();
        self.runtime.block_on(async move {
            peer.connection
                .set_remote_description(description)
                .await
                .map_err(|error| format!("Could not apply the WebRTC offer: {error}"))?;
            call.apply_pending_ice_candidates(&peer_id, peer.connection.clone())
                .await;
            let answer = peer
                .connection
                .create_answer(None)
                .await
                .map_err(|error| format!("Could not create the WebRTC answer: {error}"))?;
            peer.connection
                .set_local_description(answer)
                .await
                .map_err(|error| format!("Could not set the WebRTC answer: {error}"))?;
            let description = peer
                .connection
                .local_description()
                .await
                .ok_or_else(|| "WebRTC did not produce a local answer.".to_string())?;
            serde_json::to_string(&description)
                .map_err(|error| format!("Could not encode the WebRTC answer: {error}"))
        })
    }

    pub fn apply_answer(self: &Arc<Self>, peer_id: &str, answer: &str) -> Result<(), String> {
        let description: RTCSessionDescription = serde_json::from_str(answer)
            .map_err(|error| format!("The received WebRTC answer is invalid: {error}"))?;
        let peer = self.peer_connection(peer_id)?;
        let call = Arc::clone(self);
        let peer_id = peer_id.to_string();
        self.runtime.block_on(async move {
            peer.connection
                .set_remote_description(description)
                .await
                .map_err(|error| format!("Could not apply the WebRTC answer: {error}"))?;
            call.apply_pending_ice_candidates(&peer_id, peer.connection.clone())
                .await;
            Ok(())
        })
    }

    async fn apply_pending_ice_candidates(
        &self,
        peer_id: &str,
        connection: Arc<dyn PeerConnection>,
    ) {
        let candidates = self
            .pending_remote_candidates
            .lock()
            .map(|mut candidates| candidates.remove(peer_id).unwrap_or_default());
        let candidates = match candidates {
            Ok(candidates) => candidates,
            Err(_) => {
                eprintln!("WebRTC ICE candidates are unavailable.");
                return;
            }
        };
        for candidate in candidates {
            if let Err(error) = connection.add_ice_candidate(candidate).await {
                eprintln!("Could not apply a queued ICE candidate: {error}");
            }
        }
    }

    fn peer_connection(&self, peer_id: &str) -> Result<PeerReference, String> {
        let peers = self
            .peers
            .lock()
            .map_err(|_| "WebRTC peer sessions are unavailable.".to_string())?;
        let peer = peers
            .get(peer_id)
            .ok_or_else(|| "The WebRTC peer session is no longer active.".to_string())?;
        Ok(PeerReference {
            connection: peer.connection.clone(),
        })
    }

    fn ensure_peer(self: &Arc<Self>, peer_id: &str) -> Result<(), String> {
        let result = self.ensure_peer_inner(peer_id);
        if result.is_err() {
            self.remove_peer(peer_id);
        }
        result
    }

    fn ensure_peer_inner(self: &Arc<Self>, peer_id: &str) -> Result<(), String> {
        if !self
            .authorized_peers
            .lock()
            .map_err(|_| "Voice call participants are unavailable.".to_string())?
            .contains(peer_id)
        {
            return Err("The peer has not been authorized to join this voice call.".into());
        }
        if !self.active.load(Ordering::Acquire) {
            return Err("This voice call has ended.".into());
        }
        if self.has_peer(peer_id) {
            return Ok(());
        }
        let peer_audio = Arc::new(PeerAudio {
            outgoing: Mutex::new(VecDeque::new()),
            incoming: Mutex::new(VecDeque::new()),
        });
        self.buffers
            .peers
            .lock()
            .map_err(|_| "Voice audio buffers are unavailable.".to_string())?
            .insert(peer_id.to_string(), peer_audio.clone());

        let rtc_runtime = webrtc_runtime()?;
        let (state_tx, state_rx) = watch::channel(RTCPeerConnectionState::New);
        let (candidate_tx, candidate_rx) = tokio_mpsc::unbounded_channel();
        let handler = Arc::new(VoicePeerHandler {
            state: state_tx,
            ice_candidates: candidate_tx,
            audio: peer_audio.clone(),
            runtime: rtc_runtime.clone(),
        });
        let mut media_engine = MediaEngine::default();
        let audio_codec = RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: MIME_TYPE_OPUS.to_owned(),
                clock_rate: 48_000,
                channels: 1,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                rtcp_feedback: vec![],
            },
            payload_type: OPUS_PAYLOAD_TYPE,
        };
        media_engine
            .register_codec(audio_codec.clone(), RtpCodecKind::Audio)
            .map_err(|error| format!("Could not register the Opus codec: {error}"))?;
        let registry = register_default_interceptors(Registry::new(), &mut media_engine)
            .map_err(|error| format!("Could not configure WebRTC media processing: {error}"))?;
        let configuration = RTCConfigurationBuilder::new()
            .with_ice_servers(vec![
                RTCIceServer {
                    urls: vec!["stun:stun.cloudflare.com:3478".to_string()],
                    ..Default::default()
                },
                RTCIceServer {
                    urls: vec!["stun:stun.l.google.com:19302".to_string()],
                    ..Default::default()
                },
            ])
            .build();
        let connection = self.runtime.block_on(async {
            PeerConnectionBuilder::new()
                .with_configuration(configuration)
                .with_media_engine(media_engine)
                .with_interceptor_registry(registry)
                .with_handler(handler)
                .with_runtime(rtc_runtime)
                .with_udp_addrs(vec!["0.0.0.0:0".to_string()])
                .build()
                .await
                .map_err(|error| format!("Could not create the WebRTC peer connection: {error}"))
        })?;

        let mut random_ssrc = [0u8; 4];
        SystemRandom::new()
            .fill(&mut random_ssrc)
            .map_err(|_| "Could not generate a WebRTC audio track identifier.".to_string())?;
        let ssrc = u32::from_be_bytes(random_ssrc).max(1);
        let track = Arc::new(
            TrackLocalStaticSample::new(
                Instant::now(),
                MediaStreamTrack::new(
                    format!("resonance-audio-{peer_id}"),
                    "resonance-audio-track".to_string(),
                    "Resonance voice".to_string(),
                    RtpCodecKind::Audio,
                    vec![RTCRtpEncodingParameters {
                        rtp_coding_parameters: RTCRtpCodingParameters {
                            ssrc: Some(ssrc),
                            ..Default::default()
                        },
                        codec: audio_codec.rtp_codec,
                        ..Default::default()
                    }],
                ),
            )
            .map_err(|error| format!("Could not create the Opus audio track: {error}"))?,
        );
        self.runtime.block_on(async {
            connection
                .add_track(Arc::clone(&track) as Arc<dyn TrackLocal>)
                .await
                .map_err(|error| format!("Could not add the Opus audio track: {error}"))
        })?;

        let peer = VoicePeer {
            connection: Arc::new(connection),
            state: state_rx.clone(),
            ice_candidates: Some(candidate_rx),
        };
        self.peers
            .lock()
            .map_err(|_| "WebRTC peer sessions are unavailable.".to_string())?
            .insert(peer_id.to_string(), peer);
        self.start_audio()?;
        self.start_audio_sender(
            peer_id.to_string(),
            peer_audio,
            track,
            ssrc,
            state_rx.clone(),
        );
        Ok(())
    }

    fn start_audio_sender(
        &self,
        peer_id: String,
        audio: Arc<PeerAudio>,
        track: Arc<TrackLocalStaticSample>,
        ssrc: u32,
        state: watch::Receiver<RTCPeerConnectionState>,
    ) {
        let active = self.active.clone();
        self.runtime.spawn(async move {
            let Ok(mut encoder) =
                Encoder::new(VOICE_SAMPLE_RATE, Channels::Mono, Application::Voip)
            else {
                eprintln!("Could not initialize the Opus encoder for {peer_id}.");
                return;
            };
            let mut ticker = tokio::time::interval(Duration::from_millis(20));
            let mut pcm = [0i16; VOICE_FRAME_SAMPLES];
            let mut encoded = vec![0u8; OPUS_MAX_PACKET_BYTES];
            loop {
                ticker.tick().await;
                if !active.load(Ordering::Acquire) {
                    break;
                }
                if *state.borrow() != RTCPeerConnectionState::Connected {
                    continue;
                }
                if let Ok(mut outgoing) = audio.outgoing.lock() {
                    for sample in &mut pcm {
                        *sample = outgoing.pop_front().unwrap_or(0);
                    }
                } else {
                    break;
                }
                let length = match encoder.encode(&pcm, &mut encoded) {
                    Ok(length) => length,
                    Err(error) => {
                        eprintln!("Could not encode Opus audio for {peer_id}: {error}");
                        continue;
                    }
                };
                let sample = Sample {
                    data: encoded[..length].to_vec().into(),
                    duration: Duration::from_millis(20),
                    ..Sample::new(Instant::now())
                };
                if let Err(error) = track
                    .sample_writer(ssrc, OPUS_PAYLOAD_TYPE)
                    .write_sample(&sample)
                    .await
                {
                    eprintln!("Could not send Opus audio to {peer_id}: {error}");
                    break;
                }
            }
        });
    }

    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Release);
    }

    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Acquire)
    }

    pub fn stop(&self) {
        if !self.active.swap(false, Ordering::AcqRel) {
            return;
        }
        let _start_guard = self
            .audio_start
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Ok(mut peers) = self.peers.lock() {
            for peer in peers.values() {
                let connection = peer.connection.clone();
                if let Err(error) = self
                    .runtime
                    .block_on(async move { connection.close().await })
                {
                    eprintln!("Could not close the WebRTC peer connection: {error}");
                }
            }
            peers.clear();
        }
        if let Ok(mut peers) = self.buffers.peers.lock() {
            peers.clear();
        }
        if let Ok(mut stop) = self.audio_stop.lock()
            && let Some(stop) = stop.take()
        {
            let _ = stop.send(());
        }
        if let Ok(mut audio_thread) = self.audio_thread.lock()
            && let Some(audio_thread) = audio_thread.take()
        {
            let _ = audio_thread.join();
        }
    }

    fn start_audio(self: &Arc<Self>) -> Result<(), String> {
        let _start_guard = self
            .audio_start
            .lock()
            .map_err(|_| "Audio devices are unavailable.".to_string())?;
        if !self.active.load(Ordering::Acquire) {
            return Err("This voice call has ended.".into());
        }
        if self
            .audio_thread
            .lock()
            .map_err(|_| "Audio devices are unavailable.".to_string())?
            .is_some()
        {
            return Ok(());
        }
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let (stop_sender, stop_receiver) = mpsc::channel();
        let buffers = self.buffers.clone();
        let muted = self.muted.clone();
        let active = self.active.clone();
        let audio_thread = thread::Builder::new()
            .name("resonance-voice-audio".into())
            .spawn(move || match create_audio_streams(buffers, muted, active) {
                Ok(streams) => {
                    if ready_sender.send(Ok(())).is_ok() {
                        let _ = stop_receiver.recv();
                    }
                    drop(streams);
                }
                Err(error) => {
                    let _ = ready_sender.send(Err(error));
                }
            })
            .map_err(|error| format!("Could not start audio devices: {error}"))?;
        *self
            .audio_stop
            .lock()
            .map_err(|_| "Audio devices are unavailable.".to_string())? = Some(stop_sender);
        *self
            .audio_thread
            .lock()
            .map_err(|_| "Audio devices are unavailable.".to_string())? = Some(audio_thread);
        let ready = match ready_receiver.recv() {
            Ok(ready) => ready,
            Err(_) => {
                if let Ok(mut audio_thread) = self.audio_thread.lock()
                    && let Some(audio_thread) = audio_thread.take()
                {
                    let _ = audio_thread.join();
                }
                if let Ok(mut stop) = self.audio_stop.lock() {
                    stop.take();
                }
                return Err("The audio device worker stopped unexpectedly.".into());
            }
        };
        if let Err(error) = ready {
            if let Ok(mut audio_thread) = self.audio_thread.lock()
                && let Some(audio_thread) = audio_thread.take()
            {
                let _ = audio_thread.join();
            }
            if let Ok(mut stop) = self.audio_stop.lock() {
                stop.take();
            }
            return Err(error);
        }
        Ok(())
    }

    pub fn remove_peer(&self, peer_id: &str) {
        if let Ok(mut candidates) = self.pending_remote_candidates.lock() {
            candidates.remove(peer_id);
        }
        if let Ok(mut peers) = self.peers.lock()
            && let Some(peer) = peers.remove(peer_id)
            && let Err(error) = self
                .runtime
                .block_on(async move { peer.connection.close().await })
        {
            eprintln!("Could not close the WebRTC peer connection: {error}");
        }
        if let Ok(mut peers) = self.buffers.peers.lock() {
            peers.remove(peer_id);
        }
    }
}

impl Drop for VoiceCall {
    fn drop(&mut self) {
        self.stop();
    }
}

struct PeerReference {
    connection: Arc<dyn PeerConnection>,
}

fn webrtc_runtime() -> Result<Arc<dyn WebRtcRuntime>, String> {
    static RUNTIME: OnceLock<Arc<dyn WebRtcRuntime>> = OnceLock::new();
    if let Some(runtime) = RUNTIME.get() {
        return Ok(runtime.clone());
    }
    let runtime =
        default_runtime().ok_or_else(|| "The WebRTC Tokio runtime is unavailable.".to_string())?;
    let _ = RUNTIME.set(runtime);
    RUNTIME
        .get()
        .cloned()
        .ok_or_else(|| "The WebRTC runtime could not be initialized.".to_string())
}

struct VoicePeerHandler {
    state: watch::Sender<RTCPeerConnectionState>,
    ice_candidates: tokio_mpsc::UnboundedSender<String>,
    audio: Arc<PeerAudio>,
    runtime: Arc<dyn WebRtcRuntime>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for VoicePeerHandler {
    async fn on_ice_candidate(&self, event: webrtc::peer_connection::RTCPeerConnectionIceEvent) {
        match event.candidate.to_json() {
            Ok(candidate) => match serde_json::to_string(&candidate) {
                Ok(candidate) => {
                    if self.ice_candidates.send(candidate).is_err() {
                        eprintln!("The WebRTC ICE candidate receiver is unavailable.");
                    }
                }
                Err(error) => eprintln!("Could not serialize a local ICE candidate: {error}"),
            },
            Err(error) => eprintln!("Could not encode a local WebRTC ICE candidate: {error}"),
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        self.state.send_replace(state);
    }

    async fn on_ice_candidate_error(
        &self,
        event: webrtc::peer_connection::RTCPeerConnectionIceErrorEvent,
    ) {
        eprintln!("WebRTC ICE candidate gathering error: {event:?}");
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let audio = self.audio.clone();
        self.runtime.spawn(Box::pin(async move {
            let Ok(mut decoder) = Decoder::new(VOICE_SAMPLE_RATE, Channels::Mono) else {
                eprintln!("Could not initialize the Opus decoder for an incoming voice track.");
                return;
            };
            while let Some(event) = track.poll().await {
                match event {
                    TrackRemoteEvent::OnRtpPacket(packet) => {
                        let mut decoded = [0i16; VOICE_SAMPLE_RATE as usize * 120 / 1000];
                        match decoder.decode(packet.payload.as_ref(), &mut decoded, false) {
                            Ok(samples) => push_bounded(&audio.incoming, &decoded[..samples]),
                            Err(error) => {
                                eprintln!("Could not decode an incoming Opus packet: {error}");
                            }
                        }
                    }
                    TrackRemoteEvent::OnEnded | TrackRemoteEvent::OnError => break,
                    _ => {}
                }
            }
        }));
    }
}

fn create_audio_streams(
    buffers: Arc<AudioBuffers>,
    muted: Arc<AtomicBool>,
    active: Arc<AtomicBool>,
) -> Result<(cpal::Stream, cpal::Stream), String> {
    let host = cpal::default_host();
    let input = host
        .default_input_device()
        .ok_or_else(|| "No microphone is available.".to_string())?;
    let output = host
        .default_output_device()
        .ok_or_else(|| "No audio output device is available.".to_string())?;
    let input_config = input
        .default_input_config()
        .map_err(|error| format!("Could not read the microphone format: {error}"))?;
    let output_config = output
        .default_output_config()
        .map_err(|error| format!("Could not read the audio output format: {error}"))?;
    let input_channels = input_config.channels() as usize;
    let output_channels = output_config.channels() as usize;
    if input_channels == 0 || output_channels == 0 {
        return Err("The audio device returned an invalid format.".into());
    }
    let input_stream = match input_config.sample_format() {
        cpal::SampleFormat::F32 => build_input_stream::<f32, _>(
            &input,
            &input_config.clone().into(),
            input_channels,
            input_config.sample_rate().0,
            buffers.clone(),
            muted.clone(),
            |sample| (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16,
        ),
        cpal::SampleFormat::F64 => build_input_stream::<f64, _>(
            &input,
            &input_config.clone().into(),
            input_channels,
            input_config.sample_rate().0,
            buffers.clone(),
            muted.clone(),
            |sample| (sample.clamp(-1.0, 1.0) * i16::MAX as f64) as i16,
        ),
        cpal::SampleFormat::I16 => build_input_stream::<i16, _>(
            &input,
            &input_config.clone().into(),
            input_channels,
            input_config.sample_rate().0,
            buffers.clone(),
            muted.clone(),
            |sample| sample,
        ),
        cpal::SampleFormat::I32 => build_input_stream::<i32, _>(
            &input,
            &input_config.clone().into(),
            input_channels,
            input_config.sample_rate().0,
            buffers.clone(),
            muted.clone(),
            |sample| (sample >> 16) as i16,
        ),
        cpal::SampleFormat::U16 => build_input_stream::<u16, _>(
            &input,
            &input_config.clone().into(),
            input_channels,
            input_config.sample_rate().0,
            buffers.clone(),
            muted.clone(),
            |sample| (sample as i32 - 32768) as i16,
        ),
        cpal::SampleFormat::U32 => build_input_stream::<u32, _>(
            &input,
            &input_config.clone().into(),
            input_channels,
            input_config.sample_rate().0,
            buffers.clone(),
            muted.clone(),
            |sample| ((sample as i64 - 2_147_483_648) >> 16) as i16,
        ),
        format => {
            return Err(format!(
                "The microphone format {format:?} is not supported."
            ));
        }
    }
    .map_err(|error| format!("Could not start microphone capture: {error}"))?;

    let output_stream = match output_config.sample_format() {
        cpal::SampleFormat::F32 => build_output_stream::<f32, _>(
            &output,
            &output_config.clone().into(),
            output_channels,
            output_config.sample_rate().0,
            buffers.clone(),
            active.clone(),
            |sample| sample.clamp(-1.0, 1.0),
        ),
        cpal::SampleFormat::F64 => build_output_stream::<f64, _>(
            &output,
            &output_config.clone().into(),
            output_channels,
            output_config.sample_rate().0,
            buffers.clone(),
            active.clone(),
            |sample| sample.clamp(-1.0, 1.0) as f64,
        ),
        cpal::SampleFormat::I16 => build_output_stream::<i16, _>(
            &output,
            &output_config.clone().into(),
            output_channels,
            output_config.sample_rate().0,
            buffers.clone(),
            active.clone(),
            |sample| (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16,
        ),
        cpal::SampleFormat::I32 => build_output_stream::<i32, _>(
            &output,
            &output_config.clone().into(),
            output_channels,
            output_config.sample_rate().0,
            buffers.clone(),
            active.clone(),
            |sample| (sample.clamp(-1.0, 1.0) * i32::MAX as f32) as i32,
        ),
        cpal::SampleFormat::U16 => build_output_stream::<u16, _>(
            &output,
            &output_config.clone().into(),
            output_channels,
            output_config.sample_rate().0,
            buffers.clone(),
            active.clone(),
            |sample| ((sample.clamp(-1.0, 1.0) + 1.0) * 32767.5) as u16,
        ),
        cpal::SampleFormat::U32 => build_output_stream::<u32, _>(
            &output,
            &output_config.clone().into(),
            output_channels,
            output_config.sample_rate().0,
            buffers,
            active,
            |sample| ((sample.clamp(-1.0, 1.0) + 1.0) * 2_147_483_647.5) as u32,
        ),
        format => {
            return Err(format!(
                "The audio output format {format:?} is not supported."
            ));
        }
    }
    .map_err(|error| format!("Could not start audio playback: {error}"))?;
    input_stream
        .play()
        .map_err(|error| format!("Could not start microphone capture: {error}"))?;
    output_stream
        .play()
        .map_err(|error| format!("Could not start audio playback: {error}"))?;
    Ok((input_stream, output_stream))
}

fn build_input_stream<T, F>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    sample_rate: u32,
    buffers: Arc<AudioBuffers>,
    muted: Arc<AtomicBool>,
    convert: F,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample + Copy,
    F: Fn(T) -> i16 + Send + 'static,
{
    let mut phase = 0u64;
    device.build_input_stream(
        config,
        move |input: &[T], _| {
            let muted = muted.load(Ordering::Acquire);
            let mut mono_samples = Vec::with_capacity(input.len() / channels);
            for frame in input.chunks_exact(channels) {
                phase += u64::from(VOICE_SAMPLE_RATE);
                if phase < u64::from(sample_rate) {
                    continue;
                }
                phase -= u64::from(sample_rate);
                mono_samples.push(if muted {
                    0
                } else {
                    (frame
                        .iter()
                        .map(|sample| i32::from(convert(*sample)))
                        .sum::<i32>()
                        / channels as i32) as i16
                });
            }
            if mono_samples.is_empty() {
                return;
            }
            let Ok(peers) = buffers.peers.lock() else {
                return;
            };
            for peer in peers.values() {
                push_bounded(&peer.outgoing, &mono_samples);
            }
        },
        |error| eprintln!("Voice microphone stream error: {error}"),
        None,
    )
}

fn build_output_stream<T, F>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    sample_rate: u32,
    buffers: Arc<AudioBuffers>,
    active: Arc<AtomicBool>,
    convert: F,
) -> Result<cpal::Stream, cpal::BuildStreamError>
where
    T: cpal::SizedSample + Copy,
    F: Fn(f32) -> T + Send + 'static,
{
    let mut phase = 0.0f64;
    let mut current_sample = 0.0f32;
    device.build_output_stream(
        config,
        move |output: &mut [T], _| {
            for frame in output.chunks_mut(channels) {
                if !active.load(Ordering::Acquire) {
                    for sample in frame {
                        *sample = convert(0.0);
                    }
                    continue;
                }
                phase += f64::from(VOICE_SAMPLE_RATE) / f64::from(sample_rate);
                while phase >= 1.0 {
                    current_sample = next_mixed_sample(&buffers);
                    phase -= 1.0;
                }
                let sample = convert(current_sample);
                for channel in frame {
                    *channel = sample;
                }
            }
        },
        |error| eprintln!("Voice playback stream error: {error}"),
        None,
    )
}

fn next_mixed_sample(buffers: &AudioBuffers) -> f32 {
    let Ok(peers) = buffers.peers.lock() else {
        return 0.0;
    };
    let mut mixed = 0i32;
    let mut active_peers = 0i32;
    for peer in peers.values() {
        if let Ok(mut incoming) = peer.incoming.lock()
            && let Some(sample) = incoming.pop_front()
        {
            mixed += i32::from(sample);
            active_peers += 1;
        }
    }
    if active_peers == 0 {
        0.0
    } else {
        (mixed as f32 / active_peers as f32 / i16::MAX as f32 * 0.8).clamp(-1.0, 1.0)
    }
}

fn push_bounded(queue: &Mutex<VecDeque<i16>>, samples: &[i16]) {
    let Ok(mut queue) = queue.lock() else {
        return;
    };
    for sample in samples {
        if queue.len() >= MAX_QUEUED_SAMPLES {
            queue.pop_front();
        }
        queue.push_back(*sample);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_encodes_and_decodes_a_twenty_millisecond_voice_frame() {
        let mut encoder =
            Encoder::new(VOICE_SAMPLE_RATE, Channels::Mono, Application::Voip).unwrap();
        let mut decoder = Decoder::new(VOICE_SAMPLE_RATE, Channels::Mono).unwrap();
        let input = [0i16; VOICE_FRAME_SAMPLES];
        let mut encoded = vec![0; OPUS_MAX_PACKET_BYTES];
        let encoded_length = encoder.encode(&input, &mut encoded).unwrap();
        assert!(encoded_length > 0);

        let mut decoded = [0i16; VOICE_FRAME_SAMPLES];
        let decoded_length = decoder
            .decode(&encoded[..encoded_length], &mut decoded, false)
            .unwrap();
        assert_eq!(decoded_length, VOICE_FRAME_SAMPLES);
    }
}
