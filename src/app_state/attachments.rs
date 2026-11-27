use super::*;

impl AppState {
    pub fn choose_attachment(&mut self, cx: &mut Context<Self>) {
        if self.editing_message_id.is_some() {
            self.set_toast(
                Some("Finish editing the message before attaching a file.".into()),
                cx,
            );
            cx.notify();
            return;
        }
        if let Err(error) = self.attachment_destination() {
            self.set_toast(Some(error), cx);
            cx.notify();
            return;
        }
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Choose a file to send".into()),
        });
        cx.spawn(async move |this, cx| match receiver.await {
            Ok(Ok(Some(paths))) => {
                if let Some(path) = paths.into_iter().next() {
                    let _ = this.update(cx, |state, cx| state.upload_attachment_path(path, cx));
                }
            }
            Ok(Ok(None)) => {}
            Ok(Err(error)) => {
                let error = error.to_string();
                let _ = this.update(cx, |state, cx| {
                    state.set_toast(Some(format!("Could not choose a file: {error}")), cx);
                    cx.notify();
                });
            }
            Err(error) => {
                let error = error.to_string();
                let _ = this.update(cx, |state, cx| {
                    state.set_toast(Some(format!("Could not choose a file: {error}")), cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub(super) fn upload_attachment_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let destination = match self.attachment_destination() {
            Ok(destination) => destination,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let Some(file_name) = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
        else {
            self.set_toast(Some("The selected file has no valid name.".into()), cx);
            cx.notify();
            return;
        };
        let file = match std::fs::metadata(&path) {
            Ok(file) if file.is_file() && file.len() > 0 => file,
            Ok(_) => {
                self.set_toast(Some("Choose a non-empty file.".into()), cx);
                cx.notify();
                return;
            }
            Err(error) => {
                self.set_toast(
                    Some(format!("Could not read the selected file: {error}")),
                    cx,
                );
                cx.notify();
                return;
            }
        };
        let max_attachment_bytes = match attachment::max_attachment_bytes() {
            Ok(limit) => limit,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        if file.len() > max_attachment_bytes as u64 {
            self.set_toast(
                Some("The file exceeds the configured attachment size limit.".into()),
                cx,
            );
            cx.notify();
            return;
        }
        let content_type = attachment_content_type(&path).to_string();
        let upload_id = match self.reserve_attachment_upload(
            file_name.clone(),
            file.len() as usize,
            AttachmentKind::File,
            destination.conversation_id.clone(),
        ) {
            Ok(id) => id,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let task_upload_id = upload_id.clone();
        let background = cx.background_executor().clone();
        self.set_toast(
            Some("Encrypting attachment for direct peer transfer…".into()),
            cx,
        );
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = background
                .spawn(async move {
                    let contents = read_attachment_file(&path, max_attachment_bytes)?;
                    upload_attachment_data(
                        destination,
                        contents,
                        file_name,
                        content_type,
                        AttachmentKind::File,
                        task_upload_id,
                    )
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                state.finish_attachment_upload(&upload_id, result, cx);
            });
        })
        .detach();
    }

    pub fn preview_image_attachment(
        &mut self,
        metadata: AttachmentMetadata,
        cx: &mut Context<Self>,
    ) {
        if !attachment::is_image_previewable(&metadata) {
            self.set_toast(Some("This image format cannot be previewed.".into()), cx);
            cx.notify();
            return;
        }
        if let Err(error) = attachment::validate_metadata(&metadata) {
            self.set_toast(Some(error), cx);
            cx.notify();
            return;
        }
        if !self.ensure_no_attachment_transfer(cx) {
            return;
        }
        let format = match metadata.content_type.as_str() {
            "image/png" => ImageFormat::Png,
            "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
            "image/gif" => ImageFormat::Gif,
            "image/webp" => ImageFormat::Webp,
            "image/bmp" => ImageFormat::Bmp,
            "image/tiff" => ImageFormat::Tiff,
            _ => {
                self.set_toast(Some("This image format cannot be previewed.".into()), cx);
                cx.notify();
                return;
            }
        };
        let (store, source) = match self.attachment_source(&metadata) {
            Ok(source) => source,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.attachment_download_cancel = Some(cancel.clone());
        let attachment_id = metadata.attachment_id.clone();
        let progress = Arc::new(AttachmentTransferProgress::default());
        progress
            .total
            .store(metadata.size as u64 + 16, Ordering::Relaxed);
        self.start_attachment_progress_monitor(attachment_id.clone(), progress.clone(), cx);
        let file_name = metadata.file_name.clone();
        let background = cx.background_executor().clone();
        self.image_preview = None;
        self.image_preview_loading = Some(attachment_id.clone());
        self.set_toast(Some("Loading image preview…".into()), cx);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = background
                .spawn(async move {
                    fetch_decrypted_attachment(
                        &store,
                        source.as_ref(),
                        &metadata,
                        &cancel,
                        &progress,
                    )
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                if state.image_preview_loading.as_deref() != Some(&attachment_id) {
                    return;
                }
                state.image_preview_loading = None;
                state.finish_attachment_transfer(
                    &attachment_id,
                    attachment_transfer_state_for_result(&result),
                );
                match result {
                    Ok(bytes) => {
                        state.image_preview = Some(ImagePreview {
                            file_name,
                            image: Arc::new(Image::from_bytes(format, bytes.to_vec())),
                        });
                        state.set_toast(None, cx);
                    }
                    Err(error) => state.set_toast(Some(error), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn close_image_preview(&mut self, cx: &mut Context<Self>) {
        self.image_preview = None;
        self.image_preview_loading = None;
        self.set_toast(None, cx);
        cx.notify();
    }

    pub fn toggle_audio_playback(&mut self, metadata: AttachmentMetadata, cx: &mut Context<Self>) {
        let attachment_id = metadata.attachment_id.clone();
        if self.audio_loading_attachment_id.as_deref() == Some(&attachment_id) {
            return;
        }
        if self.audio_playback_id.as_deref() == Some(&attachment_id) {
            if let Some(player) = self.audio_player.as_ref()
                && !player.is_empty()
            {
                if player.is_paused() {
                    player.play();
                } else {
                    player.pause();
                }
                cx.notify();
                return;
            }
            self.audio_player = None;
            self.audio_playback_id = None;
        }
        if !attachment::is_audio_playable(&metadata) {
            self.set_toast(Some("This audio format cannot be played.".into()), cx);
            cx.notify();
            return;
        }
        if let Err(error) = attachment::validate_metadata(&metadata) {
            self.set_toast(Some(error), cx);
            cx.notify();
            return;
        }
        if !self.ensure_no_attachment_transfer(cx) {
            return;
        }
        let (store, source) = match self.attachment_source(&metadata) {
            Ok(source) => source,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.attachment_download_cancel = Some(cancel.clone());
        let progress = Arc::new(AttachmentTransferProgress::default());
        progress
            .total
            .store(metadata.size as u64 + 16, Ordering::Relaxed);
        self.start_attachment_progress_monitor(attachment_id.clone(), progress.clone(), cx);
        self.audio_player = None;
        self.audio_playback_id = None;
        self.audio_loading_attachment_id = Some(attachment_id.clone());
        let background = cx.background_executor().clone();
        self.set_toast(Some("Loading audio…".into()), cx);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = background
                .spawn(async move {
                    fetch_decrypted_attachment(
                        &store,
                        source.as_ref(),
                        &metadata,
                        &cancel,
                        &progress,
                    )
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                if state.audio_loading_attachment_id.as_deref() != Some(&attachment_id) {
                    return;
                }
                state.audio_loading_attachment_id = None;
                state.finish_attachment_transfer(
                    &attachment_id,
                    attachment_transfer_state_for_result(&result),
                );
                match result {
                    Ok(bytes) => match AudioPlayer::start(bytes) {
                        Ok(player) => {
                            state.audio_player = Some(player);
                            state.audio_playback_id = Some(attachment_id.clone());
                            state.set_toast(None, cx);
                            let monitor_id = attachment_id.clone();
                            cx.spawn(async move |this, cx| {
                                loop {
                                    cx.background_executor()
                                        .timer(Duration::from_millis(250))
                                        .await;
                                    let keep_monitoring = this
                                        .update(cx, |state, cx| {
                                            if state.audio_playback_id.as_deref()
                                                != Some(&monitor_id)
                                            {
                                                return false;
                                            }
                                            if state
                                                .audio_player
                                                .as_ref()
                                                .is_some_and(AudioPlayer::is_empty)
                                            {
                                                state.audio_player = None;
                                                state.audio_playback_id = None;
                                                cx.notify();
                                                false
                                            } else {
                                                true
                                            }
                                        })
                                        .unwrap_or(false);
                                    if !keep_monitoring {
                                        break;
                                    }
                                }
                            })
                            .detach();
                        }
                        Err(error) => state.set_toast(Some(error), cx),
                    },
                    Err(error) => state.set_toast(Some(error), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn is_audio_loading(&self, attachment_id: &str) -> bool {
        self.audio_loading_attachment_id.as_deref() == Some(attachment_id)
    }

    pub fn attachment_download_percent(&self, attachment_id: &str) -> Option<u8> {
        let (active_id, progress) = self.attachment_download_progress.as_ref()?;
        if active_id != attachment_id {
            return None;
        }
        let total = progress.total.load(Ordering::Relaxed);
        if total == 0 {
            return Some(0);
        }
        Some(((progress.transferred.load(Ordering::Relaxed).min(total) * 100) / total) as u8)
    }

    pub fn attachment_download_speed(&self, attachment_id: &str) -> Option<u64> {
        let (active_id, progress) = self.attachment_download_progress.as_ref()?;
        if active_id != attachment_id {
            return None;
        }
        let baseline = progress.speed_baseline.load(Ordering::Relaxed);
        if baseline == u64::MAX {
            return None;
        }
        let elapsed = progress.started.elapsed().as_secs_f64().max(0.001);
        let transferred = progress
            .transferred
            .load(Ordering::Relaxed)
            .saturating_sub(baseline);
        Some((transferred as f64 / elapsed) as u64)
    }

    pub fn attachment_transfer_status(
        &self,
        attachment_id: &str,
    ) -> Result<AttachmentTransferState, String> {
        if self.is_attachment_downloading(attachment_id) {
            return Ok(AttachmentTransferState::Active);
        }
        if let Some(store) = self.local_chat.as_ref() {
            if let Some((confirmed, expected)) =
                store.attachment_transfer_receipt_status(attachment_id)?
            {
                if confirmed == expected {
                    return Ok(AttachmentTransferState::Completed);
                }
                if confirmed > 0 {
                    return Ok(AttachmentTransferState::AwaitingConfirmation);
                }
            }
            if let Some((sent, total, _)) = store.attachment_serving_progress(attachment_id)? {
                return Ok(if sent >= total {
                    AttachmentTransferState::AwaitingConfirmation
                } else {
                    AttachmentTransferState::Active
                });
            }
        }
        if let Some(status) = self.attachment_transfer_states.get(attachment_id) {
            return Ok(*status);
        }
        if let Some(store) = self.local_chat.as_ref()
            && store.has_attachment_ciphertext(attachment_id)?
        {
            let is_mine = self
                .messages
                .iter()
                .find(|message| {
                    message
                        .attachments()
                        .any(|attachment| attachment.attachment_id == attachment_id)
                })
                .is_some_and(|message| message.is_mine);
            return Ok(if is_mine {
                AttachmentTransferState::Pending
            } else {
                AttachmentTransferState::Completed
            });
        }
        Ok(AttachmentTransferState::Pending)
    }

    pub fn attachment_is_outgoing(&self, attachment_id: &str) -> bool {
        self.messages.iter().any(|message| {
            message.is_mine
                && message
                    .attachments()
                    .any(|attachment| attachment.attachment_id == attachment_id)
        })
    }

    pub fn attachment_upload_progress(
        &self,
        attachment_id: &str,
    ) -> Result<Option<(u8, u64)>, String> {
        let Some(store) = self.local_chat.as_ref() else {
            return Ok(None);
        };
        let Some((transferred, total, speed)) = store.attachment_serving_progress(attachment_id)?
        else {
            return Ok(None);
        };
        let percent = if total == 0 {
            0
        } else {
            ((transferred.min(total) * 100) / total) as u8
        };
        Ok(Some((percent, speed)))
    }

    pub fn is_attachment_downloading(&self, attachment_id: &str) -> bool {
        self.attachment_download_progress
            .as_ref()
            .is_some_and(|(active_id, _)| active_id == attachment_id)
    }

    pub(super) fn start_attachment_progress_monitor(
        &mut self,
        attachment_id: String,
        progress: Arc<AttachmentTransferProgress>,
        cx: &mut Context<Self>,
    ) {
        self.attachment_transfer_states.remove(&attachment_id);
        self.attachment_download_progress = Some((attachment_id.clone(), progress.clone()));
        let state = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(200))
                    .await;
                let keep_monitoring = state
                    .update(cx, |state, cx| {
                        if !state.is_attachment_downloading(&attachment_id) {
                            return false;
                        }
                        state.attachment_download_progress =
                            Some((attachment_id.clone(), progress.clone()));
                        cx.notify();
                        true
                    })
                    .unwrap_or(false);
                if !keep_monitoring {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn start_attachment_upload_status_monitor(
        &mut self,
        attachment_ids: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        for id in attachment_ids {
            if self.attachment_upload_monitor_ids.contains(&id) {
                continue;
            }
            if self.attachment_upload_monitor_ids.len() >= 100 {
                self.attachment_upload_monitor_ids.remove(0);
            }
            self.attachment_upload_monitor_ids.push(id);
        }
        if self.attachment_upload_monitor_active || self.attachment_upload_monitor_ids.is_empty() {
            return;
        }
        self.attachment_upload_monitor_active = true;
        let state = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let mut last_snapshot = Vec::new();
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let keep_monitoring = state
                    .update(cx, |state, cx| {
                        let mut remaining = Vec::new();
                        let mut snapshot = Vec::new();
                        for id in state.attachment_upload_monitor_ids.clone() {
                            match state.attachment_transfer_status(&id) {
                                Ok(status) => {
                                    if status != AttachmentTransferState::Completed {
                                        remaining.push(id.clone());
                                    }
                                    let percent = match state.attachment_upload_progress(&id) {
                                        Ok(progress) => progress.map(|(percent, _)| percent),
                                        Err(error) => {
                                            eprintln!(
                                                "Could not read attachment upload progress: {error}"
                                            );
                                            None
                                        }
                                    };
                                    snapshot.push((id, status, percent));
                                }
                                Err(error) => {
                                    eprintln!("Could not read attachment transfer status: {error}");
                                }
                            }
                        }
                        state.attachment_upload_monitor_ids = remaining;
                        if state.attachment_upload_monitor_ids.is_empty() {
                            state.attachment_upload_monitor_active = false;
                            return false;
                        }
                        if snapshot != last_snapshot {
                            last_snapshot = snapshot;
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep_monitoring {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn finish_attachment_transfer(
        &mut self,
        attachment_id: &str,
        status: AttachmentTransferState,
    ) {
        if self
            .attachment_download_progress
            .as_ref()
            .is_some_and(|(active_id, _)| active_id == attachment_id)
        {
            self.attachment_download_cancel = None;
            self.attachment_download_progress = None;
        }
        if matches!(
            status,
            AttachmentTransferState::Failed | AttachmentTransferState::Cancelled
        ) {
            if self.attachment_transfer_states.len() >= 128 {
                self.attachment_transfer_states.clear();
            }
            self.attachment_transfer_states
                .insert(attachment_id.to_string(), status);
        } else {
            self.attachment_transfer_states.remove(attachment_id);
        }
    }

    pub(super) fn ensure_no_attachment_transfer(&mut self, cx: &mut Context<Self>) -> bool {
        if self.attachment_download_cancel.is_some() {
            self.set_toast(
                Some("Wait for the current attachment transfer to finish.".into()),
                cx,
            );
            cx.notify();
            false
        } else {
            true
        }
    }

    pub fn is_audio_playing(&self, attachment_id: &str) -> bool {
        self.audio_playback_id.as_deref() == Some(attachment_id)
            && self
                .audio_player
                .as_ref()
                .is_some_and(|player| !player.is_paused() && !player.is_empty())
    }

    pub fn download_attachment(&mut self, metadata: AttachmentMetadata, cx: &mut Context<Self>) {
        if let Err(error) = attachment::validate_metadata(&metadata) {
            self.set_toast(Some(error), cx);
            cx.notify();
            return;
        }
        if !self.ensure_no_attachment_transfer(cx) {
            return;
        }
        let (store, source) = match self.attachment_source(&metadata) {
            Ok(source) => source,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.attachment_download_cancel = Some(cancel.clone());
        let attachment_id = metadata.attachment_id.clone();
        let directory = attachment_download_directory();
        let receiver = cx.prompt_for_new_path(&directory, Some(&metadata.file_name));
        let background = cx.background_executor().clone();
        self.set_toast(
            Some("Choose where to save the decrypted attachment.".into()),
            cx,
        );
        cx.notify();
        cx.spawn(async move |this, cx| {
            let path = match receiver.await {
                Ok(Ok(Some(path))) => path,
                Ok(Ok(None)) => {
                    let _ = this.update(cx, |state, _| {
                        state.attachment_download_cancel = None;
                    });
                    return;
                }
                Ok(Err(error)) => {
                    let error = error.to_string();
                    let _ = this.update(cx, |state, cx| {
                        state.attachment_download_cancel = None;
                        state.set_toast(
                            Some(format!("Could not choose a save location: {error}")),
                            cx,
                        );
                        cx.notify();
                    });
                    return;
                }
                Err(error) => {
                    let error = error.to_string();
                    let _ = this.update(cx, |state, cx| {
                        state.attachment_download_cancel = None;
                        state.set_toast(
                            Some(format!("Could not choose a save location: {error}")),
                            cx,
                        );
                        cx.notify();
                    });
                    return;
                }
            };
            let progress = Arc::new(AttachmentTransferProgress::default());
            progress
                .total
                .store(metadata.size as u64 + 16, Ordering::Relaxed);
            let started = this.update(cx, |state, cx| {
                state.start_attachment_progress_monitor(
                    attachment_id.clone(),
                    progress.clone(),
                    cx,
                );
            });
            if started.is_err() {
                return;
            }
            let result = background
                .spawn(async move {
                    let plaintext = fetch_decrypted_attachment(
                        &store,
                        source.as_ref(),
                        &metadata,
                        &cancel,
                        &progress,
                    )?;
                    save_attachment_without_overwrite(&path, plaintext.as_slice())?;
                    Ok::<_, String>(path)
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                state.finish_attachment_transfer(
                    &attachment_id,
                    attachment_transfer_state_for_result(&result),
                );
                match result {
                    Ok(path) => state
                        .set_toast(Some(format!("Attachment saved to {}.", path.display())), cx),
                    Err(error) => state.set_toast(Some(error), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn upload_voice_attachment(
        &mut self,
        contents: Zeroizing<Vec<u8>>,
        cx: &mut Context<Self>,
    ) {
        let destination = match self.attachment_destination() {
            Ok(destination) => destination,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let upload_id = match self.reserve_attachment_upload(
            "Voice note.wav".into(),
            contents.len(),
            AttachmentKind::Voice,
            destination.conversation_id.clone(),
        ) {
            Ok(id) => id,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let task_upload_id = upload_id.clone();
        let background = cx.background_executor().clone();
        self.set_toast(Some("Encrypting and uploading voice note…".into()), cx);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = background
                .spawn(async move {
                    upload_attachment_data(
                        destination,
                        contents,
                        "voice-message.wav".into(),
                        "audio/wav".into(),
                        AttachmentKind::Voice,
                        task_upload_id,
                    )
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                state.finish_attachment_upload(&upload_id, result, cx);
            });
        })
        .detach();
    }

    pub(super) fn reserve_attachment_upload(
        &mut self,
        file_name: String,
        size: usize,
        kind: AttachmentKind,
        conversation_id: String,
    ) -> Result<String, String> {
        if self.pending_attachments.len() >= attachment::MAX_ATTACHMENTS_PER_MESSAGE {
            return Err("A message can contain no more than 5 attachments.".into());
        }
        let id = new_id()?;
        self.attachment_uploads_in_progress += 1;
        self.pending_attachments.push(PendingAttachment {
            id: id.clone(),
            file_name,
            size,
            kind,
            conversation_id,
            metadata: None,
        });
        Ok(id)
    }

    pub(super) fn finish_attachment_upload(
        &mut self,
        upload_id: &str,
        result: Result<AttachmentMetadata, String>,
        cx: &mut Context<Self>,
    ) {
        if !self
            .pending_attachments
            .iter()
            .any(|pending| pending.id == upload_id)
        {
            return;
        }
        self.attachment_uploads_in_progress = self.attachment_uploads_in_progress.saturating_sub(1);
        match result {
            Ok(metadata) => {
                if let Some(pending) = self
                    .pending_attachments
                    .iter_mut()
                    .find(|pending| pending.id == upload_id)
                {
                    pending.metadata = Some(metadata);
                    self.set_toast(Some("Attachment added to the message.".into()), cx);
                }
            }
            Err(error) => {
                self.pending_attachments
                    .retain(|pending| pending.id != upload_id);
                self.set_toast(Some(error), cx);
            }
        }
        cx.notify();
    }

    pub fn remove_pending_attachment(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(index) = self
            .pending_attachments
            .iter()
            .position(|pending| pending.id == id && pending.metadata.is_some())
        {
            self.pending_attachments.remove(index);
            self.set_toast(None, cx);
            cx.notify();
        }
    }

    pub(super) fn attachment_destination(&self) -> Result<AttachmentDestination, String> {
        let conversation_id = self
            .selected_conversation
            .clone()
            .ok_or_else(|| "Select a conversation first.".to_string())?;
        let store = self
            .local_chat
            .clone()
            .ok_or_else(|| "Local attachment storage is unavailable.".to_string())?;
        let conversation = self
            .conversation(&conversation_id)
            .ok_or_else(|| "The selected conversation is unavailable.".to_string())?;
        let current_user_id = self
            .current_user
            .as_ref()
            .map(|user| user.id.clone())
            .ok_or_else(|| "Sign in before attaching files.".to_string())?;
        let participants = if conversation.is_group() {
            if !conversation
                .members
                .iter()
                .any(|member| member.id == current_user_id)
            {
                return Err("You are no longer a member of this group.".into());
            }
            conversation
                .members
                .iter()
                .map(|member| member.id.clone())
                .collect::<Vec<_>>()
        } else {
            let peer = self
                .friend(&conversation.user_id)
                .ok_or_else(|| "This account is no longer in your friend list.".to_string())?;
            vec![current_user_id.clone(), peer.id.clone()]
        };
        for participant_id in participants.iter().filter(|id| *id != &current_user_id) {
            let peer = self
                .friends
                .iter()
                .find(|friend| &friend.id == participant_id)
                .or_else(|| {
                    conversation
                        .members
                        .iter()
                        .find(|member| &member.id == participant_id)
                });
            if let Some(peer) = peer {
                store.ensure_identity_not_changed(peer)?;
            }
        }
        let binding_conversation_id = if conversation.is_group() {
            conversation_id.clone()
        } else {
            MlsService::direct_group_id(&current_user_id, &conversation.user_id)?
        };
        let binding = attachment::attachment_binding(
            &binding_conversation_id,
            &current_user_id,
            participants,
        )?;
        Ok(AttachmentDestination {
            conversation_id,
            store,
            binding,
        })
    }

    pub(super) fn attachment_source(
        &self,
        metadata: &AttachmentMetadata,
    ) -> Result<(LocalChatStore, Option<(PeerTransport, User, String)>), String> {
        let store = self
            .local_chat
            .clone()
            .ok_or_else(|| "Local attachment storage is unavailable.".to_string())?;
        let message = self
            .messages
            .iter()
            .find(|message| {
                message
                    .attachments()
                    .any(|attachment| attachment.attachment_id == metadata.attachment_id)
            })
            .ok_or_else(|| {
                "The attachment is not part of local conversation history.".to_string()
            })?;
        let current_user_id = self
            .current_user
            .as_ref()
            .map(|user| user.id.as_str())
            .ok_or_else(|| "Sign in before accessing attachments.".to_string())?;
        let conversation = self
            .conversations
            .iter()
            .find(|conversation| conversation.id == message.conversation_id)
            .ok_or_else(|| "The attachment conversation is no longer available.".to_string())?;
        let transfer_conversation_id = if conversation.is_group() {
            conversation.id.clone()
        } else {
            MlsService::direct_group_id(current_user_id, &conversation.user_id)?
        };
        let mut participant_ids = if conversation.is_group() {
            if !conversation
                .members
                .iter()
                .any(|member| member.id == current_user_id)
                || !conversation
                    .members
                    .iter()
                    .any(|member| member.id == message.author_id)
            {
                return Err("The attachment sender or recipient is not in this group.".into());
            }
            conversation
                .members
                .iter()
                .map(|member| member.id.clone())
                .collect::<Vec<_>>()
        } else {
            if conversation.user_id != message.author_id && message.author_id != current_user_id {
                return Err("The attachment does not belong to this conversation.".into());
            }
            vec![current_user_id.to_string(), conversation.user_id.clone()]
        };
        participant_ids.sort();
        participant_ids.dedup();
        if let Some(binding) = metadata.binding.as_ref() {
            if binding.conversation_id != transfer_conversation_id
                || binding.sender_id != message.author_id
                || binding.participants != participant_ids
            {
                return Err(
                    "The attachment identity or conversation binding does not match.".into(),
                );
            }
        }
        if message.author_id == current_user_id {
            return Ok((store, None));
        }
        let peer = self
            .friend(&message.author_id)
            .cloned()
            .or_else(|| {
                conversation
                    .members
                    .iter()
                    .find(|member| member.id == message.author_id)
                    .cloned()
            })
            .ok_or_else(|| {
                "The attachment sender is not a conversation participant.".to_string()
            })?;
        store.ensure_identity_not_changed(&peer)?;
        let transport = self
            .peer_transport
            .clone()
            .ok_or_else(|| "Direct peer networking is unavailable.".to_string())?;
        Ok((store, Some((transport, peer, transfer_conversation_id))))
    }

    pub fn cancel_attachment_download(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.attachment_download_cancel {
            cancel.store(true, Ordering::Relaxed);
            self.set_toast(Some("Cancelling the attachment transfer…".into()), cx);
            cx.notify();
        }
    }
}
