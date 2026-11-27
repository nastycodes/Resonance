use super::*;

impl AppState {
    pub fn set_auth_mode(&mut self, mode: AuthMode, cx: &mut Context<Self>) {
        if self.auth_in_progress {
            return;
        }
        self.auth_mode = mode;
        self.set_toast(None, cx);
        self.invite_code_input.clear();
        self.activate_input(InputField::Username);
        cx.notify();
    }

    pub fn activate_input(&mut self, field: InputField) {
        self.active_input = field;
        let cursor = self
            .active_input_text()
            .map(|value| value.graphemes(true).count())
            .unwrap_or(0);
        self.input_selection = cursor..cursor;
        self.input_selection_reversed = false;
    }

    pub fn selection_range(&self) -> Range<usize> {
        let length = self
            .active_input_text()
            .map(|value| value.graphemes(true).count())
            .unwrap_or(0);
        self.input_selection.start.min(length)..self.input_selection.end.min(length)
    }

    pub(super) fn input_cursor(&self) -> usize {
        let selection = self.selection_range();
        if self.input_selection_reversed {
            selection.start
        } else {
            selection.end
        }
    }

    pub(super) fn active_input_text(&self) -> Option<&str> {
        match self.active_input {
            InputField::Username => Some(&self.username_input),
            InputField::DisplayName => Some(&self.display_name_input),
            InputField::InviteCode => Some(&self.invite_code_input),
            InputField::Password => Some(&self.password_input),
            InputField::RememberMe => None,
            InputField::ProfileName => Some(&self.profile_name_input),
            InputField::CurrentPassword => Some(&self.current_password_input),
            InputField::NewPassword => Some(&self.new_password_input),
            InputField::ConfirmNewPassword => Some(&self.confirm_password_input),
            InputField::PeerUsername => Some(&self.peer_username_input),
            InputField::VerificationFingerprint => Some(&self.identity_verification_input),
            InputField::GroupName => Some(&self.group_name_input),
            InputField::DeviceName => Some(&self.device_name_input),
            InputField::DevicePairingCode => Some(&self.device_pairing_verification_input),
        }
    }

    pub(super) fn set_input_cursor(&mut self, offset: usize, extend: bool) {
        let current_selection = self.selection_range();
        let anchor = if self.input_selection_reversed {
            current_selection.end
        } else {
            current_selection.start
        };
        let offset = self
            .active_input_text()
            .map(|value| offset.min(value.graphemes(true).count()))
            .unwrap_or(0);
        if extend {
            if offset < anchor {
                self.input_selection = offset..anchor;
                self.input_selection_reversed = true;
            } else {
                self.input_selection = anchor..offset;
                self.input_selection_reversed = false;
            }
        } else {
            self.input_selection = offset..offset;
            self.input_selection_reversed = false;
        }
    }

    pub(super) fn move_input_cursor(
        &mut self,
        direction: isize,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        if self.active_input == InputField::RememberMe {
            return;
        }
        let selection = self.selection_range();
        let offset = if !extend && !selection.is_empty() {
            if direction < 0 {
                selection.start
            } else {
                selection.end
            }
        } else if direction < 0 {
            self.input_cursor().saturating_sub(1)
        } else {
            self.input_cursor().saturating_add(1)
        };
        self.set_input_cursor(offset, extend);
        cx.notify();
    }

    pub(super) fn replace_input_range(
        &mut self,
        range: Range<usize>,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(input) = self.active_input_mut() else {
            return;
        };
        let start = range.start.min(input.graphemes(true).count());
        let end = range.end.min(input.graphemes(true).count());
        let start_byte = grapheme_offset_to_byte(input, start);
        let end_byte = grapheme_offset_to_byte(input, end);
        input.replace_range(start_byte..end_byte, text);
        let cursor = start + text.graphemes(true).count();
        self.input_selection = cursor..cursor;
        self.input_selection_reversed = false;
        cx.notify();
    }

