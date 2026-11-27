<?php
declare(strict_types=1);

function handle_account_routes(PDO $database, string $method, string $path): void
{
    if ($method === 'POST' && $path === '/v1/accounts') {
        $body = request_body();
        $username = validate_username(string_field($body, 'username'));
        $displayName = validate_display_name(string_field($body, 'display_name'));
        $password = string_field($body, 'password');
        $inviteHash = validate_invite_code(string_field($body, 'invite_code'));
        $publicKey = validate_public_key(string_field($body, 'public_key'));
        $encryptedIdentity = validate_encrypted_identity($body);
        validate_password($password);
        if (!defined('PASSWORD_ARGON2ID')) {
            throw new ApiError(503, 'This PHP installation must support Argon2id password hashing.');
        }
        $passwordHash = password_hash($password, PASSWORD_ARGON2ID);
        if ($passwordHash === false) {
            throw new RuntimeException('Could not hash the account password.');
        }

        $accountId = uuid_v4();
        $database->beginTransaction();
        try {
            $inviteStatement = $database->prepare(
                'INSERT INTO resonance_used_invites (invite_hash) VALUES (?)',
            );
            $inviteStatement->execute([$inviteHash]);
            $statement = $database->prepare(
                'INSERT INTO resonance_accounts
                 (account_id, username, display_name, public_key, password_hash)
                 VALUES (?, ?, ?, ?, ?)',
            );
            $statement->execute([$accountId, $username, $displayName, $publicKey, $passwordHash]);
            store_encrypted_identity($database, $accountId, $encryptedIdentity);
            $token = new_session($database, $accountId);
            $database->commit();
        } catch (PDOException $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            if ($error->getCode() === '23000') {
                $usedInvite = $database->prepare(
                    'SELECT 1 FROM resonance_used_invites WHERE invite_hash = ?',
                );
                $usedInvite->execute([$inviteHash]);
                if ($usedInvite->fetchColumn() !== false) {
                    throw new ApiError(403, 'Invitation code is invalid or has already been used.');
                }
                throw new ApiError(409, 'Username or public identity is already registered.');
            }
            throw $error;
        }
        session_response($database, $token, [
            'account_id' => $accountId,
            'username' => $username,
            'display_name' => $displayName,
            'public_key' => $publicKey,
        ]);
    }

    if ($method === 'POST' && $path === '/v1/sessions') {
        $body = request_body();
        $username = validate_username(string_field($body, 'username'));
        $password = string_field($body, 'password');
        if (strlen($password) > 1024 || str_contains($password, "\0")) {
            throw new ApiError(400, 'Password must not exceed 1024 bytes.');
        }
        $statement = $database->prepare(
            'SELECT account_id, username, display_name, public_key, password_hash, profile_image
             FROM resonance_accounts WHERE username = ?',
        );
        $statement->execute([$username]);
        $account = $statement->fetch();
        if (!is_array($account) || !password_verify($password, $account['password_hash'])) {
            throw new ApiError(401, 'Username or password is incorrect.');
        }
        session_response($database, new_session($database, $account['account_id']), $account);
    }

    if ($method === 'GET' && $path === '/v1/sessions/current') {
        $account = account_for_token($database);
        $profileImage = $database->prepare(
            'SELECT profile_image FROM resonance_accounts WHERE account_id = ?',
        );
        $profileImage->execute([$account['account_id']]);
        $account['profile_image'] = $profileImage->fetchColumn();
        respond(200, account_response($account));
    }

    if ($method === 'DELETE' && $path === '/v1/sessions/current') {
        $statement = $database->prepare('DELETE FROM resonance_account_sessions WHERE token_hash = ?');
        $statement->execute([hash('sha256', bearer_token(), true)]);
        respond_empty(204);
    }

    if ($method === 'PATCH' && $path === '/v1/profile') {
        $account = account_for_token($database);
        $displayName = validate_display_name(string_field(request_body(), 'display_name'));
        $statement = $database->prepare('UPDATE resonance_accounts SET display_name = ? WHERE account_id = ?');
        $statement->execute([$displayName, $account['account_id']]);
        $account['display_name'] = $displayName;
        respond(200, account_response($account));
    }

    if ($method === 'PATCH' && $path === '/v1/profile/image') {
        $account = account_for_token($database);
        $body = request_body(1500000);
        if (!array_key_exists('profile_image', $body)) {
            throw new ApiError(400, 'The profile_image field is required.');
        }
        $profileImage = $body['profile_image'];
        if ($profileImage !== null) {
            if (!is_string($profileImage) || strlen($profileImage) > 1333336) {
                throw new ApiError(400, 'The profile picture is invalid or too large.');
            }
            $decodedImage = base64_decode($profileImage, true);
            if (
                $decodedImage === false
                || strlen($decodedImage) < 1
                || strlen($decodedImage) > 1000000
            ) {
                throw new ApiError(400, 'Profile pictures must be no larger than 1 MB.');
            }
            $isPng = str_starts_with($decodedImage, "\x89PNG\r\n\x1a\n");
            $isJpeg = str_starts_with($decodedImage, "\xff\xd8\xff");
            $isWebp = substr($decodedImage, 0, 4) === 'RIFF'
                && substr($decodedImage, 8, 4) === 'WEBP';
            if (!$isPng && !$isJpeg && !$isWebp) {
                throw new ApiError(400, 'Choose a PNG, JPEG, or WebP profile picture.');
            }
            $profileImage = $decodedImage;
        }
        $statement = $database->prepare(
            'UPDATE resonance_accounts SET profile_image = ? WHERE account_id = ?',
        );
        $statement->execute([$profileImage, $account['account_id']]);
        $account['profile_image'] = $profileImage;
        respond(200, account_response($account));
    }

    if ($method === 'POST' && $path === '/v1/account/password') {
        $account = account_for_token($database);
        $body = request_body();
        $currentPassword = string_field($body, 'current_password');
        $newPassword = string_field($body, 'new_password');
        $publicKey = validate_public_key(string_field($body, 'public_key'));
        $encryptedIdentity = validate_encrypted_identity($body);
        validate_password($newPassword);
        if (!hash_equals($account['public_key'], $publicKey)) {
            throw new ApiError(400, 'The encrypted identity does not match this account.');
        }
        if (!defined('PASSWORD_ARGON2ID')) {
            throw new ApiError(503, 'This PHP installation must support Argon2id password hashing.');
        }

        $passwordHash = password_hash($newPassword, PASSWORD_ARGON2ID);
        if ($passwordHash === false) {
            throw new RuntimeException('Could not hash the account password.');
        }
        $database->beginTransaction();
        try {
            $statement = $database->prepare(
                'SELECT password_hash FROM resonance_accounts
                 WHERE account_id = ? FOR UPDATE',
            );
            $statement->execute([$account['account_id']]);
            $storedPasswordHash = $statement->fetchColumn();
            if (
                !is_string($storedPasswordHash)
                || !password_verify($currentPassword, $storedPasswordHash)
            ) {
                $database->rollBack();
                throw new ApiError(401, 'The current password is incorrect.');
            }
            $update = $database->prepare(
                'UPDATE resonance_accounts SET password_hash = ? WHERE account_id = ?',
            );
            $update->execute([$passwordHash, $account['account_id']]);
            store_encrypted_identity($database, $account['account_id'], $encryptedIdentity);
            $revokeOtherSessions = $database->prepare(
                'DELETE FROM resonance_account_sessions
                 WHERE account_id = ? AND token_hash <> ?',
            );
            $revokeOtherSessions->execute([
                $account['account_id'],
                hash('sha256', bearer_token(), true),
            ]);
            $database->commit();
        } catch (Throwable $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            throw $error;
        }
        respond_empty(204);
    }

    if ($method === 'POST' && $path === '/v1/identity') {
        $account = account_for_token($database);
        $body = request_body();
        $publicKey = validate_public_key(string_field($body, 'public_key'));
        $identity = validate_encrypted_identity($body);

        $database->beginTransaction();
        try {
            $lockAccount = $database->prepare(
                'SELECT account_id FROM resonance_accounts WHERE account_id = ? FOR UPDATE',
            );
            $lockAccount->execute([$account['account_id']]);
            $existingIdentity = $database->prepare(
                'SELECT account_id FROM resonance_encrypted_identities WHERE account_id = ? FOR UPDATE',
            );
            $existingIdentity->execute([$account['account_id']]);
            if ($existingIdentity->fetchColumn() !== false) {
                $database->rollBack();
                throw new ApiError(409, 'This account already has an encrypted identity.');
            }
            $update = $database->prepare(
                'UPDATE resonance_accounts SET public_key = ? WHERE account_id = ?',
            );
            $update->execute([$publicKey, $account['account_id']]);
            store_encrypted_identity($database, $account['account_id'], $identity);
            $database->commit();
        } catch (ApiError $error) {
            throw $error;
        } catch (PDOException $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            if ($error->getCode() === '23000') {
                throw new ApiError(409, 'This identity key is already registered.');
            }
            throw $error;
        }
        $account['public_key'] = $publicKey;
        respond(200, account_response($account));
    }

    if ($method === 'GET' && $path === '/v1/settings') {
        $account = account_for_token($database);
        $statement = $database->prepare(
            'SELECT setting_key, setting_value FROM resonance_settings WHERE account_id = ?',
        );
        $statement->execute([$account['account_id']]);
        $settings = [];
        foreach ($statement->fetchAll() as $row) {
            $settings[$row['setting_key']] = $row['setting_value'];
        }
        respond(200, (object) $settings);
    }

    if (preg_match('/\A\/v1\/settings\/([a-z_]+)\z/', $path, $matches) === 1 && $method === 'PUT') {
        $account = account_for_token($database);
        [$key, $value] = validate_setting($matches[1], string_field(request_body(), 'value'));
        $statement = $database->prepare(
            'INSERT INTO resonance_settings (account_id, setting_key, setting_value)
             VALUES (?, ?, ?) ON DUPLICATE KEY UPDATE setting_value = VALUES(setting_value)',
        );
        $statement->execute([$account['account_id'], $key, $value]);
        respond_empty(204);
    }

    if ($method === 'GET' && $path === '/v1/friends') {
        $account = account_for_token($database);
        respond(200, friend_rows($database, $account['account_id']));
    }

    if ($method === 'GET' && $path === '/v1/friend-requests') {
        $account = account_for_token($database);
        respond(200, friend_request_rows($database, $account['account_id']));
    }

    if ($method === 'GET' && $path === '/v1/friend-requests/outgoing') {
        $account = account_for_token($database);
        respond(200, outgoing_friend_request_rows($database, $account['account_id']));
    }

    if (in_array($method, ['POST', 'PUT'], true) && $path === '/v1/presence') {
        $account = account_for_token($database);
        $body = request_body();
        $port = $body['port'] ?? null;
        if (!is_int($port) || $port < 1 || $port > 65535) {
            throw new ApiError(400, 'A valid direct P2P port is required.');
        }
        $address = $_SERVER['REMOTE_ADDR'] ?? '';
        if (!is_string($address) || filter_var($address, FILTER_VALIDATE_IP) === false) {
            throw new ApiError(400, 'The account API could not determine a valid peer address.');
        }
        $statement = $database->prepare(
            'INSERT INTO resonance_peer_endpoints (account_id, address, port)
             VALUES (?, ?, ?)
             ON DUPLICATE KEY UPDATE address = VALUES(address), port = VALUES(port),
                 updated_at = UTC_TIMESTAMP(6)',
        );
        $statement->execute([$account['account_id'], $address, $port]);
        respond_empty(204);
    }

    if ($method === 'DELETE' && $path === '/v1/presence') {
        $account = account_for_token($database);
        $statement = $database->prepare(
            'DELETE FROM resonance_peer_endpoints WHERE account_id = ?',
        );
        $statement->execute([$account['account_id']]);
        respond_empty(204);
    }

}
