use super::*;

impl AppState {
    pub fn start_voice_call(&mut self, cx: &mut Context<Self>) {
        if self.voice_recorder.is_some() {
            self.set_toast(
                Some("Finish the voice note recording before starting a call.".into()),
                cx,
            );
            cx.notify();
            return;
        }
        if self.active_voice_call.is_some() || self.incoming_voice_call.is_some() {
            self.set_toast(Some("Finish the current voice call first.".into()), cx);
            cx.notify();
            return;
        }
        let Some(conversation_id) = self.selected_conversation.clone() else {
            self.set_toast(Some("Select a conversation before calling.".into()), cx);
            cx.notify();
            return;
        };
        let Some(conversation) = self.conversation(&conversation_id).cloned() else {
            self.set_toast(Some("The selected conversation is unavailable.".into()), cx);
            cx.notify();
            return;
        };
        let peers = match self.voice_call_peers(&conversation) {
            Ok(peers) => peers,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let Some(store) = self.local_chat.clone() else {
            self.set_toast(Some("Local identity storage is unavailable.".into()), cx);
            cx.notify();
            return;
        };
        for peer in &peers {
            if let Err(error) = store.ensure_identity_not_changed(peer) {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        }
        let Some(current_user) = self.current_user.as_ref() else {
            self.set_toast(Some("Sign in before starting a voice call.".into()), cx);
            cx.notify();
            return;
        };
        let Some(transport) = self.peer_transport.clone() else {
            self.set_toast(Some("Peer networking is unavailable.".into()), cx);
            cx.notify();
            return;
        };
        let initiator_id = current_user.id.clone();
        let call_id = match new_id() {
            Ok(id) => id,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let peer_ids = peers.iter().map(|peer| peer.id.clone()).collect::<Vec<_>>();
        let eligible_peers = peer_ids.iter().cloned().collect();
        let route_id = voice_route_id(&call_id, &conversation).to_string();
        self.active_voice_call = Some(ActiveVoiceCall {
            call_id: call_id.clone(),
            conversation_id: conversation.id.clone(),
            initiator_id: initiator_id.clone(),
            eligible_peers,
            participants: vec![initiator_id.clone()],
            muted: false,
            connected: false,
            connected_peers: HashSet::new(),
            connecting_peers: HashSet::new(),
            offering_peers: HashSet::new(),
            reconnect_attempts: HashMap::new(),
        });
        let background = cx.background_executor().clone();
        let task_call_id = call_id.clone();
        let group_id = conversation.is_group().then(|| conversation.id.clone());
        cx.spawn(async move |this, cx| {
            let prepared = background
                .spawn(async move {
                    transport.start_voice_call(&task_call_id, &route_id, peer_ids)?;
                    Ok::<_, String>(
                        peers
                            .into_iter()
                            .map(|peer| VoiceSignalTarget {
                                peer,
                                participants: None,
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .await;
            let _ = this.update(cx, |state, cx| match prepared {
                Ok(targets) => {
                    if !state
                        .active_voice_call
                        .as_ref()
                        .is_some_and(|call| call.call_id == call_id)
                    {
                        if let Some(transport) = &state.peer_transport {
                            transport.end_voice_call(&call_id);
                        }
                        return;
                    }
                    state.dispatch_voice_signal_targets(
                        VoiceSignalAction::Invite,
                        &call_id,
                        group_id,
                        initiator_id,
                        targets,
                        cx,
                    );
                    state.expire_unanswered_call(call_id.clone(), cx);
                    cx.notify();
                }
                Err(error) => {
                    if !state
                        .active_voice_call
                        .as_ref()
                        .is_some_and(|call| call.call_id == call_id)
                    {
                        if let Some(transport) = &state.peer_transport {
                            transport.end_voice_call(&call_id);
                        }
                        return;
                    }
                    state.end_voice_call(false, cx);
                    state.set_toast(
                        Some(format!("Could not prepare the voice call: {error}")),
                        cx,
                    );
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub fn start_voice_call_with_friend(&mut self, peer_id: &str, cx: &mut Context<Self>) {
        if !self.friends.iter().any(|friend| friend.id == peer_id) {
            self.set_toast(
                Some("This account is no longer in your friend list.".into()),
                cx,
            );
            cx.notify();
            return;
        }
        self.open_or_create_conversation(peer_id, cx);
        let opened_peer = self
            .selected_conversation
            .as_deref()
            .and_then(|id| self.conversation(id))
            .is_some_and(|conversation| {
                !conversation.is_group() && conversation.user_id == peer_id
            });
        if opened_peer {
            self.start_voice_call(cx);
        }
    }

    pub fn start_voice_call_in_conversation(
        &mut self,
        conversation_id: &str,
        cx: &mut Context<Self>,
    ) {
        if !self
            .conversation(conversation_id)
            .is_some_and(Conversation::is_group)
        {
            self.set_toast(Some("The group conversation is unavailable.".into()), cx);
            cx.notify();
            return;
        }
        self.open_conversation_by_id(conversation_id, cx);
        if self.selected_conversation.as_deref() == Some(conversation_id) {
            self.start_voice_call(cx);
        }
    }

    pub fn accept_voice_call(&mut self, cx: &mut Context<Self>) {
        let Some(incoming) = self.incoming_voice_call.clone() else {
            return;
        };
        if self.voice_recorder.is_some() {
            self.set_toast(
                Some("Finish the voice note recording before accepting the call.".into()),
                cx,
            );
            cx.notify();
            return;
        }
        if self.active_voice_call.is_some() {
            self.set_toast(Some("Finish the current voice call first.".into()), cx);
            cx.notify();
            return;
        }
        let Some(conversation) = self.conversation(&incoming.conversation_id).cloned() else {
            self.incoming_voice_call = None;
            self.set_toast(Some("The calling conversation is unavailable.".into()), cx);
            cx.notify();
            return;
        };
        let Some(current_user) = self.current_user.as_ref() else {
            return;
        };
        let Some(transport) = self.peer_transport.clone() else {
            self.set_toast(Some("Peer networking is unavailable.".into()), cx);
            cx.notify();
            return;
        };
        let caller = self.friend(&incoming.caller_id).cloned().or_else(|| {
            conversation
                .members
                .iter()
                .find(|member| member.id == incoming.caller_id)
                .cloned()
        });
        let Some(caller) = caller else {
            self.incoming_voice_call = None;
            self.set_toast(Some("The caller is no longer available.".into()), cx);
            cx.notify();
            return;
        };
        let Some(store) = self.local_chat.as_ref() else {
            self.set_toast(Some("Local identity storage is unavailable.".into()), cx);
            cx.notify();
            return;
        };
        if let Err(error) = store.ensure_identity_not_changed(&caller) {
            self.incoming_voice_call = None;
            self.set_toast(Some(error), cx);
            cx.notify();
            return;
        }
        let call_peers = match self.voice_call_peers(&conversation) {
            Ok(peers) => peers,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let (api, token) = match self.api_session() {
            Ok(session) => session,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let Some(identity) = self.identity.as_ref() else {
            self.set_toast(Some("Sign in before accepting a voice call.".into()), cx);
            return;
        };
        let private_key = Zeroizing::new(identity.private_key().to_vec());
        let route_id = voice_route_id(&incoming.call_id, &conversation).to_string();
        let local_id = current_user.id.clone();
        let call_id = incoming.call_id.clone();
        let task_call_id = call_id.clone();
        let call_peer_ids = call_peers
            .iter()
            .map(|peer| peer.id.clone())
            .collect::<Vec<_>>();
        let eligible_peers = call_peer_ids.iter().cloned().collect();
        let group_id = conversation.is_group().then(|| conversation.id.clone());
        let initiator_id = incoming.caller_id.clone();
        let caller_id = caller.id.clone();
        self.active_voice_call = Some(ActiveVoiceCall {
            call_id: call_id.clone(),
            conversation_id: conversation.id.clone(),
            initiator_id: initiator_id.clone(),
            eligible_peers,
            participants: vec![local_id, incoming.caller_id.clone()],
            muted: false,
            connected: false,
            connected_peers: HashSet::new(),
            connecting_peers: HashSet::from([caller_id.clone()]),
            offering_peers: HashSet::new(),
            reconnect_attempts: HashMap::new(),
        });
        self.incoming_voice_call = None;
        self.set_toast(Some("Joining the voice call…".into()), cx);
        let background = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let result = background
                .spawn(async move {
                    transport.start_voice_call(&task_call_id, &route_id, call_peer_ids)?;
                    transport.authorize_voice_peer(&task_call_id, &caller_id)?;
                    let targets = vec![VoiceSignalTarget {
                        peer: caller.clone(),
                        participants: None,
                    }];
                    transmit_voice_signal(
                        api,
                        token,
                        private_key,
                        VoiceSignalAction::Accept,
                        task_call_id.clone(),
                        group_id.clone(),
                        initiator_id.clone(),
                        targets,
                        None,
                        None,
                    )
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                if !state
                    .active_voice_call
                    .as_ref()
                    .is_some_and(|call| call.call_id == call_id)
                {
                    return;
                }
                if let Err(error) = result {
                    state.end_voice_call(true, cx);
                    state.set_toast(Some(format!("Could not join the voice call: {error}")), cx);
                } else {
                    state.set_toast(
                        Some("Accepted; waiting for a direct ICE connection…".into()),
                        cx,
                    );
                    state.expire_voice_connection(call_id.clone(), cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub fn decline_voice_call(&mut self, cx: &mut Context<Self>) {
        let Some(incoming) = self.incoming_voice_call.take() else {
            return;
        };
        if let Some(conversation) = self.conversation(&incoming.conversation_id).cloned()
            && let Ok(peers) = self.voice_call_peers(&conversation)
        {
            self.dispatch_voice_signal(
                VoiceSignalAction::Decline,
                &incoming.call_id,
                &conversation,
                incoming.caller_id,
                peers,
                cx,
            );
        }
        self.set_toast(Some("Call declined.".into()), cx);
        cx.notify();
    }

    pub fn end_voice_call(&mut self, notify_peers: bool, cx: &mut Context<Self>) {
        let Some(call) = self.active_voice_call.take() else {
            return;
        };
        if let Some(transport) = &self.peer_transport {
            transport.end_voice_call(&call.call_id);
        }
        if notify_peers
            && let Some(conversation) = self.conversation(&call.conversation_id).cloned()
            && let Ok(peers) = self.voice_call_peers(&conversation)
        {
            let action = if self
                .current_user
                .as_ref()
                .is_some_and(|user| user.id == call.initiator_id)
            {
                VoiceSignalAction::End
            } else {
                VoiceSignalAction::Leave
            };
            self.dispatch_voice_signal(
                action,
                &call.call_id,
                &conversation,
                call.initiator_id,
                peers,
                cx,
            );
        }
        cx.notify();
    }

    pub fn toggle_voice_call_mute(&mut self, cx: &mut Context<Self>) {
        let Some(call) = self.active_voice_call.as_mut() else {
            return;
        };
        call.muted = !call.muted;
        let call_id = call.call_id.clone();
        let muted = call.muted;
        let result = self
            .peer_transport
            .as_ref()
            .ok_or_else(|| "Peer networking is unavailable.".to_string())
            .and_then(|transport| transport.set_voice_call_muted(&call_id, muted));
        if let Err(error) = result {
            self.set_toast(Some(error), cx);
        }
        cx.notify();
    }

    pub(super) fn expire_unanswered_call(&self, call_id: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(90))
                .await;
            let _ = this.update(cx, |state, cx| {
                if state
                    .active_voice_call
                    .as_ref()
                    .is_some_and(|call| call.call_id == call_id && call.participants.len() == 1)
                {
                    state.end_voice_call(true, cx);
                    state.set_toast(Some("No one answered the voice call.".into()), cx);
                }
            });
        })
        .detach();
    }

    pub(super) fn expire_voice_connection(&self, call_id: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(90))
                .await;
            let _ = this.update(cx, |state, cx| {
                if state
                    .active_voice_call
                    .as_ref()
                    .is_some_and(|call| call.call_id == call_id && !call.connected)
                {
                    state.end_voice_call(true, cx);
                    state.set_toast(
                        Some("The direct voice connection could not be established.".into()),
                        cx,
                    );
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn expire_incoming_call(&self, call_id: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_secs(90))
                .await;
            let _ = this.update(cx, |state, cx| {
                if state
                    .incoming_voice_call
                    .as_ref()
                    .is_some_and(|call| call.call_id == call_id)
                {
                    state.incoming_voice_call = None;
                    state.set_toast(Some("The incoming call expired.".into()), cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(super) fn voice_call_peers(
        &self,
        conversation: &Conversation,
    ) -> Result<Vec<User>, String> {
        if conversation.is_group() {
            let current_user_id = self
                .current_user
                .as_ref()
                .map(|user| user.id.as_str())
                .ok_or_else(|| "Sign in before starting a voice call.".to_string())?;
            let peers = conversation
                .members
                .iter()
                .filter(|member| member.id != current_user_id)
                .map(|member| {
                    self.friend(&member.id)
                        .cloned()
                        .unwrap_or_else(|| member.clone())
                })
                .collect::<Vec<_>>();
            if peers.len() < 2 {
                return Err("A group voice call needs at least two other members.".into());
            }
            Ok(peers)
        } else {
            self.friend(&conversation.user_id)
                .cloned()
                .map(|peer| vec![peer])
                .ok_or_else(|| "This account is no longer in your friend list.".into())
        }
    }

    pub(super) fn dispatch_voice_signal(
        &mut self,
        action: VoiceSignalAction,
        call_id: &str,
        conversation: &Conversation,
        initiator_id: String,
        peers: Vec<User>,
        cx: &mut Context<Self>,
    ) {
        let group_id = conversation.is_group().then(|| conversation.id.clone());
        let targets = peers
            .into_iter()
            .map(|peer| VoiceSignalTarget {
                peer,
                participants: None,
            })
            .collect();
        self.dispatch_voice_signal_targets(action, call_id, group_id, initiator_id, targets, cx);
    }

    pub(super) fn broadcast_voice_roster(
        &mut self,
        call_id: &str,
        conversation: &Conversation,
        initiator_id: String,
        participants: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let participant_ids = participants.iter().cloned().collect::<HashSet<_>>();
        let Some(local_id) = self.current_user.as_ref().map(|user| user.id.clone()) else {
            return;
        };
        let peers = match self.voice_call_peers(conversation) {
            Ok(peers) => peers,
            Err(error) => {
                self.set_toast(Some(error), cx);
                return;
            }
        };
        let targets = peers
            .into_iter()
            .filter(|peer| peer.id != local_id && participant_ids.contains(&peer.id))
            .map(|peer| VoiceSignalTarget {
                peer,
                participants: Some(participants.clone()),
            })
            .collect();
        self.dispatch_voice_signal_targets(
            VoiceSignalAction::Roster,
            call_id,
            Some(conversation.id.clone()),
            initiator_id,
            targets,
            cx,
        );
    }

    pub(super) fn begin_mesh_links(
        &mut self,
        call_id: &str,
        participants: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(local_id) = self.current_user.as_ref().map(|user| user.id.clone()) else {
            return;
        };
        let Some(call) = self
            .active_voice_call
            .as_mut()
            .filter(|call| call.call_id == call_id)
        else {
            return;
        };
        if !participants.contains(&local_id) {
            return;
        }
        call.participants = participants.clone();
        let peer_ids = participants
            .iter()
            .filter(|peer_id| {
                peer_id.as_str() != local_id.as_str()
                    && should_initiate_voice_mesh_link(&local_id, peer_id)
                    && !call.connected_peers.contains(*peer_id)
                    && !call.connecting_peers.contains(*peer_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        if peer_ids.is_empty() {
            cx.notify();
            return;
        }
        let Some(conversation) = self
            .active_voice_call
            .as_ref()
            .and_then(|call| self.conversation(&call.conversation_id))
            .filter(|conversation| conversation.is_group())
            .cloned()
        else {
            return;
        };
        let peers = peer_ids
            .iter()
            .filter_map(|peer_id| {
                conversation
                    .members
                    .iter()
                    .find(|member| &member.id == peer_id)
                    .cloned()
                    .or_else(|| self.friend(peer_id).cloned())
            })
            .collect::<Vec<_>>();
        if peers.len() != peer_ids.len() {
            self.set_toast(
                Some("A group participant is unavailable for a direct voice connection.".into()),
                cx,
            );
            return;
        }
        if let Some(call) = self
            .active_voice_call
            .as_mut()
            .filter(|call| call.call_id == call_id)
        {
            for peer_id in &peer_ids {
                if should_initiate_voice_mesh_link(&local_id, peer_id) {
                    call.connecting_peers.insert(peer_id.clone());
                }
            }
        }
        let initiator_id = self
            .active_voice_call
            .as_ref()
            .map(|call| call.initiator_id.clone())
            .unwrap_or_default();
        let targets = peers
            .into_iter()
            .map(|peer| VoiceSignalTarget {
                peer,
                participants: None,
            })
            .collect();
        self.dispatch_voice_signal_targets(
            VoiceSignalAction::PeerOffer,
            call_id,
            Some(conversation.id),
            initiator_id,
            targets,
            cx,
        );
    }

    pub(super) fn accept_mesh_offer(
        &mut self,
        sender_id: String,
        signal: VoiceSignal,
        local_conversation_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(local_id) = self.current_user.as_ref().map(|user| user.id.clone()) else {
            return;
        };
        let Some(conversation) = self
            .conversation(&local_conversation_id)
            .filter(|conversation| conversation.is_group())
            .cloned()
        else {
            return;
        };
        let is_eligible = self.active_voice_call.as_ref().is_some_and(|call| {
            call.call_id == signal.call_id
                && call.conversation_id == local_conversation_id
                && call.initiator_id == signal.initiator_id
                && call.participants.contains(&sender_id)
                && should_initiate_voice_mesh_link(&sender_id, &local_id)
                && !call.connected_peers.contains(&sender_id)
                && !call.connecting_peers.contains(&sender_id)
        });
        if !is_eligible {
            return;
        }
        let peer = conversation
            .members
            .iter()
            .find(|member| member.id == sender_id)
            .cloned()
            .or_else(|| self.friend(&sender_id).cloned());
        let Some(peer) = peer else {
            return;
        };
        if let Some(call) = self.active_voice_call.as_mut() {
            call.connecting_peers.insert(sender_id.clone());
        }
        let call_id = signal.call_id;
        let initiator_id = signal.initiator_id;
        let group_id = conversation.id.clone();
        let Some(transport) = self.peer_transport.clone() else {
            if let Some(call) = self.active_voice_call.as_mut() {
                call.connecting_peers.remove(&sender_id);
            }
            self.set_toast(Some("Peer networking is unavailable.".into()), cx);
            return;
        };
        if let Err(error) = transport.authorize_voice_peer(&call_id, &sender_id) {
            if let Some(call) = self.active_voice_call.as_mut() {
                call.connecting_peers.remove(&sender_id);
            }
            self.set_toast(Some(error), cx);
            return;
        }
        self.dispatch_voice_signal_targets(
            VoiceSignalAction::Accept,
            &call_id,
            Some(group_id.clone()),
            initiator_id.clone(),
            vec![VoiceSignalTarget {
                peer: peer.clone(),
                participants: None,
            }],
            cx,
        );
        cx.notify();
    }

    pub(super) fn dispatch_voice_signal_targets(
        &mut self,
        action: VoiceSignalAction,
        call_id: &str,
        group_id: Option<String>,
        initiator_id: String,
        targets: Vec<VoiceSignalTarget>,
        cx: &mut Context<Self>,
    ) {
        self.dispatch_voice_signal_targets_with_description(
            action,
            call_id,
            group_id,
            initiator_id,
            targets,
            None,
            None,
            cx,
        );
    }

    pub(super) fn dispatch_voice_signal_targets_with_description(
        &mut self,
        action: VoiceSignalAction,
        call_id: &str,
        group_id: Option<String>,
        initiator_id: String,
        targets: Vec<VoiceSignalTarget>,
        description: Option<String>,
        candidate: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let attempted_peers = if matches!(
            action,
            VoiceSignalAction::Accept
                | VoiceSignalAction::PeerOffer
                | VoiceSignalAction::Offer
                | VoiceSignalAction::Answer
        ) {
            targets
                .iter()
                .map(|target| target.peer.id.clone())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let (api, token) = match self.api_session() {
            Ok(session) => session,
            Err(error) => {
                self.clear_voice_peer_attempts(call_id, &attempted_peers);
                if action == VoiceSignalAction::Candidate {
                    eprintln!("Could not prepare an ICE candidate signal: {error}");
                } else {
                    self.set_toast(Some(error), cx);
                }
                return;
            }
        };
        let Some(identity) = self.identity.as_ref() else {
            self.clear_voice_peer_attempts(call_id, &attempted_peers);
            if action == VoiceSignalAction::Candidate {
                eprintln!("Could not send an ICE candidate while signed out.");
            } else {
                self.set_toast(Some("Sign in before sending call signals.".into()), cx);
            }
            return;
        };
        let private_key = Zeroizing::new(identity.private_key().to_vec());
        let call_id = call_id.to_string();
        let task_call_id = call_id.clone();
        let background = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let result = background
                .spawn(async move {
                    transmit_voice_signal(
                        api,
                        token,
                        private_key,
                        action,
                        task_call_id,
                        group_id,
                        initiator_id,
                        targets,
                        description,
                        candidate,
                    )
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                if let Err(error) = result {
                    state.clear_voice_peer_attempts(&call_id, &attempted_peers);
                    if action == VoiceSignalAction::Candidate {
                        eprintln!("Could not send a local voice ICE candidate: {error}");
                    } else {
                        state.set_toast(Some(error), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn clear_voice_peer_attempts(&mut self, call_id: &str, peer_ids: &[String]) {
        if let Some(call) = self
            .active_voice_call
            .as_mut()
            .filter(|call| call.call_id == call_id)
        {
            for peer_id in peer_ids {
                call.connecting_peers.remove(peer_id);
                call.offering_peers.remove(peer_id);
            }
        }
        if let Some(transport) = &self.peer_transport {
            for peer_id in peer_ids {
                if let Err(error) = transport.remove_voice_peer(call_id, peer_id) {
                    eprintln!("Could not clean up a failed direct voice attempt: {error}");
                }
            }
        }
    }

    pub(super) fn connect_voice_peer(
        &mut self,
        peer: User,
        call_id: String,
        initiator_id: String,
        group_id: Option<String>,
        ice_restart: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(transport) = self.peer_transport.clone() else {
            self.set_toast(Some("Peer networking is unavailable.".into()), cx);
            return;
        };
        let Some(call) = self
            .active_voice_call
            .as_mut()
            .filter(|call| call.call_id == call_id)
        else {
            return;
        };
        if call.connected_peers.contains(&peer.id)
            || (call.offering_peers.contains(&peer.id) && !ice_restart)
        {
            return;
        }
        call.connecting_peers.insert(peer.id.clone());
        call.offering_peers.insert(peer.id.clone());

        let peer_id = peer.id.clone();
        let task_call_id = call_id.clone();
        let background = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let cleanup_peer_id = peer_id.clone();
            let cleanup_call_id = task_call_id.clone();
            let result = background
                .spawn(async move {
                    let result = if ice_restart {
                        transport.create_voice_ice_restart_offer(&peer_id, &task_call_id)
                    } else {
                        transport.create_voice_offer(&peer_id, &task_call_id)
                    };
                    if result.is_err()
                        && let Err(error) =
                            transport.remove_voice_peer(&cleanup_call_id, &cleanup_peer_id)
                    {
                        eprintln!("Could not clean up an incomplete WebRTC offer: {error}");
                    }
                    result
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                if !state
                    .active_voice_call
                    .as_ref()
                    .is_some_and(|call| call.call_id == call_id)
                {
                    if let Some(transport) = &state.peer_transport {
                        if let Err(error) = transport.remove_voice_peer(&call_id, &peer.id) {
                            eprintln!("Could not clean up an abandoned WebRTC offer: {error}");
                        }
                    }
                    return;
                }
                match result {
                    Ok(description) => {
                        state.dispatch_voice_signal_targets_with_description(
                            VoiceSignalAction::Offer,
                            &call_id,
                            group_id.clone(),
                            initiator_id.clone(),
                            vec![VoiceSignalTarget {
                                peer: peer.clone(),
                                participants: None,
                            }],
                            Some(description),
                            None,
                            cx,
                        );
                        if !ice_restart {
                            state.monitor_voice_ice_candidates(
                                peer.clone(),
                                call_id.clone(),
                                initiator_id.clone(),
                                group_id.clone(),
                                cx,
                            );
                        }
                        state.monitor_voice_peer_connection(
                            peer.clone(),
                            call_id.clone(),
                            initiator_id.clone(),
                            group_id.clone(),
                            cx,
                        );
                    }
                    Err(error) => {
                        if let Some(call) = state
                            .active_voice_call
                            .as_mut()
                            .filter(|call| call.call_id == call_id)
                        {
                            call.connecting_peers.remove(&peer.id);
                            call.offering_peers.remove(&peer.id);
                        }
                        state.set_toast(
                            Some(format!(
                                "Could not prepare a direct WebRTC connection. Restrictive NAT or firewall rules may prevent direct ICE: {error}"
                            )),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn monitor_voice_peer_connection(
        &mut self,
        peer: User,
        call_id: String,
        initiator_id: String,
        group_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(transport) = self.peer_transport.clone() else {
            return;
        };
        let mut state_rx = match transport.voice_peer_state(&peer.id, &call_id) {
            Ok(state_rx) => state_rx,
            Err(error) => {
                self.set_toast(Some(error), cx);
                return;
            }
        };
        if *state_rx.borrow() == webrtc::peer_connection::RTCPeerConnectionState::Connected {
            self.handle_voice_peer_state(
                &peer,
                &call_id,
                &initiator_id,
                group_id.clone(),
                webrtc::peer_connection::RTCPeerConnectionState::Connected,
                cx,
            );
        }
        cx.spawn(async move |this, cx| {
            while state_rx.changed().await.is_ok() {
                let connection_state = *state_rx.borrow_and_update();
                let should_continue =
                    connection_state != webrtc::peer_connection::RTCPeerConnectionState::Closed;
                let _ = this.update(cx, |state, cx| {
                    state.handle_voice_peer_state(
                        &peer,
                        &call_id,
                        &initiator_id,
                        group_id.clone(),
                        connection_state,
                        cx,
                    );
                });
                if !should_continue {
                    break;
                }
            }
        })
        .detach();
    }

    fn monitor_voice_ice_candidates(
        &mut self,
        peer: User,
        call_id: String,
        initiator_id: String,
        group_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(transport) = self.peer_transport.clone() else {
            return;
        };
        let mut candidates = match transport.take_voice_ice_candidates(&peer.id, &call_id) {
            Ok(candidates) => candidates,
            Err(error) => {
                eprintln!("Could not monitor local ICE candidates: {error}");
                return;
            }
        };
        cx.spawn(async move |this, cx| {
            while let Some(candidate) = candidates.recv().await {
                let peer = peer.clone();
                let call_id = call_id.clone();
                let initiator_id = initiator_id.clone();
                let group_id = group_id.clone();
                let _ = this.update(cx, |state, cx| {
                    if state
                        .active_voice_call
                        .as_ref()
                        .is_some_and(|call| call.call_id == call_id)
                    {
                        state.dispatch_voice_signal_targets_with_description(
                            VoiceSignalAction::Candidate,
                            &call_id,
                            group_id,
                            initiator_id,
                            vec![VoiceSignalTarget {
                                peer,
                                participants: None,
                            }],
                            None,
                            Some(candidate),
                            cx,
                        );
                    }
                });
            }
        })
        .detach();
    }

    pub(super) fn handle_voice_peer_state(
        &mut self,
        peer: &User,
        call_id: &str,
        initiator_id: &str,
        group_id: Option<String>,
        connection_state: webrtc::peer_connection::RTCPeerConnectionState,
        cx: &mut Context<Self>,
    ) {
        use webrtc::peer_connection::RTCPeerConnectionState;
        let local_id = self.current_user.as_ref().map(|user| user.id.clone());
        let offerer = local_id.as_deref().is_some_and(|local_id| {
            if group_id.is_some() {
                should_initiate_voice_mesh_link(local_id, &peer.id)
            } else {
                local_id == initiator_id
            }
        });
        let mut newly_connected = false;
        let mut roster = None;
        let mut restart = false;
        let mut restart_exhausted = false;
        {
            let Some(call) = self
                .active_voice_call
                .as_mut()
                .filter(|call| call.call_id == call_id)
            else {
                return;
            };
            match connection_state {
                RTCPeerConnectionState::Connected => {
                    newly_connected = call.connected_peers.insert(peer.id.clone());
                    call.connecting_peers.remove(&peer.id);
                    call.offering_peers.remove(&peer.id);
                    call.reconnect_attempts.remove(&peer.id);
                    if !call.participants.contains(&peer.id) {
                        call.participants.push(peer.id.clone());
                    }
                    call.connected = true;
                    if newly_connected && local_id.as_deref() == Some(call.initiator_id.as_str()) {
                        roster = Some((
                            call.initiator_id.clone(),
                            call.participants.clone(),
                            call.conversation_id.clone(),
                        ));
                    }
                }
                RTCPeerConnectionState::Disconnected => {
                    call.connected_peers.remove(&peer.id);
                    call.connected = !call.connected_peers.is_empty();
                    call.connecting_peers.insert(peer.id.clone());
                }
                RTCPeerConnectionState::Failed => {
                    call.connected_peers.remove(&peer.id);
                    call.offering_peers.remove(&peer.id);
                    call.connected = !call.connected_peers.is_empty();
                    if offerer {
                        let attempts = call.reconnect_attempts.entry(peer.id.clone()).or_default();
                        if *attempts < 2 {
                            *attempts += 1;
                            call.connecting_peers.insert(peer.id.clone());
                            restart = true;
                        } else {
                            call.connecting_peers.remove(&peer.id);
                            restart_exhausted = true;
                        }
                    } else {
                        call.connecting_peers.remove(&peer.id);
                    }
                }
                RTCPeerConnectionState::Closed => {
                    call.connected_peers.remove(&peer.id);
                    call.connecting_peers.remove(&peer.id);
                    call.offering_peers.remove(&peer.id);
                    call.connected = !call.connected_peers.is_empty();
                }
                _ => {}
            }
        }
        match connection_state {
            RTCPeerConnectionState::Connected if newly_connected => {
                self.set_toast(Some("Voice connected directly.".into()), cx);
            }
            RTCPeerConnectionState::Disconnected => self.set_toast(
                Some("The direct ICE path was interrupted; WebRTC is attempting to recover it.".into()),
                cx,
            ),
            RTCPeerConnectionState::Failed if restart => {
                self.set_toast(
                    Some("Direct ICE failed; attempting an ICE restart without a relay.".into()),
                    cx,
                );
                self.connect_voice_peer(
                    peer.clone(),
                    call_id.to_string(),
                    initiator_id.to_string(),
                    group_id.clone(),
                    true,
                    cx,
                );
            }
            RTCPeerConnectionState::Failed if restart_exhausted => self.set_toast(
                Some("Direct ICE connection failed after two restart attempts. This network requires a relay, which Resonance voice does not use.".into()),
                cx,
            ),
            _ => {}
        }
        if let Some((roster_initiator, participants, conversation_id)) = roster
            && let Some(conversation) = self.conversation(&conversation_id).cloned()
            && conversation.is_group()
        {
            self.broadcast_voice_roster(call_id, &conversation, roster_initiator, participants, cx);
        }
        cx.notify();
    }

    pub(super) fn accept_voice_offer(
        &mut self,
        sender_id: String,
        signal: VoiceSignal,
        local_conversation_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(local_id) = self.current_user.as_ref().map(|user| user.id.clone()) else {
            return;
        };
        let Some(conversation) = self.conversation(&local_conversation_id).cloned() else {
            return;
        };
        let is_group = conversation.is_group();
        let valid_offer = self.active_voice_call.as_ref().is_some_and(|call| {
            call.call_id == signal.call_id
                && call.conversation_id == local_conversation_id
                && call.initiator_id == signal.initiator_id
                && call.eligible_peers.contains(&sender_id)
                && if is_group {
                    should_initiate_voice_mesh_link(&sender_id, &local_id)
                } else {
                    sender_id == signal.initiator_id
                }
        });
        if !valid_offer {
            return;
        }
        let Some(offer) = signal.description else {
            self.set_toast(Some("The received WebRTC offer was empty.".into()), cx);
            return;
        };
        let peer = conversation
            .members
            .iter()
            .find(|member| member.id == sender_id)
            .cloned()
            .or_else(|| self.friend(&sender_id).cloned());
        let Some(peer) = peer else {
            return;
        };
        if let Some(call) = self.active_voice_call.as_mut() {
            call.connecting_peers.insert(sender_id.clone());
        }
        let Some(transport) = self.peer_transport.clone() else {
            self.set_toast(Some("Peer networking is unavailable.".into()), cx);
            return;
        };
        let call_id = signal.call_id;
        let initiator_id = signal.initiator_id;
        let group_id = is_group.then(|| conversation.id);
        let task_call_id = call_id.clone();
        let peer_id = sender_id;
        let cleanup_call_id = task_call_id.clone();
        let cleanup_peer_id = peer_id.clone();
        let background = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let result = background
                .spawn(async move {
                    let result = transport.accept_voice_offer(&peer_id, &task_call_id, &offer);
                    if result.is_err()
                        && let Err(error) =
                            transport.remove_voice_peer(&cleanup_call_id, &cleanup_peer_id)
                    {
                        eprintln!("Could not clean up an incomplete WebRTC answer: {error}");
                    }
                    result
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                if !state
                    .active_voice_call
                    .as_ref()
                    .is_some_and(|call| call.call_id == call_id)
                {
                    return;
                }
                match result {
                    Ok(answer) => {
                        state.dispatch_voice_signal_targets_with_description(
                            VoiceSignalAction::Answer,
                            &call_id,
                            group_id.clone(),
                            initiator_id.clone(),
                            vec![VoiceSignalTarget {
                                peer: peer.clone(),
                                participants: None,
                            }],
                            Some(answer),
                            None,
                            cx,
                        );
                        state.monitor_voice_ice_candidates(
                            peer.clone(),
                            call_id.clone(),
                            initiator_id.clone(),
                            group_id.clone(),
                            cx,
                        );
                        state.monitor_voice_peer_connection(
                            peer.clone(),
                            call_id.clone(),
                            initiator_id.clone(),
                            group_id.clone(),
                            cx,
                        );
                    }
                    Err(error) => {
                        state.clear_voice_peer_attempts(&call_id, std::slice::from_ref(&peer.id));
                        state.set_toast(
                            Some(format!(
                                "Could not negotiate a direct WebRTC connection: {error}"
                            )),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn apply_voice_answer(
        &mut self,
        sender_id: String,
        signal: VoiceSignal,
        local_conversation_id: String,
        cx: &mut Context<Self>,
    ) {
        let Some(local_id) = self.current_user.as_ref().map(|user| user.id.clone()) else {
            return;
        };
        let Some(call) = self.active_voice_call.as_ref().filter(|call| {
            call.call_id == signal.call_id
                && call.conversation_id == local_conversation_id
                && call.initiator_id == signal.initiator_id
                && call.eligible_peers.contains(&sender_id)
                && call.connecting_peers.contains(&sender_id)
                && self
                    .conversation(&call.conversation_id)
                    .is_some_and(|conversation| {
                        if conversation.is_group() {
                            should_initiate_voice_mesh_link(&local_id, &sender_id)
                        } else {
                            local_id == call.initiator_id
                        }
                    })
        }) else {
            return;
        };
        let call_id = call.call_id.clone();
        let conversation_id = call.conversation_id.clone();
        let Some(conversation) = self.conversation(&conversation_id).cloned() else {
            return;
        };
        let peer = conversation
            .members
            .iter()
            .find(|member| member.id == sender_id)
            .cloned()
            .or_else(|| self.friend(&sender_id).cloned());
        let Some(peer) = peer else {
            return;
        };
        let Some(answer) = signal.description else {
            self.set_toast(Some("The received WebRTC answer was empty.".into()), cx);
            return;
        };
        let Some(transport) = self.peer_transport.clone() else {
            self.set_toast(Some("Peer networking is unavailable.".into()), cx);
            return;
        };
        let task_call_id = call_id.clone();
        let peer_id = sender_id;
        let cleanup_call_id = task_call_id.clone();
        let cleanup_peer_id = peer_id.clone();
        let background = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let result = background
                .spawn(async move {
                    let result = transport.apply_voice_answer(&peer_id, &task_call_id, &answer);
                    if result.is_err()
                        && let Err(error) =
                            transport.remove_voice_peer(&cleanup_call_id, &cleanup_peer_id)
                    {
                        eprintln!("Could not clean up a failed WebRTC negotiation: {error}");
                    }
                    result
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                if !state
                    .active_voice_call
                    .as_ref()
                    .is_some_and(|call| call.call_id == call_id)
                {
                    return;
                }
                match result {
                    Ok(()) => {}
                    Err(error) => {
                        state.clear_voice_peer_attempts(&call_id, std::slice::from_ref(&peer.id));
                        state.set_toast(
                            Some(format!("Could not apply the WebRTC answer: {error}")),
                            cx,
                        );
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn handle_voice_signal(
        &mut self,
        received: ReceivedVoiceSignal,
        cx: &mut Context<Self>,
    ) {
        let ReceivedVoiceSignal {
            sender_id,
            local_conversation_id,
            signal,
        } = received;
        match signal.action {
            VoiceSignalAction::Invite => {
                if sender_id != signal.initiator_id {
                    return;
                }
                if self.active_voice_call.is_some() || self.incoming_voice_call.is_some() {
                    if let Some(conversation) = self.conversation(&local_conversation_id).cloned()
                        && let Ok(peers) = self.voice_call_peers(&conversation)
                    {
                        self.dispatch_voice_signal(
                            VoiceSignalAction::Decline,
                            &signal.call_id,
                            &conversation,
                            signal.initiator_id,
                            peers,
                            cx,
                        );
                    }
                    self.set_toast(Some("A voice call arrived while you are busy.".into()), cx);
                    return;
                }
                let Some(conversation) = self.conversation(&local_conversation_id) else {
                    return;
                };
                let caller_name = conversation
                    .members
                    .iter()
                    .find(|member| member.id == sender_id)
                    .map(|member| member.name.clone())
                    .or_else(|| self.friend(&sender_id).map(|friend| friend.name.clone()))
                    .unwrap_or_else(|| "A group member".into());
                self.incoming_voice_call = Some(IncomingVoiceCall {
                    call_id: signal.call_id.clone(),
                    conversation_id: local_conversation_id,
                    caller_id: sender_id,
                    caller_name: caller_name.clone(),
                });
                self.set_toast(Some(format!("{caller_name} is calling.")), cx);
                self.expire_incoming_call(signal.call_id, cx);
            }
            VoiceSignalAction::Accept => {
                let Some(local_id) = self.current_user.as_ref().map(|user| user.id.clone()) else {
                    return;
                };
                let Some((peer, group_id)) = self
                    .active_voice_call
                    .as_ref()
                    .filter(|call| {
                        call.call_id == signal.call_id
                            && call.conversation_id == local_conversation_id
                            && call.initiator_id == signal.initiator_id
                            && call.eligible_peers.contains(&sender_id)
                            && !call.connected_peers.contains(&sender_id)
                            && !call.offering_peers.contains(&sender_id)
                            && self.conversation(&call.conversation_id).is_some_and(
                                |conversation| {
                                    !conversation.is_group()
                                        || should_initiate_voice_mesh_link(&local_id, &sender_id)
                                },
                            )
                    })
                    .and_then(|_| {
                        let conversation = self.conversation(&local_conversation_id)?.clone();
                        let peer = conversation
                            .members
                            .iter()
                            .find(|member| member.id == sender_id)
                            .cloned()
                            .or_else(|| self.friend(&sender_id).cloned())?;
                        let group_id = conversation.is_group().then(|| conversation.id.clone());
                        Some((peer, group_id))
                    })
                else {
                    return;
                };
                self.connect_voice_peer(
                    peer,
                    signal.call_id,
                    signal.initiator_id,
                    group_id,
                    false,
                    cx,
                );
            }
            VoiceSignalAction::Connected => {}
            VoiceSignalAction::PeerOffer => {
                self.accept_mesh_offer(sender_id, signal, local_conversation_id, cx);
            }
            VoiceSignalAction::Offer => {
                self.accept_voice_offer(sender_id, signal, local_conversation_id, cx);
            }
            VoiceSignalAction::Answer => {
                self.apply_voice_answer(sender_id, signal, local_conversation_id, cx);
            }
            VoiceSignalAction::Candidate => {
                let Some(candidate) = signal.candidate else {
                    return;
                };
                let Some(call) = self.active_voice_call.as_ref().filter(|call| {
                    call.call_id == signal.call_id
                        && call.conversation_id == local_conversation_id
                        && call.initiator_id == signal.initiator_id
                        && call.eligible_peers.contains(&sender_id)
                        && self
                            .current_user
                            .as_ref()
                            .is_some_and(|user| user.id != sender_id)
                }) else {
                    return;
                };
                let call_id = call.call_id.clone();
                let peer_id = sender_id;
                let Some(transport) = self.peer_transport.clone() else {
                    return;
                };
                cx.spawn(async move |_this, cx| {
                    let result = cx
                        .background_spawn(async move {
                            transport.add_voice_ice_candidate(&peer_id, &call_id, &candidate)
                        })
                        .await;
                    if let Err(error) = result {
                        eprintln!("Could not apply a remote voice ICE candidate: {error}");
                    }
                })
                .detach();
            }
            VoiceSignalAction::Roster => {
                if sender_id != signal.initiator_id {
                    return;
                }
                let Some(participants) = signal.participants else {
                    return;
                };
                let Some(call) = self.active_voice_call.as_ref().filter(|call| {
                    call.call_id == signal.call_id
                        && call.conversation_id == local_conversation_id
                        && call.initiator_id == signal.initiator_id
                        && self
                            .conversation(&call.conversation_id)
                            .is_some_and(Conversation::is_group)
                }) else {
                    return;
                };
                if !participants.contains(&call.initiator_id) {
                    return;
                }
                self.begin_mesh_links(&signal.call_id, participants, cx);
            }
            VoiceSignalAction::Decline => {
                let is_direct_call = self
                    .active_voice_call
                    .as_ref()
                    .filter(|call| call.call_id == signal.call_id)
                    .and_then(|call| self.conversation(&call.conversation_id))
                    .is_some_and(|conversation| !conversation.is_group());
                if is_direct_call {
                    self.end_voice_call(true, cx);
                    self.set_toast(Some("The call was declined.".into()), cx);
                } else if self
                    .active_voice_call
                    .as_ref()
                    .is_some_and(|call| call.call_id == signal.call_id)
                {
                    self.set_toast(Some("A participant declined the call.".into()), cx);
                }
            }
            VoiceSignalAction::Leave => {
                let is_direct_call = self
                    .active_voice_call
                    .as_ref()
                    .filter(|call| call.call_id == signal.call_id)
                    .and_then(|call| self.conversation(&call.conversation_id))
                    .is_some_and(|conversation| !conversation.is_group());
                if is_direct_call {
                    self.end_voice_call(false, cx);
                } else if let Some(call) = self.active_voice_call.as_mut()
                    && call.call_id == signal.call_id
                {
                    call.connected_peers.remove(&sender_id);
                    call.connecting_peers.remove(&sender_id);
                    call.participants
                        .retain(|participant| participant != &sender_id);
                    let roster = Some((
                        call.initiator_id.clone(),
                        call.participants.clone(),
                        call.conversation_id.clone(),
                    ));
                    if let Some(transport) = &self.peer_transport {
                        let _ = transport.remove_voice_peer(&signal.call_id, &sender_id);
                    }
                    self.set_toast(Some("A participant left the voice call.".into()), cx);
                    if let Some((initiator_id, participants, conversation_id)) = roster
                        && self
                            .current_user
                            .as_ref()
                            .is_some_and(|user| user.id == initiator_id)
                        && let Some(conversation) = self.conversation(&conversation_id).cloned()
                    {
                        self.broadcast_voice_roster(
                            &signal.call_id,
                            &conversation,
                            initiator_id,
                            participants,
                            cx,
                        );
                    }
                }
            }
            VoiceSignalAction::End => {
                if sender_id == signal.initiator_id
                    && self.active_voice_call.as_ref().is_some_and(|call| {
                        call.call_id == signal.call_id && call.initiator_id == signal.initiator_id
                    })
                {
                    self.end_voice_call(false, cx);
                } else if sender_id == signal.initiator_id
                    && self.incoming_voice_call.as_ref().is_some_and(|call| {
                        call.call_id == signal.call_id && call.caller_id == signal.initiator_id
                    })
                {
                    self.incoming_voice_call = None;
                    self.set_toast(Some("The caller ended the call.".into()), cx);
                }
            }
        }
        cx.notify();
    }

    pub fn toggle_voice_recording(&mut self, cx: &mut Context<Self>) {
        if self.active_voice_call.is_some() {
            self.set_toast(
                Some("Voice notes are unavailable during a voice call.".into()),
                cx,
            );
            cx.notify();
            return;
        }
        if self.editing_message_id.is_some() {
            self.set_toast(
                Some("Finish editing the message before recording a voice note.".into()),
                cx,
            );
            cx.notify();
            return;
        }
        if let Some(recording) = self.voice_recorder.take() {
            let recording_conversation = self.voice_recording_conversation.take();
            if recording_conversation != self.selected_conversation {
                drop(recording);
                self.set_toast(
                    Some("The conversation changed; the voice recording was discarded.".into()),
                    cx,
                );
                cx.notify();
                return;
            }
            match recording.finish() {
                Ok(contents) => self.upload_voice_attachment(contents, cx),
                Err(error) => {
                    self.set_toast(Some(error), cx);
                    cx.notify();
                }
            }
            return;
        }
        if let Err(error) = self.attachment_destination() {
            self.set_toast(Some(error), cx);
            cx.notify();
            return;
        }
        match VoiceRecorder::start() {
            Ok(recording) => {
                self.voice_recorder = Some(recording);
                self.voice_recording_conversation = self.selected_conversation.clone();
                self.set_toast(
                    Some("Recording voice note… click the microphone again to stop.".into()),
                    cx,
                );
            }
            Err(error) => self.set_toast(Some(error), cx),
        }
        cx.notify();
    }

    pub fn is_recording_voice(&self) -> bool {
        self.voice_recorder.is_some()
    }
}
