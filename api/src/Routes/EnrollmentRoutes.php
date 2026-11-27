<?php
declare(strict_types=1);

function handle_enrollment_routes(PDO $database, string $method, string $path): void
{
    if ($method === 'POST' && $path === '/v1/devices/enrollments') {
        $account = account_for_token($database);
        $body = request_body();
        $requestId = strtolower(string_field($body, 'request_id'));
        $nonce = strtolower(string_field($body, 'nonce'));
        $deviceId = strtolower(string_field($body, 'device_id'));
        $deviceSecret = strtolower(string_field($body, 'device_secret'));
        $publicKey = validate_public_key(string_field($body, 'public_key'));
        $signingPublicKey = strtolower(string_field($body, 'signing_public_key'));
        $encryptionPublicKey = strtolower(string_field($body, 'encryption_public_key'));
        $deviceName = validate_device_name(string_field($body, 'device_name'));
        $signature = strtolower(string_field($body, 'signature'));
        if (
            !preg_match('/\A[a-f0-9]{32}\z/', $requestId)
            || !preg_match('/\A[a-f0-9]{64}\z/', $nonce)
            || !preg_match('/\A[a-f0-9]{32}\z/', $deviceId)
            || !preg_match('/\A[a-f0-9]{64}\z/', $deviceSecret)
            || !preg_match('/\A[a-f0-9]{64}\z/', $signingPublicKey)
            || !preg_match('/\A[a-f0-9]{64}\z/', $encryptionPublicKey)
            || !preg_match('/\A[a-f0-9]{128}\z/', $signature)
            || $publicKey !== strtolower($account['public_key'])
        ) {
            throw new ApiError(400, 'The device enrollment proof is invalid.');
        }
        if (!function_exists('sodium_crypto_sign_verify_detached')) {
            throw new ApiError(503, 'The PHP sodium extension is required for device enrollment.');
        }
        $secretHash = hash('sha256', hex2bin($deviceSecret), true);
        $requestTranscript = device_enrollment_request_transcript(
            $publicKey,
            $deviceId,
            bin2hex($secretHash),
            $signingPublicKey,
            $encryptionPublicKey,
            $requestId,
            $nonce,
            $deviceName,
        );
        if (!sodium_crypto_sign_verify_detached(
            hex2bin($signature),
            $requestTranscript,
            hex2bin($signingPublicKey),
        )) {
            throw new ApiError(403, 'The new device did not prove possession of its signing key.');
        }
        $database->beginTransaction();
        try {
            $accountLock = $database->prepare(
                'SELECT account_id FROM resonance_accounts WHERE account_id = ? FOR UPDATE',
            );
            $accountLock->execute([$account['account_id']]);
            if ($accountLock->fetchColumn() === false) {
                throw new ApiError(401, 'The account is no longer available.');
            }
            $deviceCount = $database->prepare(
                'SELECT COUNT(*) FROM resonance_devices
                 WHERE account_id = ? AND revoked_at IS NULL',
            );
            $deviceCount->execute([$account['account_id']]);
            if ((int) $deviceCount->fetchColumn() === 0) {
                throw new ApiError(
                    403,
                    'No active trusted device is available to authorize this installation.',
                );
            }
            $existingDevice = $database->prepare(
                'SELECT device_id FROM resonance_devices WHERE device_id = ? FOR UPDATE',
            );
            $existingDevice->execute([$deviceId]);
            if ($existingDevice->fetchColumn() !== false) {
                throw new ApiError(409, 'This device identifier is already registered.');
            }
            $existingRequest = $database->prepare(
                'SELECT account_id, device_id, device_name, device_secret_hash,
                        signing_public_key, encryption_public_key, nonce, request_signature,
                        CASE WHEN enrollment_state = \'pending\'
                                  AND expires_at <= UTC_TIMESTAMP(6)
                             THEN \'expired\' ELSE enrollment_state END AS enrollment_state,
                        CAST(expires_at AS CHAR) AS expires_at
                 FROM resonance_device_enrollments WHERE request_id = ? FOR UPDATE',
            );
            $existingRequest->execute([$requestId]);
            $existing = $existingRequest->fetch();
            if (is_array($existing)) {
                if (
                    $existing['account_id'] !== $account['account_id']
                    || $existing['device_id'] !== $deviceId
                    || $existing['device_name'] !== $deviceName
                    || !hash_equals($existing['device_secret_hash'], $secretHash)
                    || $existing['signing_public_key'] !== $signingPublicKey
                    || $existing['encryption_public_key'] !== $encryptionPublicKey
                    || $existing['nonce'] !== $nonce
                    || $existing['request_signature'] !== $signature
                ) {
                    throw new ApiError(409, 'The enrollment identifier has already been used.');
                }
                $database->commit();
                respond(200, [
                    'request_id' => $requestId,
                    'state' => $existing['enrollment_state'],
                    'expires_at' => $existing['expires_at'],
                ]);
            }
            $insert = $database->prepare(
                'INSERT INTO resonance_device_enrollments
                 (request_id, account_id, device_id, device_name, device_secret_hash,
                  signing_public_key, encryption_public_key, nonce, request_signature,
                  expires_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 15 MINUTE))',
            );
            $insert->execute([
                $requestId,
                $account['account_id'],
                $deviceId,
                $deviceName,
                $secretHash,
                $signingPublicKey,
                $encryptionPublicKey,
                $nonce,
                $signature,
            ]);
            $expiresAt = $database->prepare(
                'SELECT CAST(expires_at AS CHAR) FROM resonance_device_enrollments
                 WHERE request_id = ?',
            );
            $expiresAt->execute([$requestId]);
            $expiresAtValue = $expiresAt->fetchColumn();
            $database->commit();
        } catch (Throwable $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            throw $error;
        }
        respond(201, [
            'request_id' => $requestId,
            'state' => 'pending',
            'expires_at' => $expiresAtValue,
        ]);
    }

    if (
        preg_match('/\A\/v1\/devices\/enrollments\/([a-f0-9]{32})\z/i', $path, $enrollmentMatches) === 1
        && $method === 'GET'
    ) {
        $account = account_for_token($database);
        $requestId = strtolower($enrollmentMatches[1]);
        $status = $database->prepare(
            'SELECT request_id,
                    CASE WHEN enrollment_state = \'pending\'
                               AND expires_at <= UTC_TIMESTAMP(6)
                         THEN \'expired\' ELSE enrollment_state END AS state,
                    CAST(expires_at AS CHAR) AS expires_at
             FROM resonance_device_enrollments
             WHERE request_id = ? AND account_id = ?',
        );
        $status->execute([$requestId, $account['account_id']]);
        $result = $status->fetch();
        if (!is_array($result)) {
            throw new ApiError(404, 'The device enrollment request was not found.');
        }
        respond(200, $result);
    }

    if ($method === 'GET' && $path === '/v1/devices/enrollments') {
        $account = account_for_token($database);
        $device = device_for_account($database, $account);
        if (!is_string($device['signing_public_key'])) {
            throw new ApiError(409, 'Update this trusted device before authorizing another device.');
        }
        $pending = $database->prepare(
            'SELECT request_id, device_id, device_name,
                    LOWER(HEX(device_secret_hash)) AS device_secret_hash, signing_public_key,
                    encryption_public_key, nonce, request_signature,
                    CAST(created_at AS CHAR) AS created_at,
                    CAST(expires_at AS CHAR) AS expires_at
             FROM resonance_device_enrollments
             WHERE account_id = ? AND enrollment_state = \'pending\'
               AND expires_at > UTC_TIMESTAMP(6)
             ORDER BY created_at, request_id',
        );
        $pending->execute([$account['account_id']]);
        respond(200, $pending->fetchAll());
    }

    if (
        preg_match('/\A\/v1\/devices\/enrollments\/([a-f0-9]{32})\/approve\z/i', $path, $enrollmentMatches) === 1
        && $method === 'POST'
    ) {
        $account = account_for_token($database);
        $approver = device_for_account($database, $account);
        $requestId = strtolower($enrollmentMatches[1]);
        $signature = strtolower(string_field(request_body(), 'signature'));
        if (!preg_match('/\A[a-f0-9]{128}\z/', $signature)) {
            throw new ApiError(400, 'The device authorization signature is invalid.');
        }
        if (
            !is_string($approver['signing_public_key'])
            || !function_exists('sodium_crypto_sign_verify_detached')
        ) {
            throw new ApiError(403, 'This trusted device cannot authorize enrollments.');
        }
        $database->beginTransaction();
        try {
            $accountLock = $database->prepare(
                'SELECT account_id FROM resonance_accounts WHERE account_id = ? FOR UPDATE',
            );
            $accountLock->execute([$account['account_id']]);
            if ($accountLock->fetchColumn() === false) {
                throw new ApiError(401, 'The account is no longer available.');
            }
            $approver = device_for_account($database, $account);
            if (!is_string($approver['signing_public_key'])) {
                throw new ApiError(403, 'This trusted device cannot authorize enrollments.');
            }
            $request = $database->prepare(
                'SELECT device_id, device_name, device_secret_hash, signing_public_key,
                        encryption_public_key, nonce, request_signature, enrollment_state,
                        expires_at <= UTC_TIMESTAMP(6) AS is_expired
                 FROM resonance_device_enrollments
                 WHERE request_id = ? AND account_id = ? FOR UPDATE',
            );
            $request->execute([$requestId, $account['account_id']]);
            $enrollment = $request->fetch();
            if (!is_array($enrollment)) {
                throw new ApiError(404, 'The device enrollment request was not found.');
            }
            $requestTranscript = device_enrollment_request_transcript(
                strtolower($account['public_key']),
                $enrollment['device_id'],
                bin2hex($enrollment['device_secret_hash']),
                $enrollment['signing_public_key'],
                $enrollment['encryption_public_key'],
                $requestId,
                $enrollment['nonce'],
                $enrollment['device_name'],
            );
            if (!sodium_crypto_sign_verify_detached(
                hex2bin($enrollment['request_signature']),
                $requestTranscript,
                hex2bin($enrollment['signing_public_key']),
            )) {
                throw new ApiError(403, 'The target device enrollment proof is invalid.');
            }
            $transcript = device_enrollment_approval_transcript(
                strtolower($account['public_key']),
                $requestId,
                $enrollment['device_id'],
                $enrollment['signing_public_key'],
                $enrollment['encryption_public_key'],
                $enrollment['nonce'],
                $enrollment['request_signature'],
            );
            if (!sodium_crypto_sign_verify_detached(
                hex2bin($signature),
                $transcript,
                hex2bin($approver['signing_public_key']),
            )) {
                throw new ApiError(403, 'The trusted device did not authorize this enrollment.');
            }
            if ($enrollment['enrollment_state'] === 'authorized') {
                $database->commit();
                respond_empty(204);
            }
            if (
                $enrollment['enrollment_state'] !== 'pending'
                || (int) $enrollment['is_expired'] === 1
            ) {
                throw new ApiError(410, 'The device enrollment request has expired or was rejected.');
            }
            $existingDevice = $database->prepare(
                'SELECT device_id FROM resonance_devices WHERE device_id = ? FOR UPDATE',
            );
            $existingDevice->execute([$enrollment['device_id']]);
            if ($existingDevice->fetchColumn() !== false) {
                throw new ApiError(409, 'This device identifier is already registered.');
            }
            $insertDevice = $database->prepare(
                'INSERT INTO resonance_devices
                 (device_id, account_id, identity_public_key, device_secret_hash, device_name,
                  signing_public_key, encryption_public_key)
                 VALUES (?, ?, ?, ?, ?, ?, ?)',
            );
            $insertDevice->execute([
                $enrollment['device_id'],
                $account['account_id'],
                strtolower($account['public_key']),
                $enrollment['device_secret_hash'],
                $enrollment['device_name'],
                $enrollment['signing_public_key'],
                $enrollment['encryption_public_key'],
            ]);
            $authorize = $database->prepare(
                'UPDATE resonance_device_enrollments
                 SET enrollment_state = \'authorized\', approved_by_device_id = ?,
                     approved_at = UTC_TIMESTAMP(6)
                 WHERE request_id = ? AND enrollment_state = \'pending\'',
            );
            $authorize->execute([$approver['device_id'], $requestId]);
            $database->commit();
        } catch (Throwable $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            throw $error;
        }
        respond_empty(204);
    }

}
