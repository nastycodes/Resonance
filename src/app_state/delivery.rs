use super::*;

pub(super) fn encode_group_payload(payload: &GroupRelayPayload) -> Result<String, String> {
    serde_json::to_string(payload)
        .map(|payload| format!("{GROUP_RELAY_PREFIX}{payload}"))
        .map_err(|error| format!("Could not encode the group message: {error}"))
}

pub(super) fn encode_mls_welcome_payload(
    conversation: &Conversation,
    mls_group_id: &str,
    record: &MlsWelcomeRecord,
) -> Result<String, String> {
    if conversation.is_group() {
        encode_group_payload(&GroupRelayPayload::Invite {
            conversation_id: conversation.id.clone(),
            name: conversation.name.clone(),
            members: conversation.members.clone(),
            membership_version: conversation.membership_version,
            welcome: record.welcome.clone(),
        })
    } else {
        serde_json::to_string(&DirectMlsWelcome {
            conversation_id: mls_group_id.to_string(),
            welcome: record.welcome.clone(),
        })
        .map(|payload| format!("{DIRECT_MLS_WELCOME_PREFIX}{payload}"))
        .map_err(|error| format!("Could not encode the direct MLS welcome: {error}"))
    }
}

pub(super) fn decode_group_payload(body: &str) -> Result<Option<GroupRelayPayload>, String> {
    let Some(payload) = body.strip_prefix(GROUP_RELAY_PREFIX) else {
        return Ok(None);
    };
    serde_json::from_str(payload)
        .map(Some)
        .map_err(|error| format!("The received group message is invalid: {error}"))
}

pub(super) fn encode_membership_commit_payload(
    operation_id: &str,
    membership_version: u64,
    members: &[User],
    commit: &str,
) -> Result<String, String> {
    serde_json::to_string(&GroupMembershipCommit {
        operation_id: operation_id.to_string(),
        membership_version,
        members: members.to_vec(),
        commit: commit.to_string(),
    })
    .map(|payload| format!("{GROUP_MEMBERSHIP_PREFIX}{payload}"))
    .map_err(|error| format!("Could not encode the MLS membership commit: {error}"))
}