    pub fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.chars().any(char::is_control) {
            return;
        }
        self.replace_input_range(self.selection_range(), text, cx);
    }

    pub fn backspace(&mut self, cx: &mut Context<Self>) {
        if self.active_input == InputField::RememberMe {
            return;
        }
        let mut range = self.selection_range();
        if range.is_empty() {
            if range.start == 0 {
                return;
            }
            range.start -= 1;
        }
        self.replace_input_range(range, "", cx);
    }

    pub(super) fn delete_forward(&mut self, cx: &mut Context<Self>) {
        if self.active_input == InputField::RememberMe {
            return;
        }
        let mut range = self.selection_range();
        if range.is_empty() {
            let length = self
                .active_input_text()
                .map(|value| value.graphemes(true).count())
                .unwrap_or(0);
            if range.end >= length {
                return;
            }
            range.end += 1;
        }
        self.replace_input_range(range, "", cx);
    }

    pub(super) fn select_all_input(&mut self, cx: &mut Context<Self>) {
        let length = self
            .active_input_text()
            .map(|value| value.graphemes(true).count())
            .unwrap_or(0);
        if self.active_input != InputField::RememberMe {
            self.input_selection = 0..length;
            self.input_selection_reversed = false;
            cx.notify();
        }
    }

    pub(super) fn copy_input_selection(&self, cx: &mut Context<Self>) {
        let range = self.selection_range();
        if range.is_empty() {
            return;
        }
        if let Some(value) = self.active_input_text() {
            let start = grapheme_offset_to_byte(value, range.start);
            let end = grapheme_offset_to_byte(value, range.end);
            cx.write_to_clipboard(ClipboardItem::new_string(value[start..end].to_string()));
        }
    }

    pub(super) fn active_input_mut(&mut self) -> Option<&mut String> {
        match self.active_input {
            InputField::Username => Some(&mut self.username_input),
            InputField::DisplayName => Some(&mut self.display_name_input),
            InputField::InviteCode => Some(&mut self.invite_code_input),
            InputField::Password => Some(&mut self.password_input),
            InputField::RememberMe => None,
            InputField::ProfileName => Some(&mut self.profile_name_input),
            InputField::CurrentPassword => Some(&mut self.current_password_input),
            InputField::NewPassword => Some(&mut self.new_password_input),
            InputField::ConfirmNewPassword => Some(&mut self.confirm_password_input),
            InputField::PeerUsername => Some(&mut self.peer_username_input),
            InputField::VerificationFingerprint => Some(&mut self.identity_verification_input),
            InputField::GroupName => Some(&mut self.group_name_input),
            InputField::DeviceName => Some(&mut self.device_name_input),
            InputField::DevicePairingCode => Some(&mut self.device_pairing_verification_input),
        }
    }

    pub fn paste_text(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.insert_text(&text, cx);
        }
    }

    pub fn handle_input_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        if self.active_input == InputField::DeviceName && self.device_rename_target.is_none() {
            return;
        }
        let key = event.keystroke.key.as_str();
        if key.eq_ignore_ascii_case("tab") && self.current_user.is_none() {
            self.activate_input(next_auth_input(
                self.active_input,
                self.auth_mode,
                event.keystroke.modifiers.shift,
            ));
            cx.notify();
        } else if self.active_input == InputField::RememberMe
            && (key.eq_ignore_ascii_case("enter") || key == "space")
        {
            self.toggle_remember_me(cx);
        } else if key.eq_ignore_ascii_case("a")
            && (event.keystroke.modifiers.control || event.keystroke.modifiers.platform)
        {
            self.select_all_input(cx);
        } else if key.eq_ignore_ascii_case("c")
            && (event.keystroke.modifiers.control || event.keystroke.modifiers.platform)
        {
            self.copy_input_selection(cx);
        } else if key.eq_ignore_ascii_case("x")
            && (event.keystroke.modifiers.control || event.keystroke.modifiers.platform)
        {
            self.copy_input_selection(cx);
            self.replace_input_range(self.selection_range(), "", cx);
        } else if key.eq_ignore_ascii_case("v")
            && (event.keystroke.modifiers.control || event.keystroke.modifiers.platform)
        {
            self.paste_text(cx);
        } else if key.eq_ignore_ascii_case("arrowleft") || key.eq_ignore_ascii_case("left") {
            self.move_input_cursor(-1, event.keystroke.modifiers.shift, cx);
        } else if key.eq_ignore_ascii_case("arrowright") || key.eq_ignore_ascii_case("right") {
            self.move_input_cursor(1, event.keystroke.modifiers.shift, cx);
        } else if key.eq_ignore_ascii_case("home") {
            self.set_input_cursor(0, event.keystroke.modifiers.shift);
            cx.notify();
        } else if key.eq_ignore_ascii_case("end") {
            let end = self
                .active_input_text()
                .map(|value| value.graphemes(true).count())
                .unwrap_or(0);
            self.set_input_cursor(end, event.keystroke.modifiers.shift);
            cx.notify();
        } else if key.eq_ignore_ascii_case("backspace") {
            self.backspace(cx);
        } else if key.eq_ignore_ascii_case("delete") {
            self.delete_forward(cx);
        } else if key.eq_ignore_ascii_case("enter") {
            match self.active_input {
                InputField::PeerUsername => self.add_friend(cx),
                InputField::VerificationFingerprint => self.verify_contact_identity(cx),
                InputField::ProfileName => self.save_profile(cx),
                InputField::CurrentPassword
                | InputField::NewPassword
                | InputField::ConfirmNewPassword => self.change_account_password(cx),
                InputField::GroupName => self.create_group(cx),
                InputField::DeviceName => {
                    self.confirm_device_rename(cx);
                }
                InputField::DevicePairingCode => {}
                InputField::RememberMe => self.toggle_remember_me(cx),
                _ => self.submit_auth(cx),
            }
        } else {
            if let Some(text) = event.keystroke.key_char.as_deref() {
                self.insert_text(text, cx);
            }
        }
    }

    pub fn toggle_remember_me(&mut self, cx: &mut Context<Self>) {
        self.remember_me = !self.remember_me;
        if !self.remember_me {
            cx.spawn(async move |this, cx| {
                let task = cx.update(|cx| cx.delete_credentials(AUTH_CREDENTIAL_URL));
                let result = task.await;
                this.update(cx, |state, cx| {
                    if let Err(error) = result {
                        let error = error.to_string();
                        if !is_missing_credentials_error(&error) {
                            state.set_toast(
                                Some(format!("Could not remove saved login credentials: {error}")),
                                cx,
                            );
                        }
                    }
                    cx.notify();
                })
            })
            .detach();
        }
        cx.notify();
    }

    pub fn submit_auth(&mut self, cx: &mut Context<Self>) {
        if self.auth_in_progress {
            return;
        }
        let Some(api) = self.account_api.clone() else {
            self.set_toast(
                Some(
                    self.startup_error
                        .clone()
                        .unwrap_or_else(|| "The MySQL account API is unavailable.".into()),
                ),
                cx,
            );
            cx.notify();
            return;
        };
        let mode = self.auth_mode;
        let identity_service = self.identity_service;
        let username = self.username_input.trim().to_string();
        let display_name = self.display_name_input.clone();
        let invite_code = self.invite_code_input.clone();
        let password = Zeroizing::new(self.password_input.clone());
        let remember_me = mode == AuthMode::Login && self.remember_me;
        let credential_username = username.clone();
        let credential_password = password.clone();
        self.auth_in_progress = true;
        self.password_input.clear();
        cx.notify();

        let background = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let result = background
                .spawn(async move {
                    match mode {
                        AuthMode::Register => Self::register_account(
                            &identity_service,
                            &api,
                            &username,
                            &display_name,
                            &password,
                            &invite_code,
                        ),
                        AuthMode::Login => {
                            Self::login_account(&identity_service, &api, &username, &password)
                        }
                    }
                })
                .await;

            let _ = this.update(cx, |state, cx| {
                state.auth_in_progress = false;
                match result {
                    Ok((identity, session)) => {
                        if remember_me {
                            let username = credential_username;
                            let password = credential_password;
                            cx.spawn(async move |this, cx| {
                                let task = cx.update(|cx| {
                                    cx.write_credentials(
                                        AUTH_CREDENTIAL_URL,
                                        &username,
                                        password.as_bytes(),
                                    )
                                });
                                let result = task.await;
                                this.update(cx, |state, cx| {
                                    if let Err(error) = result {
                                        state.set_toast(
                                            Some(format!(
                                                "Signed in, but could not save credentials: {error}"
                                            )),
                                            cx,
                                        );
                                    }
                                    cx.notify();
                                })
                            })
                            .detach();
                        }
                        let identity_was_created = session.identity_missing;
                        state.enter_identity(identity, session.access_token, cx);
                        if identity_was_created && state.message.is_none() {
                            state.set_toast(Some(
                                "This account had no stored identity key. A new key was generated and saved to MySQL."
                                    .into(),
                            ), cx);
                        }
                        state.startup_error = None;
                    }
                    Err(error) => state.set_toast(Some(error), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn register_account(
        identity_service: &IdentityService,
        api: &AccountApiClient,
        username: &str,
        display_name: &str,
        password: &str,
        invite_code: &str,
    ) -> Result<(UnlockedIdentity, AccountSession), String> {
        let (identity, record) = identity_service
            .create(username, display_name, password)
            .map_err(|error| error.to_string())?;
        let session = api
            .register(
                &record.username,
                &record.display_name,
                password,
                invite_code,
                &record,
            )
            .map_err(|error| error.message)?;
        if session.account.public_key != record.id || session.identity.as_ref() != Some(&record) {
            let _ = api.logout(&session.access_token);
            return Err(
                "The account API returned a different identity than the one registered.".into(),
            );
        }
        Ok((identity, session))
    }

    fn login_account(
        identity_service: &IdentityService,
        api: &AccountApiClient,
        username: &str,
        password: &str,
    ) -> Result<(UnlockedIdentity, AccountSession), String> {
        let mut session = api
            .login(username, password)
            .map_err(|error| error.message)?;
        let mut identity = if let Some(record) = session.identity.as_ref() {
            match identity_service.unlock(record, password) {
                Ok(identity) => identity,
                Err(error) => {
                    let _ = api.logout(&session.access_token);
                    return Err(error.to_string());
                }
            }
        } else if session.identity_missing {
            let (identity, record) = match identity_service.create(
                &session.account.username,
                &session.account.display_name,
                password,
            ) {
                Ok(identity) => identity,
                Err(error) => {
                    let _ = api.logout(&session.access_token);
                    return Err(error.to_string());
                }
            };
            match api.initialize_identity(&session.access_token, &record) {
                Ok(account) if account.public_key == record.id => session.account = account,
                Ok(_) => {
                    let _ = api.logout(&session.access_token);
                    return Err("The account API returned a different identity.".into());
                }
                Err(error) => {
                    let _ = api.logout(&session.access_token);
                    return Err(error.message);
                }
            }
            identity
        } else {
            let _ = api.logout(&session.access_token);
            return Err(session.identity_error.clone().unwrap_or_else(|| {
                "The encrypted identity stored for this account is corrupted.".into()
            }));
        };
        sync_account_profile(identity_service, &mut identity, &session.account).map_err(
            |error| {
                let _ = api.logout(&session.access_token);
                error
            },
        )?;
        Ok((identity, session))
    }

    pub(super) fn enter_identity(
        &mut self,
        identity: UnlockedIdentity,
        token: String,
        cx: &mut Context<Self>,
    ) {
        let account_id = identity.user.id.clone();
        let update_token = token.clone();
        let identity_for_local = identity.clone();
        let identity_for_device = identity.clone();
        self.device_enrollment_status = None;
        self.device_pairing_code = None;
        self.registered_devices.clear();
        self.pending_device_enrollments.clear();
        self.set_toast(None, cx);
        if let Some(api) = self.account_api.clone() {
            self.account_api = Some(api.without_device_auth());
        }
        self.current_user = Some(identity.user.clone());
        self.profile_name_input = identity.user.name.clone();
        self.identity = Some(identity);
        self.session_token = Some(token);
        self.local_chat = None;
        self.current_device_id = None;
        self.friends.clear();
        self.incoming_identity_verification_requests.clear();
        self.conversations.clear();
        self.messages.clear();
        self.selected_conversation = None;
        self.navigation.navigate(Route::Friends);
        cx.notify();

        let background = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let local_data = background
                .spawn(async move {
                    let store =
                        LocalChatStore::open(&account_id, identity_for_local.private_key())?;
                    let device_id = store.device_identity().map(|device| device.id);
                    let conversations_and_messages = match (store.conversations(), store.messages())
                    {
                        (Ok(conversations), Ok(messages)) => Ok((conversations, messages)),
                        (Err(error), _) | (_, Err(error)) => Err(error),
                    };
                    Ok((store, device_id, conversations_and_messages))
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                if state.session_token.as_deref() != Some(&update_token) {
                    return;
                }
                let (store, device_id, conversations_and_messages) = match local_data {
                    Ok(data) => data,
                    Err(error) => {
                        state.set_toast(Some(error), cx);
                        cx.notify();
                        return;
                    }
                };
                state.local_chat = Some(store.clone());
                match device_id {
                    Ok(device_id) => state.current_device_id = Some(device_id),
                    Err(error) => state.set_toast(Some(error), cx),
                }
                match conversations_and_messages {
                    Ok((conversations, messages)) => {
                        state.conversations = conversations;
                        state.messages = messages;
                    }
                    Err(error) => state.set_toast(Some(error), cx),
                }

                if let Some(api) = state.account_api.clone() {
                    let api = api.without_device_auth();
                    state.account_api = Some(api.clone());
                    let token = update_token.clone();
                    let registration_update_token = token.clone();
                    let identity = identity_for_device.clone();
                    let account_id = identity.user.id.clone();
                    let background = cx.background_executor().clone();
                    cx.spawn(async move |this, cx| {
                        let result = background
                            .spawn(async move {
                                register_identity_device(
                                    api,
                                    &token,
                                    &store,
                                    &account_id,
                                    &identity,
                                )
                            })
                            .await;
                        let _ = this.update(cx, |state, cx| {
                            if state.session_token.as_deref() != Some(&registration_update_token) {
                                return;
                            }
                            let (api, error, enrollment_status, pairing_code) = result;
                            state.account_api = Some(api);
                            state.device_enrollment_status = enrollment_status;
                            state.device_pairing_code = pairing_code;
                            if let Some(error) = error {
                                state.set_toast(Some(error), cx);
                            }
                            if state
                                .account_api
                                .as_ref()
                                .is_some_and(AccountApiClient::has_device_auth)
                            {
                                state.load_remote_data(true, cx);
                                state.start_incoming_message_poll(cx);
                            }
                            cx.notify();
                        });
                    })
                    .detach();
                }
                cx.notify();
            });
        })
        .detach();
    }
}

fn register_identity_device(
    base_api: AccountApiClient,
    token: &str,
    store: &LocalChatStore,
    account_id: &str,
    identity: &UnlockedIdentity,
) -> (
    AccountApiClient,
    Option<String>,
    Option<DeviceEnrollmentStatus>,
    Option<String>,
) {
    let mut device_api = base_api.clone();
    let mut registration_error = None;
    let mut enrollment_status = None;
    let mut pairing_code_value = None;
    match store.device_identity() {
        Ok(device) => {
            match base_api.register_device(token, &device, account_id, identity.private_key()) {
                Ok(registered_api) => device_api = registered_api,
                Err(error)
                    if error.status == Some(403)
                        && error.message
                            == "An existing trusted device must authorize this installation." =>
                {
                    match base_api.begin_device_enrollment(token, &device, account_id) {
                        Ok(status) if status.state == "authorized" => {
                            match base_api.register_device(
                                token,
                                &device,
                                account_id,
                                identity.private_key(),
                            ) {
                                Ok(registered_api) => device_api = registered_api,
                                Err(error) => registration_error = Some(error.message),
                            }
                        }
                        Ok(status) => {
                            if let Ok((signing_key, encryption_key)) =
                                crate::services::device_enrollment::device_public_keys(&device)
                            {
                                pairing_code_value = Some(pairing_code(
                                    account_id,
                                    &status.request_id,
                                    &device.id,
                                    &signing_key,
                                    &encryption_key,
                                    &device.enrollment_nonce,
                                ));
                            }
                            enrollment_status = Some(status);
                            registration_error = Some(
                            "This installation is awaiting approval from a trusted device. Compare the pairing code in Settings > Devices."
                                .into(),
                        );
                        }
                        Err(error) => registration_error = Some(error.message),
                    }
                }
                Err(error) => registration_error = Some(error.message),
            }
        }
        Err(error) => registration_error = Some(error),
    }
    (
        device_api,
        registration_error,
        enrollment_status,
        pairing_code_value,
    )
}
