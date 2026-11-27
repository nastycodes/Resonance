use super::*;

impl AppState {
    pub fn send_message(&mut self, cx: &mut Context<Self>) {
        let body = self.message_input.read(cx).text().to_string();
        self.send_message_text(body, cx);
    }

    pub(super) fn send_message_text(&mut self, body: String, cx: &mut Context<Self>) {
        if self.active_message_send_id.is_some() {
            self.set_toast(Some("A message is still being sent.".into()), cx);
            cx.notify();
            return;
        }
        let original_draft = body;
        let body = original_draft.trim().to_string();
        if self.attachment_uploads_in_progress > 0 {
            self.set_toast(Some("Wait for attachments to finish uploading.".into()), cx);
            cx.notify();
            return;
        }
        if self.editing_message_id.is_some() && !self.pending_attachments.is_empty() {
            self.set_toast(
                Some("Attachments cannot be used while editing a message.".into()),
                cx,
            );
            cx.notify();
            return;
        }
        if body.is_empty() && self.pending_attachments.is_empty() {
            return;
        }
        if let Some(message_id) = self.editing_message_id.clone() {
            let Some(store) = self.local_chat.clone() else {
                self.set_toast(Some("Local message storage is unavailable.".into()), cx);
                cx.notify();
                return;
            };
            match store.edit_message(&message_id, &body) {
                Ok(()) => {
                    self.editing_message_id = None;
                    self.clear_message_draft();
                    self.input_selection = 0..0;
                    self.input_selection_reversed = false;
                    match (store.messages(), store.conversations()) {
                        (Ok(messages), Ok(conversations)) => {
                            self.messages = messages;
                            self.conversations = conversations;
                            self.set_toast(None, cx);
                        }
                        (Err(error), _) | (_, Err(error)) => self.set_toast(Some(error), cx),
                    }
                }
                Err(error) => self.set_toast(Some(error), cx),
            }
            cx.notify();
            return;
        }
        let Some(conversation_id) = self.selected_conversation.clone() else {
            self.set_toast(Some("Select a conversation first.".into()), cx);
            cx.notify();
            return;
        };
        if self
            .pending_attachments
            .iter()
            .any(|pending| pending.conversation_id != conversation_id)
        {
            self.set_toast(
                Some("Remove attachments selected for a different conversation first.".into()),
                cx,
            );
            cx.notify();
            return;
        }
        let attachments = self
            .pending_attachments
            .iter()
            .filter_map(|pending| pending.metadata.clone())
            .collect::<Vec<_>>();
        if attachments.len() != self.pending_attachments.len() {
            self.set_toast(Some("Wait for attachments to finish uploading.".into()), cx);
            cx.notify();
            return;
        }
        let Some(conversation) = self.conversation(&conversation_id).cloned() else {
            self.set_toast(Some("The selected conversation is unavailable.".into()), cx);
            cx.notify();
            return;
        };
        if conversation.is_group()
            && self.current_user.as_ref().map_or(true, |user| {
                !conversation
                    .members
                    .iter()
                    .any(|member| member.id == user.id)
            })
        {
            self.set_toast(Some("You are no longer a member of this group.".into()), cx);
            cx.notify();
            return;
        }
        let peers = if conversation.is_group() {
            let current_user_id = self.current_user.as_ref().map(|user| user.id.as_str());
            conversation
                .members
                .iter()
                .filter(|member| Some(member.id.as_str()) != current_user_id)
                .cloned()
                .collect()
        } else {
            let Some(peer) = self.friend(&conversation.user_id).cloned() else {
                self.set_toast(
                    Some("This account is no longer in your friend list.".into()),
                    cx,
                );
                cx.notify();
                return;
            };
            vec![peer]
        };
        if peers.is_empty() {
            self.set_toast(
                Some("The conversation has no available recipients.".into()),
                cx,
            );
            cx.notify();
            return;
        }
        let Some(store) = self.local_chat.clone() else {
            self.set_toast(Some("Local message storage is unavailable.".into()), cx);
            cx.notify();
            return;
        };
        let message_id = match new_id() {
            Ok(id) => id,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let Some(identity) = self.identity.as_ref() else {
            self.set_toast(Some("Sign in before sending a message.".into()), cx);
            cx.notify();
            return;
        };
        let author_id = identity.user.id.clone();
        let author_name = identity.user.name.clone();
        let time = match timestamp() {
            Ok(time) => time,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let mut message = Message::new(
            &message_id,
            &conversation_id,
            &author_id,
            &author_name,
            &body,
            &time,
            true,
        );
        message.attachments = attachments;
        let (api, token) = match self.api_session() {
            Ok(session) => session,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let peer_transport = self.peer_transport.clone();
        let Some(private_key) = self
            .identity
            .as_ref()
            .map(|identity| Zeroizing::new(identity.private_key().to_vec()))
        else {
            self.set_toast(Some("Sign in before sending a message.".into()), cx);
            cx.notify();
            return;
        };
        let attachment_ids = self
            .pending_attachments
            .iter()
            .map(|pending| pending.id.clone())
            .collect::<Vec<_>>();
        let send_id = message_id.clone();
        self.active_message_send_id = Some(send_id.clone());
        self.set_toast(Some("Encrypting and sending message...".into()), cx);
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    std::thread::spawn(move || {
                        send_message_work(
                            message,
                            conversation,
                            peers,
                            store,
                            api,
                            token,
                            private_key,
                            peer_transport,
                        )
                    })
                    .join()
                    .unwrap_or_else(|_| {
                        Err("The message sending worker stopped unexpectedly.".into())
                    })
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                if state.active_message_send_id.as_deref() != Some(send_id.as_str()) {
                    return;
                }
                state.active_message_send_id = None;
                match result {
                    Ok(completion) => {
                        state.messages = completion.messages;
                        state.conversations = completion.conversations;
                        if state.selected_conversation.as_deref() == Some(conversation_id.as_str())
                        {
                            state.message_scroll_handle.scroll_to_bottom();
                        }
                        if state.message_input.read(cx).text() == original_draft {
                            state.clear_message_draft();
                            state.input_selection = 0..0;
                            state.input_selection_reversed = false;
                        }
                        let current_attachment_ids = state
                            .pending_attachments
                            .iter()
                            .map(|pending| pending.id.clone())
                            .collect::<Vec<_>>();
                        if current_attachment_ids == attachment_ids {
                            state.pending_attachments.clear();
                        }
                        state.start_attachment_upload_status_monitor(completion.attachment_ids, cx);
                        state.set_toast(completion.toast, cx);
                    }
                    Err(error) => state.set_toast(Some(error), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn retry_failed_message(&mut self, message_id: &str, cx: &mut Context<Self>) {
        let Some(store) = self.local_chat.clone() else {
            self.set_toast(Some("Local message storage is unavailable.".into()), cx);
            cx.notify();
            return;
        };
        let message = match store.messages() {
            Ok(messages) => messages
                .into_iter()
                .find(|message| message.id == message_id && message.is_mine),
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let Some(message) = message else {
            return;
        };
        let result = if message.group_deliveries.is_empty() {
            if message.delivery_state != "failed" {
                return;
            }
            store.set_delivery_state(message_id, "not_delivered")
        } else {
            store.retry_group_delivery(message_id)
        };
        match result {
            Ok(()) => match store.messages() {
                Ok(messages) => {
                    self.messages = messages;
                    self.set_toast(Some("Retrying message delivery.".into()), cx);
                }
                Err(error) => self.set_toast(Some(error), cx),
            },
            Err(error) => self.set_toast(Some(error), cx),
        }
        cx.notify();
    }

    pub fn delete_conversation(&mut self, cx: &mut Context<Self>) {
        let Some(conversation_id) = self.selected_conversation.clone() else {
            return;
        };
        self.delete_conversation_by_id(&conversation_id, cx);
    }

    pub fn close_conversation(&mut self, conversation_id: &str, cx: &mut Context<Self>) {
        match self.local_chat.as_ref() {
            Some(store) => match store.close_conversation(conversation_id) {
                Ok(()) => match store.conversations() {
                    Ok(conversations) => {
                        self.conversations = conversations;
                        if self.selected_conversation.as_deref() == Some(conversation_id) {
                            self.selected_conversation = None;
                            self.messages.clear();
                            if self.editing_message_id.take().is_some() {
                                self.clear_message_draft();
                            }
                            self.navigation.navigate(Route::Friends);
                        }
                    }
                    Err(error) => self.set_toast(Some(error), cx),
                },
                Err(error) => self.set_toast(Some(error), cx),
            },
            None => self.set_toast(Some("Local message storage is unavailable.".into()), cx),
        }
        self.active_menu = None;
        cx.notify();
    }

    pub fn delete_conversation_by_id(&mut self, conversation_id: &str, cx: &mut Context<Self>) {
        let mut deleted = false;
        let mut clear_draft = false;
        match self.local_chat.as_ref() {
            Some(store) => match store.delete_conversation(&conversation_id) {
                Ok(()) => {
                    deleted = true;
                    if self.selected_conversation.as_deref() == Some(conversation_id) {
                        self.selected_conversation = None;
                        self.messages.clear();
                        if self.editing_message_id.take().is_some() {
                            clear_draft = true;
                        }
                    }
                    match store.conversations() {
                        Ok(conversations) => {
                            self.conversations = conversations;
                        }
                        Err(error) => self.set_toast(Some(error), cx),
                    }
                }
                Err(error) => self.set_toast(Some(error), cx),
            },
            None => self.set_toast(Some("Local message storage is unavailable.".into()), cx),
        }
        if clear_draft {
            self.clear_message_draft();
        }
        self.active_menu = None;
        if deleted {
            self.navigation.navigate(Route::Friends);
        }
        cx.notify();
    }

    pub fn copy_message(&mut self, message_id: &str, body: &str, cx: &mut Context<Self>) {
        let body = self
            .messages
            .iter()
            .find(|message| message.id == message_id)
            .map(|message| {
                if message.deleted {
                    "You deleted this message".to_string()
                } else {
                    message.body.clone()
                }
            })
            .unwrap_or_else(|| body.to_string());
        cx.write_to_clipboard(ClipboardItem::new_string(body));
        self.active_menu = None;
        cx.notify();
    }

    pub fn delete_message(&mut self, message_id: &str, cx: &mut Context<Self>) {
        let Some(user_id) = self.current_user.as_ref().map(|user| user.id.as_str()) else {
            self.set_toast(Some("Sign in before deleting a message.".into()), cx);
            cx.notify();
            return;
        };
        let Some(message) = self
            .messages
            .iter()
            .find(|message| message.id == message_id)
        else {
            self.set_toast(Some("The message could not be found.".into()), cx);
            cx.notify();
            return;
        };
        if !message.is_mine || message.author_id != user_id {
            self.set_toast(Some("Only your own messages can be deleted.".into()), cx);
            cx.notify();
            return;
        }
        match self.local_chat.as_ref() {
            Some(store) => match store.delete_message(message_id) {
                Ok(()) => match store.messages() {
                    Ok(messages) => {
                        self.messages = messages;
                        self.set_toast(None, cx);
                        if self.editing_message_id.as_deref() == Some(message_id) {
                            self.editing_message_id = None;
                            self.clear_message_draft();
                        }
                    }
                    Err(error) => self.set_toast(Some(error), cx),
                },
                Err(error) => self.set_toast(Some(error), cx),
            },
            None => self.set_toast(Some("Local message storage is unavailable.".into()), cx),
        }
        self.active_menu = None;
        cx.notify();
    }

    pub fn edit_message(&mut self, message_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(user_id) = self.current_user.as_ref().map(|user| user.id.as_str()) else {
            self.set_toast(Some("Sign in before editing a message.".into()), cx);
            cx.notify();
            return;
        };
        let Some(message) = self
            .messages
            .iter()
            .find(|message| message.id == message_id)
        else {
            self.set_toast(Some("The message could not be found.".into()), cx);
            cx.notify();
            return;
        };
        if !message.is_mine || message.author_id != user_id || message.deleted {
            self.set_toast(
                Some("Only your own active messages can be edited.".into()),
                cx,
            );
            cx.notify();
            return;
        }
        self.set_message_editor_value(message.body.clone());
        self.editing_message_id = Some(message_id.to_string());
        self.active_menu = None;
        let focus = self.message_input.read(cx).focus_handle_ref().clone();
        window.focus(&focus, cx);
        self.set_toast(None, cx);
        cx.notify();
    }

    pub fn cancel_message_edit(&mut self, cx: &mut Context<Self>) {
        self.editing_message_id = None;
        self.clear_message_draft();
        cx.notify();
    }

    pub(super) fn set_message_editor_value(&mut self, value: String) {
        self.pending_message_input_value = Some(value);
    }

    pub(super) fn clear_message_draft(&mut self) {
        self.input_selection = 0..0;
        self.input_selection_reversed = false;
        self.pending_message_input_value = Some(String::new());
    }

    pub fn message_body_editor(
        &self,
        message_id: &str,
        body: &str,
        _window: &Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextBox> {
        let mut editors = self.message_body_editors.borrow_mut();
        let text_wrap = TextWrap::Soft;
        let editor_key = format!("{message_id}:wrap");
        if let Some(editor) = editors.get(&editor_key) {
            editor.update(cx, |input, cx| {
                if input.text() != body {
                    input.set_value(body, cx);
                }
            });
            return editor.clone();
        }

        let mut style = ComponentStyle::default();
        style.background = rgb(crate::theme::colors::PANEL_HOVER).into();
        style.background_focused = rgb(crate::theme::colors::PANEL_HOVER).into();
        style.border_width_px = 0.0;
        style.corner_radius = px(0.0);
        style.padding = px(0.0);
        style.vertical_align = VerticalAlign::Top;
        style.line_height = px(20.0);
        style.font_size = px(14.0);
        style.text = rgb(crate::theme::colors::MUTED).into();
        style.caret = rgb(crate::theme::colors::ACCENT).into();
        style.selection = rgb(crate::theme::colors::ACCENT).into();
        style.selected_text = rgb(crate::theme::colors::CANVAS).into();

        let editor = cx.new(|cx| {
            TextBox::multi_line(1, cx)
                .value(body)
                .read_only(true)
                .mode(Mode::MultiLine {
                    min_lines: 1,
                    max_lines: None,
                    wrap: text_wrap,
                })
                .style(style)
        });
        editors.insert(editor_key, editor.clone());
        editor
    }
}