pub(super) fn create_mls_group_from_api(
    api: &AccountApiClient,
    token: &str,
    store: &LocalChatStore,
    account_id: &str,
    identity_private_key: &[u8],
    conversation_id: &str,
    peers: &[User],
) -> Result<String, String> {
    let peer_ids = peers.iter().map(|peer| peer.id.clone()).collect::<Vec<_>>();
    let claimed = api
        .claim_mls_key_packages(token, &peer_ids)
        .map_err(|error| error.message)?;
    if claimed.len() != peers.len() {
        return Err("The account API returned an incomplete MLS key package set.".into());
    }
    let mut packages = HashMap::with_capacity(claimed.len());
    for package in claimed {
        if packages
            .insert(package.account_public_key.clone(), package.key_package)
            .is_some()
        {
            return Err("The account API returned a duplicate MLS key package.".into());
        }
    }
    let member_key_packages = peers
        .iter()
        .map(|peer| {
            packages
                .remove(&peer.id)
                .map(|key_package| (peer.id.clone(), key_package))
                .ok_or_else(|| format!("No MLS key package was returned for @{}.", peer.username))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let replacement_packages =
        MlsService::generate_key_packages(store, account_id, identity_private_key, 10)?;
    api.publish_mls_key_packages(token, &replacement_packages)
        .map_err(|error| error.message)?;
    let welcome = MlsService::create_group(
        store,
        account_id,
        identity_private_key,
        conversation_id,
        &member_key_packages,
    )?;
    store.save_mls_welcome(conversation_id, &welcome)?;
    Ok(welcome)
}

pub(super) fn prepare_group_membership_change(
    api: &AccountApiClient,
    token: &str,
    store: &LocalChatStore,
    current_user: &User,
    identity_private_key: &[u8],
    conversation: &Conversation,
    target: &User,
    adding: bool,
) -> Result<(), String> {
    if conversation.membership_version == u64::MAX
        || conversation
            .members
            .first()
            .map(|member| member.id.as_str())
            != Some(current_user.id.as_str())
    {
        return Err("The local group owner or roster version is invalid.".into());
    }
    if store.pending_group_change(&conversation.id)?.is_some() {
        return Err("This group already has a pending membership change.".into());
    }
    let roster = api
        .group_roster(token, &conversation.id)
        .map_err(|error| error.message)?;
    let local_ids = sorted_member_ids(
        &conversation
            .members
            .iter()
            .map(|member| member.id.clone())
            .collect::<Vec<_>>(),
    );
    let server_ids = sorted_member_ids(
        &roster
            .members
            .iter()
            .map(|member| member.public_key.to_lowercase())
            .collect::<Vec<_>>(),
    );
    if roster.version != conversation.membership_version
        || roster.owner_public_key.to_lowercase() != current_user.id
        || local_ids != server_ids
    {
        return Err("The server roster differs from local MLS state; synchronize before changing membership.".into());
    }
    let mut members = conversation.members.clone();
    let (add_key_package, remove_member_id) = if adding {
        if members.len() >= 12 || members.iter().any(|member| member.id == target.id) {
            return Err("The group cannot add this member.".into());
        }
        let claimed = api
            .claim_mls_key_packages(token, std::slice::from_ref(&target.id))
            .map_err(|error| error.message)?;
        let key_package = claimed
            .into_iter()
            .find(|package| package.account_public_key == target.id)
            .map(|package| package.key_package)
            .ok_or_else(|| {
                "The account API returned no MLS key package for this member.".to_string()
            })?;
        let replacements =
            MlsService::generate_key_packages(store, &current_user.id, identity_private_key, 10)?;
        api.publish_mls_key_packages(token, &replacements)
            .map_err(|error| error.message)?;
        members.push(sanitized_group_member(target.clone()));
        (Some((target.id.as_str(), key_package)), None)
    } else {
        if target.id == current_user.id || members.len() <= 3 {
            return Err(
                "The group owner cannot be removed and groups require at least three members."
                    .into(),
            );
        }
        if !members.iter().any(|member| member.id == target.id) {
            return Err("The member is not in this group.".into());
        }
        members.retain(|member| member.id != target.id);
        (None, Some(target.id.as_str()))
    };
    let prepared = MlsService::prepare_membership_change(
        store,
        &current_user.id,
        identity_private_key,
        &conversation.id,
        add_key_package
            .as_ref()
            .map(|(member_id, package)| (*member_id, package.as_str())),
        remove_member_id,
    )?;
    let member_ids = members
        .iter()
        .map(|member| member.id.clone())
        .collect::<Vec<_>>();
    if prepared.expected_member_ids != sorted_member_ids(&member_ids) {
        return Err("The MLS proposal does not match the requested group roster.".into());
    }
    let operation_id = new_id()?;
    let membership_version = conversation.membership_version + 1;
    let sent_at = timestamp()?;
    let existing_membership_payload = encode_membership_commit_payload(
        &operation_id,
        membership_version,
        &members,
        &prepared.commit,
    )?;
    let welcome_payload = if let Some(welcome) = prepared.welcome.as_deref() {
        Some(encode_group_payload(&GroupRelayPayload::Invite {
            conversation_id: conversation.id.clone(),
            name: conversation.name.clone(),
            members: members.clone(),
            membership_version,
            welcome: welcome.to_string(),
        })?)
    } else {
        None
    };
    let mut recipients = conversation
        .members
        .iter()
        .filter(|member| member.id != current_user.id)
        .map(|member| (member, existing_membership_payload.as_str()))
        .collect::<Vec<_>>();
    if let Some(added) = members
        .iter()
        .find(|member| adding && member.id == target.id)
    {
        let payload = welcome_payload
            .as_deref()
            .ok_or_else(|| "The MLS add operation did not produce a Welcome.".to_string())?;
        recipients.push((added, payload));
    }
    let deliveries = recipients
        .into_iter()
        .map(|(recipient, payload)| {
            let message_id = new_id()?;
            let envelope = relay::encrypt(
                identity_private_key,
                &recipient.id,
                &recipient.id,
                &message_id,
                &sent_at,
                Some(&conversation.id),
                payload,
            )?;
            Ok(relay::RelayDelivery {
                recipient_public_key: recipient.id.clone(),
                envelope,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    store.save_pending_group_change(
        PendingGroupMembershipChange {
            group_id: conversation.id.clone(),
            operation_id,
            expected_version: conversation.membership_version,
            members,
            deliveries,
        },
        prepared.snapshot,
    )
}

pub(super) fn retry_pending_group_membership_changes(
    api: &AccountApiClient,
    token: &str,
    store: &LocalChatStore,
    account_id: &str,
    identity_private_key: &[u8],
) -> Result<(), String> {
    let changes = store.pending_group_changes()?;
    let mut failures = Vec::new();
    for change in changes {
        let member_ids = change
            .members
            .iter()
            .map(|member| member.id.clone())
            .collect::<Vec<_>>();
        let result = (|| {
            let version = api
                .update_group_members(
                    token,
                    &change.group_id,
                    &change.operation_id,
                    change.expected_version,
                    &member_ids,
                    &change.deliveries,
                )
                .map_err(|error| error.message)?;
            if version != change.expected_version + 1 {
                return Err("The account API returned an unexpected group roster version.".into());
            }
            let pending_snapshot = store.mls_storage_snapshot()?;
            let (state, snapshot) = MlsService::merge_pending_membership_change(
                store,
                account_id,
                identity_private_key,
                &change.group_id,
                &pending_snapshot,
            )?;
            if state.member_ids != sorted_member_ids(&member_ids) || state.epoch != version {
                return Err(
                    "The MLS commit does not match the requested group roster and epoch.".into(),
                );
            }
            store.complete_pending_group_change(
                &change.group_id,
                &change.operation_id,
                version,
                &state.member_ids,
                snapshot,
            )
        })();
        if let Err(error) = result {
            failures.push(format!("{}: {error}", change.group_id));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Pending group membership changes need recovery: {}",
            failures.join("; ")
        ))
    }
}

pub(super) fn reconcile_group_rosters(
    api: &AccountApiClient,
    token: &str,
    store: &LocalChatStore,
    account_id: &str,
    identity_private_key: &[u8],
) -> Vec<String> {
    let mut errors = Vec::new();
    let conversations = match store.conversations() {
        Ok(conversations) => conversations,
        Err(error) => return vec![error],
    };
    for conversation in conversations.iter().filter(|item| item.is_group()) {
        match store.pending_group_change(&conversation.id) {
            Ok(Some(_)) => continue,
            Ok(None) => {}
            Err(error) => {
                errors.push(error);
                continue;
            }
        }
        if !conversation
            .members
            .iter()
            .any(|member| member.id == account_id)
        {
            continue;
        }
        if let Err(error) = verify_group_roster(
            api,
            token,
            store,
            account_id,
            identity_private_key,
            conversation,
        ) {
            errors.push(format!(
                "Could not synchronize group {}: {error}",
                conversation.id
            ));
        }
    }
    errors
}

pub(super) fn sorted_member_ids(member_ids: &[String]) -> Vec<String> {
    let mut ids = member_ids.to_vec();
    ids.sort();
    ids
}

pub(super) struct SendMessageCompletion {
    pub(super) messages: Vec<Message>,
    pub(super) conversations: Vec<Conversation>,
    pub(super) attachment_ids: Vec<String>,
    pub(super) toast: Option<String>,
}

pub(super) fn send_message_work(
    mut message: Message,
    conversation: Conversation,
    peers: Vec<User>,
    store: LocalChatStore,
    api: AccountApiClient,
    token: String,
    private_key: Zeroizing<Vec<u8>>,
    peer_transport: Option<PeerTransport>,
) -> Result<SendMessageCompletion, String> {
    let author_id = message.author_id.clone();
    for peer in &peers {
        store.ensure_identity_not_changed(peer)?;
    }
    if conversation.is_group() {
        verify_group_roster(
            &api,
            &token,
            &store,
            &author_id,
            private_key.as_slice(),
            &conversation,
        )
        .map_err(|error| {
            format!("Group messages are paused until MLS membership synchronizes: {error}")
        })?;
    }
    let mls_group_id = if conversation.is_group() {
        conversation.id.clone()
    } else {
        MlsService::direct_group_id(&author_id, &peers[0].id)?
    };
    let mls_exists =
        MlsService::has_group(&store, &author_id, private_key.as_slice(), &mls_group_id)?;
    let mls_welcome = if mls_exists {
        store.mls_welcome(&mls_group_id)?
    } else if !conversation.is_group() && author_id > peers[0].id {
        return Err("Waiting for the peer to establish end-to-end encryption.".into());
    } else {
        create_mls_group_from_api(
            &api,
            &token,
            &store,
            &author_id,
            private_key.as_slice(),
            &mls_group_id,
            &peers,
        )
        .map_err(|error| format!("Could not establish end-to-end encryption: {error}"))?;
        store.mls_welcome(&mls_group_id)?
    };
    let welcome_payload = mls_welcome
        .as_ref()
        .map(|record| encode_mls_welcome_payload(&conversation, &mls_group_id, record))
        .transpose()?;
    let plaintext = attachment::encode_message_payload(&message.body, &message.attachments)?;
    let encrypted_payload = MlsService::encrypt(
        &store,
        &author_id,
        private_key.as_slice(),
        &mls_group_id,
        plaintext.as_bytes(),
    )?;
    message.encrypted_payload = Some(format!("{MLS_MESSAGE_PREFIX}{encrypted_payload}"));
    if conversation.is_group() {
        store.add_group_message(
            message.clone(),
            &conversation.id,
            &conversation.name,
            &conversation.members,
        )?;
    } else {
        store.add_message(message.clone(), &peers[0])?;
    }

    let mut relay_deliveries = 0;
    let mut group_deliveries = message.group_deliveries.clone();
    let delivery = (|| {
        let group_id = conversation.is_group().then_some(conversation.id.as_str());
        let group_commit_pending = conversation.is_group()
            && (store.group_commit_delivery_pending(&conversation.id)?
                || store.pending_group_change(&conversation.id)?.is_some());
        let mut failures = Vec::new();
        for peer in &peers {
            if let (Some(record), Some(payload)) =
                (mls_welcome.as_ref(), welcome_payload.as_deref())
                && !record
                    .queued_recipients
                    .iter()
                    .any(|recipient| recipient == &peer.id)
            {
                let welcome_envelope = relay::encrypt(
                    private_key.as_slice(),
                    &peer.id,
                    &peer.id,
                    &record.message_id,
                    &record.sent_at,
                    group_id,
                    payload,
                )?;
                if let Err(error) =
                    api.send_relay_message(&token, &peer.id, group_id, &welcome_envelope)
                {
                    failures.push(format!("@{}: {}", peer.username, error.message));
                    continue;
                }
                if let Err(error) = store.mark_mls_welcome_queued(&mls_group_id, &peer.id) {
                    failures.push(format!("@{}: {error}", peer.username));
                    continue;
                }
            }
            let Some(payload) = message.encrypted_payload.as_deref() else {
                if !conversation.is_group() {
                    return Err("The message has not been encrypted.".into());
                }
                let Some(delivery) = group_deliveries
                    .iter_mut()
                    .find(|delivery| delivery.recipient_id == peer.id)
                else {
                    return Err(format!(
                        "@{} is not in the saved group delivery roster.",
                        peer.username
                    ));
                };
                delivery.state = RecipientDeliveryState::Rejected;
                delivery.last_error = Some("The message has not been encrypted.".into());
                store.update_group_delivery(&message.id, delivery.clone())?;
                failures.push(format!(
                    "@{}: The message has not been encrypted.",
                    peer.username
                ));
                continue;
            };
            let envelope = match relay::encrypt(
                private_key.as_slice(),
                &peer.id,
                &peer.id,
                &message.id,
                &message.sent_at,
                group_id,
                payload,
            ) {
                Ok(envelope) => envelope,
                Err(error) if conversation.is_group() => {
                    let Some(delivery) = group_deliveries
                        .iter_mut()
                        .find(|delivery| delivery.recipient_id == peer.id)
                    else {
                        return Err(format!(
                            "@{} is not in the saved group delivery roster.",
                            peer.username
                        ));
                    };
                    delivery.state = RecipientDeliveryState::Rejected;
                    delivery.last_error = Some(error.clone());
                    store.update_group_delivery(&message.id, delivery.clone())?;
                    failures.push(format!("@{}: {error}", peer.username));
                    continue;
                }
                Err(error) => return Err(error),
            };
            if !group_commit_pending && let Some(transport) = peer_transport.clone() {
                let peer = peer.clone();
                let message = message.clone();
                if let Err(error) = std::thread::Builder::new()
                    .name("resonance-peer-message".into())
                    .spawn(move || {
                        if let Err(error) = transport.send(&peer, &message) {
                            eprintln!("Direct peer message delivery failed: {error}");
                        }
                    })
                {
                    eprintln!("Could not start direct peer message delivery: {error}");
                }
            }
            match api.send_relay_message(&token, &peer.id, group_id, &envelope) {
                Ok(queue_sequence) => {
                    if let Err(error) = store.set_message_queue_sequence(&message.id, queue_sequence) {
                        failures.push(format!("@{}: {}", peer.username, error));
                    }
                    relay_deliveries += 1;
                    if conversation.is_group() {
                        let Some(delivery) = group_deliveries
                            .iter_mut()
                            .find(|delivery| delivery.recipient_id == peer.id)
                        else {
                            return Err(format!(
                                "@{} is not in the saved group delivery roster.",
                                peer.username
                            ));
                        };
                        delivery.api_accepted = true;
                        delivery.api_failure_permanent = false;
                        if delivery.state != RecipientDeliveryState::Delivered {
                            delivery.state = RecipientDeliveryState::Pending;
                        }
                        delivery.attempts = delivery.attempts.max(1);
                        delivery.next_attempt_at = None;
                        delivery.last_error = None;
                        store.update_group_delivery(&message.id, delivery.clone())?;
                    }
                }
                Err(error) => {
                    failures.push(format!("@{}: {}", peer.username, error.message));
                    if conversation.is_group() {
                        let Some(delivery) = group_deliveries
                            .iter_mut()
                            .find(|delivery| delivery.recipient_id == peer.id)
                        else {
                            return Err(format!(
                                "@{} is not in the saved group delivery roster.",
                                peer.username
                            ));
                        };
                        delivery.attempts = delivery.attempts.saturating_add(1);
                        delivery.last_error = Some(error.message.clone());
                        delivery.next_attempt_at = None;
                        if permanent_group_delivery_error(&error) {
                            delivery.api_failure_permanent = true;
                            delivery.state = RecipientDeliveryState::Rejected;
                        } else if delivery.attempts >= GROUP_RELAY_MAX_ATTEMPTS {
                            delivery.api_failure_permanent = false;
                            delivery.state = RecipientDeliveryState::Failed;
                        } else {
                            delivery.api_failure_permanent = false;
                            delivery.state = RecipientDeliveryState::Pending;
                            delivery.next_attempt_at = Some(group_delivery_retry_at(
                                delivery.attempts,
                                timestamp()?.parse::<u64>().map_err(|error| {
                                    format!(
                                        "The device clock returned an invalid timestamp: {error}"
                                    )
                                })?,
                            ));
                        }
                        store.update_group_delivery(&message.id, delivery.clone())?;
                    }
                }
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(failures.join("; "))
        }
    })();

    let toast = match delivery {
        Ok(()) if conversation.is_group() => None,
        Ok(()) => store.set_delivery_state(&message.id, "relayed").err(),
        Err(error) => {
            if !conversation.is_group() && relay_deliveries > 0 {
                let _ = store.set_delivery_state(&message.id, "relayed");
            }
            Some(format!(
                "Message saved locally, but delivery failed: {error}"
            ))
        }
    };
    let messages = store.messages()?;
    let conversations = store.conversations()?;
    let attachment_ids = message
        .attachments()
        .map(|attachment| attachment.attachment_id.clone())
        .collect();
    Ok(SendMessageCompletion {
        messages,
        conversations,
        attachment_ids,
        toast,
    })
}

pub(super) fn verify_group_roster(
    api: &AccountApiClient,
    token: &str,
    store: &LocalChatStore,
    account_id: &str,
    identity_private_key: &[u8],
    conversation: &Conversation,
) -> Result<(), String> {
    if store.pending_group_change(&conversation.id)?.is_some() {
        return Err("The group has a pending MLS membership change.".into());
    }
    if !conversation
        .members
        .iter()
        .any(|member| member.id == account_id)
    {
        return Err("This account is no longer a member of the group.".into());
    }
    let local_owner = conversation
        .members
        .first()
        .ok_or_else(|| "The group has no local owner.".to_string())?;
    let roster = api
        .group_roster(token, &conversation.id)
        .map_err(|error| error.message)?;
    let local_ids = sorted_member_ids(
        &conversation
            .members
            .iter()
            .map(|member| member.id.clone())
            .collect::<Vec<_>>(),
    );
    let api_ids = sorted_member_ids(
        &roster
            .members
            .iter()
            .map(|member| member.public_key.to_lowercase())
            .collect::<Vec<_>>(),
    );
    if roster.version != conversation.membership_version
        || roster.owner_public_key.to_lowercase() != local_owner.id
        || api_ids != local_ids
    {
        return Err(format!(
            "The group roster is stale (local version {}, API version {}).",
            conversation.membership_version, roster.version
        ));
    }
    let state = MlsService::group_state(store, account_id, identity_private_key, &conversation.id)?;
    if state.owner_id != local_owner.id
        || state.member_ids != local_ids
        || state.epoch != conversation.membership_version
    {
        return Err("The local MLS epoch does not match the synchronized group roster.".into());
    }
    Ok(())
}

pub(super) fn validate_group_members(
    conversation_id: &str,
    name: &str,
    members: &[User],
    sender_id: &str,
    recipient_id: &str,
) -> Result<Vec<User>, String> {
    let members = validate_membership_roster(conversation_id, name, members)?;
    let member_ids = members
        .iter()
        .map(|member| member.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    if !member_ids.contains(sender_id) || !member_ids.contains(recipient_id) {
        return Err("The received group does not include its sender and recipient.".into());
    }
    Ok(members)
}

pub(super) fn validate_membership_roster(
    conversation_id: &str,
    name: &str,
    members: &[User],
) -> Result<Vec<User>, String> {
    if conversation_id.len() != 32
        || !conversation_id.bytes().all(|byte| byte.is_ascii_hexdigit())
        || name.trim().is_empty()
        || name.chars().count() > 64
        || name.chars().any(char::is_control)
        || !(3..=12).contains(&members.len())
    {
        return Err("The received group details are invalid.".into());
    }
    let mut member_ids = std::collections::HashSet::new();
    for member in members {
        if member.id.len() != 64
            || !member.id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || member.name.trim().is_empty()
            || member.name.chars().count() > 64
            || member.username.len() > 32
            || !member_ids.insert(member.id.as_str())
        {
            return Err("The received group members are invalid.".into());
        }
    }
    Ok(members
        .iter()
        .cloned()
        .map(sanitized_group_member)
        .collect())
}

pub(super) fn same_group_members(left: &[User], right: &[User]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .all(|member| right.iter().any(|other| other.id == member.id))
}

pub(super) fn process_mls_relay_body(
    body: &str,
    envelope: &relay::RelayEnvelope,
    queue_sequence: Option<u64>,
    sender_id: &str,
    account_id: &str,
    private_key: &[u8],
    store: &LocalChatStore,
    friend: Option<&User>,
) -> Result<Option<Conversation>, String> {
    if let Some(payload) = body.strip_prefix(DIRECT_MLS_WELCOME_PREFIX) {
        let friend = friend.ok_or_else(|| "A non-friend sent a direct MLS welcome.".to_string())?;
        store.ensure_identity_not_changed(friend)?;
        if envelope.group_id.is_some() {
            return Err("A direct MLS welcome cannot name a group.".into());
        }
        let welcome: DirectMlsWelcome = serde_json::from_str(payload)
            .map_err(|error| format!("The direct MLS welcome is invalid: {error}"))?;
        let expected_group_id = MlsService::direct_group_id(account_id, sender_id)?;
        if welcome.conversation_id != expected_group_id || friend.id != sender_id {
            return Err("The direct MLS welcome does not match the authenticated peer.".into());
        }
        if !MlsService::has_group(store, account_id, private_key, &expected_group_id)? {
            let expected_members = vec![account_id.to_string(), sender_id.to_string()];
            let expected_owner = if account_id < sender_id {
                account_id
            } else {
                sender_id
            };
            MlsService::join_group(
                store,
                account_id,
                private_key,
                &expected_group_id,
                &welcome.welcome,
                &expected_members,
                expected_owner,
            )?;
        }
        store.conversation_for(friend)?;
        return Ok(None);
    }

    if let Some(payload) = body.strip_prefix(GROUP_MEMBERSHIP_PREFIX) {
        let event: GroupMembershipCommit = serde_json::from_str(payload)
            .map_err(|error| format!("The MLS membership event is invalid: {error}"))?;
        let group_id = envelope.group_id.as_deref().ok_or_else(|| {
            "The MLS membership event is missing its group identifier.".to_string()
        })?;
        if event.operation_id.len() != 32
            || !event
                .operation_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("The MLS membership operation identifier is invalid.".into());
        }
        let conversation = store
            .conversations()?
            .into_iter()
            .find(|conversation| conversation.id == group_id && conversation.is_group())
            .ok_or_else(|| "The membership event refers to an unknown group.".to_string())?;
        if conversation
            .members
            .first()
            .map(|member| member.id.as_str())
            != Some(sender_id)
        {
            return Err("Only the known group owner may update membership.".into());
        }
        if event.membership_version <= conversation.membership_version {
            return Ok(None);
        }
        if event.membership_version != conversation.membership_version + 1 {
            return Err("The group membership event is out of sequence.".into());
        }
        let members = validate_membership_roster(group_id, &conversation.name, &event.members)?;
        for member in &members {
            if member.id != account_id {
                store.ensure_identity_not_changed(member)?;
            }
        }
        let (authenticated_owner, mls_state, snapshot) =
            MlsService::process_membership_commit_pending(
                store,
                account_id,
                private_key,
                group_id,
                &event.commit,
            )?;
        if authenticated_owner != sender_id
            || mls_state.owner_id != sender_id
            || mls_state.epoch != event.membership_version
            || !same_group_member_ids(&members, &mls_state.member_ids)
        {
            return Err("The MLS commit does not authenticate the advertised group roster.".into());
        }
        for credential_account_id in &mls_state.credential_changes {
            if credential_account_id != account_id {
                let member = members
                    .iter()
                    .find(|member| member.id == *credential_account_id)
                    .ok_or_else(|| {
                        "A changed MLS credential is absent from the committed roster.".to_string()
                    })?;
                store.ensure_identity_verified(member)?;
            }
        }
        store.apply_remote_group_change(
            group_id,
            event.membership_version,
            members,
            &mls_state.member_ids,
            &mls_state.owner_id,
            &mls_state.credential_changes,
            snapshot,
        )?;
        return Ok(None);
    }

    if let Some(group_payload) = decode_group_payload(body)? {
        let (conversation_id, name, members, membership_version, welcome) = match group_payload {
            GroupRelayPayload::Invite {
                conversation_id,
                name,
                members,
                membership_version,
                welcome,
            } => (conversation_id, name, members, membership_version, welcome),
            GroupRelayPayload::Message { .. } => {
                return Err("Legacy unencrypted group messages are not accepted.".into());
            }
        };
        if envelope.group_id.as_deref() != Some(conversation_id.as_str())
            || members.first().map(|member| member.id.as_str()) != Some(sender_id)
        {
            return Err("The group invitation does not match its authenticated sender.".into());
        }
        if membership_version == 0 {
            return Err("The group invitation has an invalid roster version.".into());
        }
        let members =
            validate_group_members(&conversation_id, &name, &members, sender_id, account_id)?;
        for member in &members {
            if member.id != account_id {
                store.ensure_identity_not_changed(member)?;
            }
        }
        let existing_conversations = store.conversations()?;
        let existing = existing_conversations
            .iter()
            .find(|conversation| conversation.id == conversation_id);
        if let Some(existing) = existing {
            if !existing.is_group() || existing.name != name {
                return Err("The group invitation conflicts with local membership.".into());
            }
            if existing.membership_version > membership_version {
                return Ok(None);
            }
            if existing.membership_version == membership_version
                && !same_group_members(&existing.members, &members)
            {
                return Err("The group invitation conflicts with the local roster version.".into());
            }
        }
        let expected_members = members
            .iter()
            .map(|member| member.id.clone())
            .collect::<Vec<_>>();
        let has_group = MlsService::has_group(store, account_id, private_key, &conversation_id)?;
        let must_join = !has_group
            || existing.is_some_and(|existing| existing.membership_version < membership_version);
        if must_join {
            MlsService::join_group(
                store,
                account_id,
                private_key,
                &conversation_id,
                &welcome,
                &expected_members,
                sender_id,
            )?;
        }
        let state = MlsService::group_state(store, account_id, private_key, &conversation_id)?;
        if state.owner_id != sender_id
            || state.epoch != membership_version
            || state.member_ids != sorted_member_ids(&expected_members)
        {
            return Err(
                "The MLS Welcome does not match the authorized group roster and epoch.".into(),
            );
        }
        if existing.is_none() {
            store.create_group_at_version(&conversation_id, &name, &members, membership_version)?;
        } else if existing.is_some_and(|existing| existing.membership_version < membership_version)
        {
            store.update_group_from_welcome(&conversation_id, membership_version, members)?;
        }
        return Ok(None);
    }

    let encoded_message = body
        .strip_prefix(MLS_MESSAGE_PREFIX)
        .ok_or_else(|| "The received application message is not MLS-encrypted.".to_string())?;
    let (conversation, mls_group_id, sender_name) =
        if let Some(group_id) = envelope.group_id.as_deref() {
            let conversation = store
                .conversations()?
                .into_iter()
                .find(|conversation| conversation.id == group_id && conversation.is_group())
                .ok_or_else(|| "The message refers to an unknown MLS group.".to_string())?;
            if !conversation
                .members
                .iter()
                .any(|member| member.id == account_id)
                || !conversation
                    .members
                    .iter()
                    .any(|member| member.id == sender_id)
            {
                return Err("The message sender is not a member of this group.".into());
            }
            let sender = conversation
                .members
                .iter()
                .find(|member| member.id == sender_id)
                .ok_or_else(|| "The group sender is not a known member.".to_string())?;
            let name = sender.name.clone();
            store.ensure_identity_not_changed(sender)?;
            (conversation, group_id.to_string(), name)
        } else {
            let friend = friend.ok_or_else(|| "A non-friend sent a direct message.".to_string())?;
            store.ensure_identity_not_changed(friend)?;
            let conversation = store.conversation_for(friend)?;
            (
                conversation,
                MlsService::direct_group_id(account_id, sender_id)?,
                friend.name.clone(),
            )
        };
    let Some(decrypted) = MlsService::decrypt_pending(
        store,
        account_id,
        private_key,
        &mls_group_id,
        encoded_message,
    )?
    else {
        return Ok(None);
    };
    if decrypted.sender_id != sender_id {
        return Err("The MLS message sender does not match its relay identity.".into());
    }
    let plaintext = String::from_utf8(decrypted.plaintext)
        .map_err(|_| "The encrypted message body is not valid UTF-8.".to_string())?;
    let (body, attachments) = match attachment::decode_message_payload(&plaintext)? {
        Some(payload) => payload,
        None => (plaintext, Vec::new()),
    };
    let mut message = Message::new(
        &envelope.message_id,
        &conversation.id,
        sender_id,
        &sender_name,
        &body,
        &envelope.sent_at,
        false,
    );
    message.queue_sequence = queue_sequence;
    message.attachments = attachments;
    if conversation.is_group() {
        store.add_group_message_with_mls_snapshot(
            message,
            &conversation.id,
            &conversation.name,
            &conversation.members,
            Some(decrypted.snapshot),
        )?;
    } else {
        let friend = friend.ok_or_else(|| "The sender is not a known friend.".to_string())?;
        store.add_message_with_mls_snapshot(message, friend, Some(decrypted.snapshot))?;
    }
    Ok(Some(conversation))
}

pub(super) fn retry_pending_direct_messages(
    api: &AccountApiClient,
    token: &str,
    private_key: &[u8],
    store: &LocalChatStore,
    account_id: &str,
    friends: &[User],
    peer_transport: Option<&PeerTransport>,
) -> Result<(), String> {
    let conversations = store.conversations()?;
    let now = timestamp()?
        .parse::<u64>()
        .map_err(|error| format!("The device clock returned an invalid timestamp: {error}"))?;
    for conversation in &conversations {
        let mls_group_id = if conversation.is_group() {
            conversation.id.clone()
        } else {
            MlsService::direct_group_id(account_id, &conversation.user_id)?
        };
        let Some(record) = store.mls_welcome(&mls_group_id)? else {
            continue;
        };
        let recipients = if conversation.is_group() {
            conversation
                .members
                .iter()
                .filter(|member| member.id != account_id)
                .filter_map(|member| friends.iter().find(|friend| friend.id == member.id))
                .collect::<Vec<_>>()
        } else {
            friends
                .iter()
                .find(|friend| friend.id == conversation.user_id)
                .into_iter()
                .collect()
        };
        let payload = encode_mls_welcome_payload(conversation, &mls_group_id, &record)?;
        for peer in recipients {
            if record
                .queued_recipients
                .iter()
                .any(|recipient| recipient == &peer.id)
            {
                continue;
            }
            let group_id = conversation.is_group().then_some(conversation.id.as_str());
            let envelope = relay::encrypt(
                private_key,
                &peer.id,
                &peer.id,
                &record.message_id,
                &record.sent_at,
                group_id,
                &payload,
            )?;
            api.send_relay_message(token, &peer.id, group_id, &envelope)
                .map_err(|error| {
                    format!(
                        "Could not retry the encrypted conversation invitation to @{}: {}",
                        peer.username, error.message
                    )
                })?;
            store.mark_mls_welcome_queued(&mls_group_id, &peer.id)?;
        }
    }

    let messages = store.messages()?;
    let mut verified_group_ids = HashSet::new();
    let mut roster_failures = Vec::new();
    let mut relay_failures = Vec::new();
    for message in messages.into_iter().filter(|message| {
        message.is_mine
            && message.encrypted_payload.is_some()
            && (matches!(message.delivery_state.as_str(), "relayed" | "not_delivered")
                || message.group_deliveries.iter().any(|delivery| {
                    !delivery.api_accepted
                        && matches!(
                            delivery.state,
                            RecipientDeliveryState::Pending | RecipientDeliveryState::Delivered
                        )
                }))
    }) {
        let Some(conversation) = conversations
            .iter()
            .find(|conversation| conversation.id == message.conversation_id)
        else {
            continue;
        };
        if conversation.is_group() && !verified_group_ids.contains(&conversation.id) {
            match verify_group_roster(api, token, store, account_id, private_key, conversation) {
                Ok(_) => {
                    verified_group_ids.insert(conversation.id.clone());
                }
                Err(error) => {
                    roster_failures.push(format!("{}: {error}", conversation.id));
                    continue;
                }
            }
        }
        if conversation.is_group() {
            if message.group_deliveries.is_empty() {
                continue;
            }
            let group_id = Some(conversation.id.as_str());
            let group_commit_pending = store.group_commit_delivery_pending(&conversation.id)?;
            for mut delivery in message.group_deliveries.clone() {
                if !group_delivery_retry_due(&delivery, now)? {
                    continue;
                }
                if !conversation
                    .members
                    .iter()
                    .any(|member| member.id == delivery.recipient_id)
                {
                    delivery.state = RecipientDeliveryState::Rejected;
                    delivery.last_error = Some("The recipient is no longer a group member.".into());
                    store.update_group_delivery(&message.id, delivery)?;
                    continue;
                }
                let Some(peer) = friends
                    .iter()
                    .find(|friend| friend.id == delivery.recipient_id)
                else {
                    delivery.attempts = delivery.attempts.saturating_add(1);
                    delivery.last_error = Some("The recipient is not currently available.".into());
                    if delivery.attempts >= GROUP_RELAY_MAX_ATTEMPTS {
                        delivery.state = RecipientDeliveryState::Failed;
                        delivery.next_attempt_at = None;
                    } else {
                        delivery.state = RecipientDeliveryState::Pending;
                        delivery.next_attempt_at =
                            Some(group_delivery_retry_at(delivery.attempts, now));
                    }
                    store.update_group_delivery(&message.id, delivery)?;
                    continue;
                };
                if !group_commit_pending
                    && peer_transport
                        .is_some_and(|transport| transport.send(peer, &message).is_ok())
                {
                    delivery.state = RecipientDeliveryState::Delivered;
                    delivery.delivered_at = Some(timestamp()?);
                    store.update_group_delivery(&message.id, delivery.clone())?;
                }
                let Some(payload) = message.encrypted_payload.as_deref() else {
                    delivery.state = RecipientDeliveryState::Rejected;
                    delivery.last_error = Some("The message has not been encrypted.".into());
                    store.update_group_delivery(&message.id, delivery)?;
                    continue;
                };
                let envelope = match relay::encrypt(
                    private_key,
                    &peer.id,
                    &peer.id,
                    &message.id,
                    &message.sent_at,
                    group_id,
                    payload,
                ) {
                    Ok(envelope) => envelope,
                    Err(error) => {
                        delivery.state = RecipientDeliveryState::Rejected;
                        delivery.last_error = Some(error);
                        store.update_group_delivery(&message.id, delivery)?;
                        continue;
                    }
                };
                match api.send_relay_message(token, &peer.id, group_id, &envelope) {
                    Ok(_) => {
                        delivery.api_accepted = true;
                        delivery.api_failure_permanent = false;
                        if delivery.state != RecipientDeliveryState::Delivered {
                            delivery.state = RecipientDeliveryState::Pending;
                        }
                        delivery.attempts = delivery.attempts.max(1);
                        delivery.next_attempt_at = None;
                        delivery.last_error = None;
                    }
                    Err(error) => {
                        delivery.attempts = delivery.attempts.saturating_add(1);
                        delivery.last_error = Some(error.message.clone());
                        delivery.next_attempt_at = None;
                        if permanent_group_delivery_error(&error) {
                            delivery.api_failure_permanent = true;
                            delivery.state = RecipientDeliveryState::Rejected;
                        } else if delivery.attempts >= GROUP_RELAY_MAX_ATTEMPTS {
                            delivery.api_failure_permanent = false;
                            if delivery.state != RecipientDeliveryState::Delivered {
                                delivery.state = RecipientDeliveryState::Failed;
                            }
                        } else if delivery.state != RecipientDeliveryState::Delivered {
                            delivery.api_failure_permanent = false;
                            delivery.state = RecipientDeliveryState::Pending;
                            delivery.next_attempt_at =
                                Some(group_delivery_retry_at(delivery.attempts, now));
                        } else {
                            delivery.api_failure_permanent = false;
                            delivery.next_attempt_at =
                                Some(group_delivery_retry_at(delivery.attempts, now));
                        }
                        relay_failures.push(format!(
                            "Could not retry delivery to @{}: {}",
                            peer.username, error.message
                        ));
                    }
                }
                store.update_group_delivery(&message.id, delivery)?;
            }
            continue;
        }
        let recipients = if conversation.is_group() {
            conversation
                .members
                .iter()
                .filter(|member| member.id != account_id)
                .filter_map(|member| friends.iter().find(|friend| friend.id == member.id))
                .collect::<Vec<_>>()
        } else {
            friends
                .iter()
                .find(|friend| friend.id == conversation.user_id)
                .into_iter()
                .collect()
        };
        if recipients.is_empty() {
            continue;
        }
        let group_id = conversation.is_group().then_some(conversation.id.as_str());
        let group_commit_pending =
            conversation.is_group() && store.group_commit_delivery_pending(&conversation.id)?;
        let relay_retry_due = match message.delivery_state.as_str() {
            "not_delivered" => true,
            "relayed" => match message.last_queued_at.as_deref() {
                Some(queued_at) => {
                    let queued_at = queued_at.parse::<u64>().map_err(|error| {
                        format!("The queued message timestamp is invalid: {error}")
                    })?;
                    now.saturating_sub(queued_at) >= RELAY_RETRY_INTERVAL_SECS
                }
                None => true,
            },
            _ => false,
        };
        let mut all_delivered = true;
        let mut relay_delivered = false;
        let mut permanently_failed = false;
        let mut delivery_failures = Vec::new();
        for peer in recipients {
            let directly_delivered = !group_commit_pending
                && peer_transport.is_some_and(|transport| transport.send(peer, &message).is_ok());
            if directly_delivered {
                continue;
            }
            all_delivered = false;
            if relay_retry_due {
                let envelope = relay::encrypt(
                    private_key,
                    &peer.id,
                    &peer.id,
                    &message.id,
                    &message.sent_at,
                    group_id,
                    message
                        .encrypted_payload
                        .as_deref()
                        .ok_or_else(|| "The message has not been encrypted.".to_string())?,
                )?;
                match api.send_relay_message(token, &peer.id, group_id, &envelope) {
                    Ok(_) => relay_delivered = true,
                    Err(error) => {
                        permanently_failed |= !conversation.is_group()
                            && matches!(error.status, Some(400 | 403 | 404 | 409));
                        delivery_failures.push(format!(
                            "Could not retry delivery to @{}: {}",
                            peer.username, error.message
                        ));
                    }
                }
            }
        }
        if all_delivered {
            store.mark_delivered(&message.id)?;
        } else if permanently_failed {
            store.set_delivery_state(&message.id, "failed")?;
        } else if relay_delivered {
            store.set_delivery_state(&message.id, "relayed")?;
        }
        relay_failures.extend(delivery_failures);
    }
    if roster_failures.is_empty() && relay_failures.is_empty() {
        Ok(())
    } else {
        roster_failures.extend(relay_failures);
        Err(roster_failures.join("; "))
    }
}

pub(super) fn poll_account(
    api: AccountApiClient,
    token: String,
    account_id: String,
    private_key: Zeroizing<Vec<u8>>,
    store: LocalChatStore,
    known_friends: Vec<User>,
    peer_transport: Option<PeerTransport>,
    port: Option<u16>,
    read_conversation_id: Option<String>,
) -> RemotePollReport {
    let api = match api.independent_client() {
        Ok(api) => api,
        Err(error) => {
            return RemotePollReport {
                friends: None,
                friend_requests: None,
                outgoing_friend_requests: None,
                new_friends: Vec::new(),
                messages: store.messages(),
                conversations: store.conversations(),
                voice_signals: Vec::new(),
                errors: vec![error.message],
            };
        }
    };
    let mut errors = Vec::new();
    if let Some(port) = port {
        if let Err(error) = api.publish_presence(&token, port) {
            errors.push(error.message);
        }
    }
    let friends = match api.friends(&token) {
        Ok(friends) => Some(friends),
        Err(error) => {
            errors.push(error.message);
            None
        }
    };
    let friend_requests = match api.friend_requests(&token) {
        Ok(requests) => Some(requests),
        Err(error) => {
            errors.push(error.message);
            None
        }
    };
    let outgoing_friend_requests = match api.outgoing_friend_requests(&token) {
        Ok(requests) => Some(requests),
        Err(error) => {
            errors.push(error.message);
            None
        }
    };
    let new_friends = match friends.as_deref() {
        Some(friends) => match store.sync_friends(friends) {
            Ok(new_friends) => new_friends,
            Err(error) => {
                errors.push(error);
                Vec::new()
            }
        },
        None => Vec::new(),
    };
    let message_friends = friends.as_ref().unwrap_or(&known_friends);
    if let Err(error) = retry_pending_group_membership_changes(
        &api,
        &token,
        &store,
        &account_id,
        private_key.as_slice(),
    ) {
        errors.push(error);
    }
    let mut voice_signals = Vec::new();
    let acknowledge_queued =
        |message_id: &str, lease_token: Option<&str>, state: &str| -> Result<(), AccountApiError> {
            let lease_token = lease_token.ok_or_else(|| AccountApiError {
                status: None,
                message: "The message has no active device delivery lease.".into(),
            })?;
            api.acknowledge_relay_message(&token, message_id, lease_token, state)?;
            store
                .remove_pending_inbound_relay(message_id)
                .map_err(|message| AccountApiError {
                    status: None,
                    message,
                })
        };
    let release_queued = |message_id: &str,
                          lease_token: Option<&str>,
                          failure_code: &str|
     -> Result<(), AccountApiError> {
        let Some(lease_token) = lease_token else {
            return Ok(());
        };
        api.release_relay_message(&token, message_id, lease_token, failure_code)
    };
    let remote_pending = match api.pending_relay_messages(&token) {
        Ok(pending) => pending,
        Err(error) => {
            errors.push(error.message);
            Vec::new()
        }
    };
    for queued in &remote_pending {
        if let Err(error) = store.save_pending_inbound_relay(
            &queued.sender_public_key,
            &queued.envelope,
            Some(&queued.lease_token),
            queued.queue_sequence,
        ) {
            errors.push(error);
            if let Err(error) = api.release_relay_message(
                &token,
                &queued.envelope.message_id,
                &queued.lease_token,
                "temporary",
            ) {
                errors.push(error.message);
            }
        }
    }
    let pending = match store.pending_inbound_relays() {
        Ok(pending) => pending,
        Err(error) => {
            errors.push(error);
            Vec::new()
        }
    };
    for queued in pending {
        let friend = message_friends
            .iter()
            .find(|friend| friend.id == queued.sender_public_key);
        let body = match relay::decrypt(
            private_key.as_slice(),
            &account_id,
            &queued.sender_public_key,
            &queued.envelope,
        ) {
            Ok(body) => body,
            Err(error) => {
                errors.push(error);
                if let Err(error) = acknowledge_queued(
                    &queued.envelope.message_id,
                    queued.lease_token.as_deref(),
                    "rejected",
                ) {
                    errors.push(error.message);
                }
                continue;
            }
        };
        let voice_signal = match VoiceSignal::decode(&body) {
            Ok(signal) => signal,
            Err(error) => {
                errors.push(error);
                if let Err(error) = acknowledge_queued(
                    &queued.envelope.message_id,
                    queued.lease_token.as_deref(),
                    "rejected",
                ) {
                    errors.push(error.message);
                }
                continue;
            }
        };
        if let Some(signal) = voice_signal {
            if signal.action == VoiceSignalAction::Invite {
                let now = chrono::Utc::now().timestamp();
                let is_expired = queued
                    .envelope
                    .sent_at
                    .parse::<i64>()
                    .map(|sent_at| now.saturating_sub(sent_at) > 90 || sent_at > now + 30)
                    .unwrap_or(true);
                if is_expired {
                    if let Err(error) = acknowledge_queued(
                        &queued.envelope.message_id,
                        queued.lease_token.as_deref(),
                        "rejected",
                    ) {
                        errors.push(error.message);
                    }
                    continue;
                }
            }
            let local_conversation = if let Some(group_id) = queued.envelope.group_id.as_deref() {
                let conversations = match store.conversations() {
                    Ok(conversations) => conversations,
                    Err(error) => {
                        errors.push(error);
                        continue;
                    }
                };
                let Some(conversation) = conversations
                    .into_iter()
                    .find(|conversation| conversation.id == group_id && conversation.is_group())
                else {
                    errors.push("The voice signal refers to an unknown group.".into());
                    if let Err(error) = acknowledge_queued(
                        &queued.envelope.message_id,
                        queued.lease_token.as_deref(),
                        "rejected",
                    ) {
                        errors.push(error.message);
                    }
                    continue;
                };
                if signal.conversation_id != group_id
                    || !conversation
                        .members
                        .iter()
                        .any(|member| member.id == queued.sender_public_key)
                    || !conversation
                        .members
                        .iter()
                        .any(|member| member.id == signal.initiator_id)
                {
                    errors.push("The voice signal does not match the group members.".into());
                    if let Err(error) = acknowledge_queued(
                        &queued.envelope.message_id,
                        queued.lease_token.as_deref(),
                        "rejected",
                    ) {
                        errors.push(error.message);
                    }
                    continue;
                }
                conversation.id
            } else {
                if signal.conversation_id != signal.call_id
                    || ![account_id.as_str(), queued.sender_public_key.as_str()]
                        .contains(&signal.initiator_id.as_str())
                {
                    errors.push("The direct voice signal is invalid.".into());
                    if let Err(error) = acknowledge_queued(
                        &queued.envelope.message_id,
                        queued.lease_token.as_deref(),
                        "rejected",
                    ) {
                        errors.push(error.message);
                    }
                    continue;
                }
                let Some(friend) = message_friends
                    .iter()
                    .find(|friend| friend.id == queued.sender_public_key)
                else {
                    errors.push("A non-friend sent a direct voice signal.".into());
                    if let Err(error) = acknowledge_queued(
                        &queued.envelope.message_id,
                        queued.lease_token.as_deref(),
                        "rejected",
                    ) {
                        errors.push(error.message);
                    }
                    continue;
                };
                match store.conversation_for(friend) {
                    Ok(conversation) => conversation.id,
                    Err(error) => {
                        errors.push(error);
                        continue;
                    }
                }
            };
            if signal.action == VoiceSignalAction::Invite
                && signal.initiator_id != queued.sender_public_key
            {
                errors.push("Only the call initiator can send an invitation.".into());
                if let Err(error) = acknowledge_queued(
                    &queued.envelope.message_id,
                    queued.lease_token.as_deref(),
                    "rejected",
                ) {
                    errors.push(error.message);
                }
                continue;
            }
            if (signal.action == VoiceSignalAction::End
                && signal.initiator_id != queued.sender_public_key)
                || (signal.action == VoiceSignalAction::Leave
                    && signal.initiator_id == queued.sender_public_key)
            {
                errors.push("The voice signal is not authorized for its sender.".into());
                if let Err(error) = acknowledge_queued(
                    &queued.envelope.message_id,
                    queued.lease_token.as_deref(),
                    "rejected",
                ) {
                    errors.push(error.message);
                }
                continue;
            }
            voice_signals.push(ReceivedVoiceSignal {
                sender_id: queued.sender_public_key.clone(),
                local_conversation_id: local_conversation,
                signal,
            });
            if let Err(error) = acknowledge_queued(
                &queued.envelope.message_id,
                queued.lease_token.as_deref(),
                "delivered",
            ) {
                errors.push(error.message);
            }
            continue;
        }
        let already_received = match store.messages() {
            Ok(messages) => messages
                .iter()
                .any(|message| message.id == queued.envelope.message_id),
            Err(error) => {
                errors.push(error);
                continue;
            }
        };
        let received = if already_received {
            if let Some(queue_sequence) = queued.queue_sequence
                && let Err(error) = store.set_message_queue_sequence(
                    &queued.envelope.message_id,
                    queue_sequence,
                )
            {
                errors.push(error);
            }
            Ok(None)
        } else {
            process_mls_relay_body(
                &body,
                &queued.envelope,
                queued.queue_sequence,
                &queued.sender_public_key,
                &account_id,
                private_key.as_slice(),
                &store,
                friend,
            )
        };
        let conversation = match received {
            Ok(Some(conversation)) => conversation,
            Ok(None) => {
                if let Err(error) = store.mark_delivered(&queued.envelope.message_id) {
                    errors.push(error);
                    if let Err(error) = release_queued(
                        &queued.envelope.message_id,
                        queued.lease_token.as_deref(),
                        "temporary",
                    ) {
                        errors.push(error.message);
                    }
                    continue;
                }
                if let Err(error) = acknowledge_queued(
                    &queued.envelope.message_id,
                    queued.lease_token.as_deref(),
                    "delivered",
                ) {
                    errors.push(error.message);
                }
                continue;
            }
            Err(error) => {
                let retryable = retryable_mls_delivery_error(&error);
                errors.push(error);
                let failure_code = if retryable {
                    "missing_mls_state"
                } else {
                    "temporary"
                };
                if let Err(error) = release_queued(
                    &queued.envelope.message_id,
                    queued.lease_token.as_deref(),
                    failure_code,
                ) {
                    errors.push(error.message);
                }
                continue;
            }
        };
        if let Err(error) = store.mark_delivered(&queued.envelope.message_id) {
            errors.push(error);
            if let Err(error) = release_queued(
                &queued.envelope.message_id,
                queued.lease_token.as_deref(),
                "temporary",
            ) {
                errors.push(error.message);
            }
            continue;
        }
        if let Err(error) = acknowledge_queued(
            &queued.envelope.message_id,
            queued.lease_token.as_deref(),
            "delivered",
        ) {
            errors.push(error.message);
            continue;
        }
        if read_conversation_id.as_deref() == Some(conversation.id.as_str()) {
            match store.mark_conversation_read(&conversation.id) {
                Ok(read_message_ids) => {
                    for message_id in read_message_ids {
                        if let Err(error) = api.send_message_receipt(&token, &message_id, "read") {
                            errors.push(error.message);
                        }
                    }
                }
                Err(error) => errors.push(error),
            }
        }
        continue;
    }
    errors.extend(reconcile_group_rosters(
        &api,
        &token,
        &store,
        &account_id,
        private_key.as_slice(),
    ));
    if let Err(error) = retry_pending_direct_messages(
        &api,
        &token,
        private_key.as_slice(),
        &store,
        &account_id,
        friends.as_deref().unwrap_or(&known_friends),
        peer_transport.as_ref(),
    ) {
        errors.push(error);
    }
    match api.message_receipts(&token) {
        Ok(receipts) => {
            let mut group_commit_receipts: HashMap<String, (usize, bool)> = HashMap::new();
            for receipt in &receipts {
                let entry = group_commit_receipts
                    .entry(receipt.message_id.clone())
                    .or_insert((0, true));
                entry.0 += 1;
                entry.1 &= receipt.delivered_at.is_some();
            }
            let delivered_group_commits = group_commit_receipts
                .into_iter()
                .filter(|(_, (recipient_count, all_delivered))| {
                    *recipient_count > 0 && *all_delivered
                })
                .map(|(message_id, _)| message_id)
                .collect::<Vec<_>>();
            if let Err(error) = store.acknowledge_group_commit_deliveries(&delivered_group_commits)
            {
                errors.push(error);
            }
            let local_messages = match store.messages() {
                Ok(messages) => messages,
                Err(error) => {
                    errors.push(error);
                    Vec::new()
                }
            };
            let local_ids = local_messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<HashSet<_>>();
            let group_ids = match store.conversations() {
                Ok(conversations) => conversations
                    .into_iter()
                    .filter(|conversation| conversation.is_group())
                    .map(|conversation| conversation.id)
                    .collect::<HashSet<_>>(),
                Err(error) => {
                    errors.push(error);
                    HashSet::new()
                }
            };
            for receipt in receipts {
                if !local_ids.contains(receipt.message_id.as_str()) {
                    continue;
                }
                let is_group = local_messages
                    .iter()
                    .find(|message| message.id == receipt.message_id)
                    .is_some_and(|message| group_ids.contains(&message.conversation_id));
                if is_group {
                    let Some(state) = receipt
                        .delivery_state
                        .as_deref()
                        .and_then(RecipientDeliveryState::from_api)
                    else {
                        errors.push(format!(
                            "The server returned an invalid group delivery state for message {}.",
                            receipt.message_id
                        ));
                        continue;
                    };
                    let Some(recipient_id) = receipt.recipient_public_key else {
                        errors.push(format!(
                            "The server omitted a group recipient for message {}.",
                            receipt.message_id
                        ));
                        continue;
                    };
                    let delivery = GroupRecipientDelivery {
                        recipient_id,
                        state,
                        api_accepted: true,
                        api_failure_permanent: false,
                        attempts: receipt.attempts.unwrap_or_default().min(u32::from(u8::MAX))
                            as u8,
                        next_attempt_at: receipt.next_retry_at,
                        delivered_at: receipt.delivered_at,
                        read_at: receipt.read_at,
                        last_error: receipt.failure_code,
                        device_available: receipt.device_available.unwrap_or(false),
                        active_devices: receipt.active_devices.unwrap_or_default(),
                        pending_devices: receipt.pending_devices.unwrap_or_default(),
                        delivered_devices: receipt.delivered_devices.unwrap_or_default(),
                        failed_devices: receipt.failed_devices.unwrap_or_default(),
                        rejected_devices: receipt.rejected_devices.unwrap_or_default(),
                        expired_devices: receipt.expired_devices.unwrap_or_default(),
                    };
                    if let Err(error) = store.update_group_delivery(&receipt.message_id, delivery) {
                        errors.push(error);
                    }
                } else if let Err(error) =
                    store.update_receipt(&receipt.message_id, receipt.delivered_at, receipt.read_at)
                {
                    errors.push(error);
                }
            }
        }
        Err(error) => errors.push(error.message),
    }
    let local_ids = match store.messages() {
        Ok(messages) => messages
            .iter()
            .map(|message| message.id.clone())
            .collect::<std::collections::HashSet<_>>(),
        Err(error) => {
            errors.push(error);
            std::collections::HashSet::new()
        }
    };
    match api.inbound_message_receipts(&token) {
        Ok(receipts) => {
            for receipt in receipts {
                if local_ids.contains(&receipt.message_id)
                    && let Err(error) = store.update_receipt(
                        &receipt.message_id,
                        receipt.delivered_at,
                        receipt.read_at,
                    )
                {
                    errors.push(error);
                }
            }
        }
        Err(error) => errors.push(error.message),
    }
    match store.messages() {
        Ok(messages) => {
            for message in messages.into_iter().filter(|message| {
                !message.is_mine
                    && message.read_at.is_some()
                    && message.delivery_state != "received_direct"
                    && !message.deleted
            }) {
                if let Err(error) = api.send_message_receipt(&token, &message.id, "read") {
                    errors.push(error.message);
                }
            }
        }
        Err(error) => errors.push(error),
    }
    RemotePollReport {
        friends,
        friend_requests,
        outgoing_friend_requests,
        new_friends,
        messages: store.messages(),
        conversations: store.conversations(),
        voice_signals,
        errors,
    }
}
