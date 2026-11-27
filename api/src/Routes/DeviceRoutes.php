<?php
declare(strict_types=1);

function handle_device_routes(PDO $database, string $method, string $path): void
{
    if (
        preg_match('/\A\/v1\/devices\/([a-f0-9]{32})\/rename\z/i', $path, $deviceMatches) === 1
        && $method === 'POST'
    ) {
        $account = account_for_token($database);
        device_for_account($database, $account);
        $deviceId = strtolower($deviceMatches[1]);
        $name = validate_device_name(string_field(request_body(), 'name'));
        $database->beginTransaction();
        try {
            $accountLock = $database->prepare(
                'SELECT account_id FROM resonance_accounts WHERE account_id = ? FOR UPDATE',
            );
            $accountLock->execute([$account['account_id']]);
            if ($accountLock->fetchColumn() === false) {
                throw new ApiError(401, 'The account is no longer available.');
            }
            device_for_account($database, $account);
            $target = $database->prepare(
                'SELECT device_id FROM resonance_devices
                 WHERE device_id = ? AND account_id = ? AND revoked_at IS NULL FOR UPDATE',
            );
            $target->execute([$deviceId, $account['account_id']]);
            if ($target->fetchColumn() === false) {
                throw new ApiError(404, 'The active device was not found.');
            }
            $rename = $database->prepare(
                'UPDATE resonance_devices SET device_name = ?
                 WHERE device_id = ? AND account_id = ? AND revoked_at IS NULL',
            );
            $rename->execute([$name, $deviceId, $account['account_id']]);
            $database->commit();
        } catch (Throwable $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            throw $error;
        }
        respond_empty(204);
    }

    if ($method === 'GET' && $path === '/v1/devices') {
        $account = account_for_token($database);
        device_for_account($database, $account);
        $devices = $database->prepare(
            'SELECT device_id, device_name, signing_public_key, encryption_public_key,
                    CAST(created_at AS CHAR) AS created_at,
                    CAST(last_seen_at AS CHAR) AS last_seen_at,
                    CAST(revoked_at AS CHAR) AS revoked_at
             FROM resonance_devices WHERE account_id = ?
             ORDER BY created_at, device_id',
        );
        $devices->execute([$account['account_id']]);
        respond(200, $devices->fetchAll());
    }

    if ($method === 'POST' && $path === '/v1/devices/register') {
        $account = account_for_token($database);
        $body = request_body();
        $deviceId = strtolower(string_field($body, 'device_id'));
        $deviceSecret = strtolower(string_field($body, 'device_secret'));
        $publicKey = validate_public_key(string_field($body, 'public_key'));
        $signature = strtolower(string_field($body, 'signature'));
        $hasDeviceKeys = isset(
            $body['signing_public_key'],
            $body['encryption_public_key'],
            $body['device_name'],
        );
        $signingPublicKey = null;
        $encryptionPublicKey = null;
        $deviceName = 'Resonance desktop';
        if ($hasDeviceKeys) {
            $signingPublicKey = strtolower(string_field($body, 'signing_public_key'));
            $encryptionPublicKey = strtolower(string_field($body, 'encryption_public_key'));
            $deviceName = validate_device_name(string_field($body, 'device_name'));
        }
        if (
            !preg_match('/\A[a-f0-9]{32}\z/', $deviceId)
            || !preg_match('/\A[a-f0-9]{64}\z/', $deviceSecret)
            || !preg_match('/\A[a-f0-9]{128}\z/', $signature)
            || ($hasDeviceKeys && (
                !is_string($signingPublicKey)
                || !preg_match('/\A[a-f0-9]{64}\z/', $signingPublicKey)
                || !is_string($encryptionPublicKey)
                || !preg_match('/\A[a-f0-9]{64}\z/', $encryptionPublicKey)
            ))
            || $publicKey !== strtolower($account['public_key'])
        ) {
            throw new ApiError(400, 'The device registration identity is invalid.');
        }
        if (!function_exists('sodium_crypto_sign_verify_detached')) {
            throw new ApiError(503, 'The PHP sodium extension is required for device registration.');
        }
        $signed = $hasDeviceKeys
            ? "Resonance device registration v2\0"
                . implode("\0", [
                    $publicKey,
                    $deviceId,
                    $deviceSecret,
                    $signingPublicKey,
                    $encryptionPublicKey,
                    $deviceName,
                ])
            : "Resonance device registration v1\0"
                . $publicKey . "\0" . $deviceId . "\0" . $deviceSecret;
        if (!sodium_crypto_sign_verify_detached(
            hex2bin($signature),
            $signed,
            hex2bin($publicKey),
        )) {
            throw new ApiError(403, 'The account identity did not authenticate this device.');
        }

        $secretHash = hash('sha256', hex2bin($deviceSecret), true);
        $database->beginTransaction();
        try {
            $accountLock = $database->prepare(
                'SELECT account_id FROM resonance_accounts WHERE account_id = ? FOR UPDATE',
            );
            $accountLock->execute([$account['account_id']]);
            if ($accountLock->fetchColumn() === false) {
                throw new ApiError(401, 'The account is no longer available.');
            }
            $existing = $database->prepare(
                'SELECT account_id, identity_public_key, device_secret_hash, revoked_at
                 FROM resonance_devices WHERE device_id = ? FOR UPDATE',
            );
            $existing->execute([$deviceId]);
            $existingDevice = $existing->fetch();
            if (is_array($existingDevice)) {
                if (
                    $existingDevice['account_id'] !== $account['account_id']
                    || $existingDevice['identity_public_key'] !== $publicKey
                    || !hash_equals($existingDevice['device_secret_hash'], $secretHash)
                    || $existingDevice['revoked_at'] !== null
                ) {
                    throw new ApiError(409, 'This device identifier is already registered or revoked.');
                }
                if ($hasDeviceKeys) {
                    $touch = $database->prepare(
                        'UPDATE resonance_devices
                         SET last_seen_at = UTC_TIMESTAMP(6),
                             signing_public_key = ?, encryption_public_key = ?
                         WHERE device_id = ?',
                    );
                    $touch->execute([$signingPublicKey, $encryptionPublicKey, $deviceId]);
                } else {
                    $touch = $database->prepare(
                        'UPDATE resonance_devices SET last_seen_at = UTC_TIMESTAMP(6)
                         WHERE device_id = ?',
                    );
                    $touch->execute([$deviceId]);
                }
            } else {
                $registeredCount = $database->prepare(
                    'SELECT COUNT(*) FROM resonance_devices WHERE account_id = ?',
                );
                $registeredCount->execute([$account['account_id']]);
                if ((int) $registeredCount->fetchColumn() !== 0) {
                    throw new ApiError(
                        403,
                        'An existing trusted device must authorize this installation.',
                    );
                }
                if ($hasDeviceKeys) {
                    $insertDevice = $database->prepare(
                        'INSERT INTO resonance_devices
                         (device_id, account_id, identity_public_key, device_secret_hash,
                          device_name, signing_public_key, encryption_public_key)
                         VALUES (?, ?, ?, ?, ?, ?, ?)',
                    );
                    $insertDevice->execute([
                        $deviceId,
                        $account['account_id'],
                        $publicKey,
                        $secretHash,
                        $deviceName,
                        $signingPublicKey,
                        $encryptionPublicKey,
                    ]);
                } else {
                    $insertDevice = $database->prepare(
                        'INSERT INTO resonance_devices
                         (device_id, account_id, identity_public_key, device_secret_hash)
                         VALUES (?, ?, ?, ?)',
                    );
                    $insertDevice->execute([
                        $deviceId,
                        $account['account_id'],
                        $publicKey,
                        $secretHash,
                    ]);
                }
            }
            $restoreDeliveries = $database->prepare(
                'INSERT IGNORE INTO resonance_message_device_deliveries
                 (recipient_id, message_id, device_id)
                 SELECT recipient_id, message_id, ?
                 FROM resonance_relay_messages
                 WHERE recipient_id = ? AND expires_at > UTC_TIMESTAMP(6)',
            );
            $restoreDeliveries->execute([$deviceId, $account['account_id']]);
            $restoredReceipts = $database->prepare(
                'SELECT message_id FROM resonance_message_receipts
                 WHERE recipient_id = ? AND expires_at > UTC_TIMESTAMP(6)',
            );
            $restoredReceipts->execute([$account['account_id']]);
            foreach ($restoredReceipts->fetchAll() as $receipt) {
                refresh_account_delivery_state(
                    $database,
                    $account['account_id'],
                    $receipt['message_id'],
                );
            }
            $database->commit();
        } catch (Throwable $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            throw $error;
        }
        respond(200, ['device_id' => $deviceId]);
    }

    if (
        preg_match('/\A\/v1\/devices\/([a-f0-9]{32})\/revoke\z/i', $path, $deviceMatches) === 1
        && $method === 'POST'
    ) {
        $account = account_for_token($database);
        $requestingDevice = device_for_account($database, $account);
        $deviceId = strtolower($deviceMatches[1]);
        if ($deviceId === $requestingDevice['device_id']) {
            throw new ApiError(409, 'Use another trusted device to revoke this installation.');
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
            $requestingDevice = device_for_account($database, $account);
            if ($deviceId === $requestingDevice['device_id']) {
                throw new ApiError(409, 'Use another trusted device to revoke this installation.');
            }
            $target = $database->prepare(
                'SELECT device_id FROM resonance_devices
                 WHERE device_id = ? AND account_id = ? AND revoked_at IS NULL FOR UPDATE',
            );
            $target->execute([$deviceId, $account['account_id']]);
            if ($target->fetchColumn() === false) {
                $revokedTarget = $database->prepare(
                    'SELECT device_id FROM resonance_devices
                     WHERE device_id = ? AND account_id = ? AND revoked_at IS NOT NULL FOR UPDATE',
                );
                $revokedTarget->execute([$deviceId, $account['account_id']]);
                if ($revokedTarget->fetchColumn() !== false) {
                    $database->commit();
                    respond_empty(204);
                }
                throw new ApiError(404, 'The active device was not found.');
            }
            $activeCount = $database->prepare(
                'SELECT COUNT(*) FROM resonance_devices
                 WHERE account_id = ? AND revoked_at IS NULL',
            );
            $activeCount->execute([$account['account_id']]);
            if ((int) $activeCount->fetchColumn() <= 1) {
                throw new ApiError(
                    409,
                    'Keep at least one active trusted device to authorize future installations.',
                );
            }
            $revoke = $database->prepare(
                'UPDATE resonance_devices SET revoked_at = UTC_TIMESTAMP(6)
                 WHERE device_id = ? AND account_id = ? AND revoked_at IS NULL',
            );
            $revoke->execute([$deviceId, $account['account_id']]);
            $deleteHistory = $database->prepare(
                'DELETE FROM resonance_device_history_syncs
                 WHERE source_device_id = ? OR target_device_id = ?',
            );
            $deleteHistory->execute([$deviceId, $deviceId]);
            $cancelDeliveries = $database->prepare(
                'UPDATE resonance_message_device_deliveries
                 SET delivery_state = \'revoked\', lease_token = NULL, lease_expires_at = NULL
                 WHERE device_id = ? AND delivery_state = \'pending\'',
            );
            $cancelDeliveries->execute([$deviceId]);
            $cleanup = $database->prepare(
                'DELETE m FROM resonance_relay_messages m
                 WHERE m.recipient_id = ?
                   AND EXISTS (
                       SELECT 1 FROM resonance_devices active
                       WHERE active.account_id = m.recipient_id AND active.revoked_at IS NULL
                   )
                   AND NOT EXISTS (
                       SELECT 1
                       FROM resonance_message_device_deliveries d
                       JOIN resonance_devices active ON active.device_id = d.device_id
                       WHERE d.recipient_id = m.recipient_id AND d.message_id = m.message_id
                         AND active.revoked_at IS NULL AND d.delivery_state = \'pending\'
                   )',
            );
            $cleanup->execute([$account['account_id']]);
            $revokedDeliveryIds = $database->prepare(
                'SELECT message_id FROM resonance_message_device_deliveries
                 WHERE recipient_id = ? AND device_id = ? AND delivery_state = \'revoked\'',
            );
            $revokedDeliveryIds->execute([$account['account_id'], $deviceId]);
            foreach ($revokedDeliveryIds->fetchAll() as $delivery) {
                refresh_account_delivery_state(
                    $database,
                    $account['account_id'],
                    $delivery['message_id'],
                );
            }
            $database->commit();
        } catch (Throwable $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            throw $error;
        }
        respond_empty(204);
    }

    if ($method === 'POST' && $path === '/v1/devices/history') {
        $account = account_for_token($database);
        $source = device_for_account($database, $account);
        $body = request_body(7 * 1024 * 1024);
        $syncId = strtolower(string_field($body, 'sync_id'));
        $sourceDeviceId = strtolower(string_field($body, 'source_device_id'));
        $targetDeviceId = strtolower(string_field($body, 'target_device_id'));
        $ephemeralKey = strtolower(string_field($body, 'ephemeral_public_key'));
        $nonce = strtolower(string_field($body, 'nonce'));
        $ciphertext = strtolower(string_field($body, 'ciphertext'));
        $signature = strtolower(string_field($body, 'signature'));
        if (
            !preg_match('/\A[a-f0-9]{32}\z/', $syncId)
            || !preg_match('/\A[a-f0-9]{32}\z/', $sourceDeviceId)
            || !preg_match('/\A[a-f0-9]{32}\z/', $targetDeviceId)
            || !preg_match('/\A[a-f0-9]{64}\z/', $ephemeralKey)
            || !preg_match('/\A[a-f0-9]{24}\z/', $nonce)
            || strlen($ciphertext) < 32
            || strlen($ciphertext) > 6291488
            || !preg_match('/\A(?:[a-f0-9]{2})+\z/', $ciphertext)
            || !preg_match('/\A[a-f0-9]{128}\z/', $signature)
            || $sourceDeviceId !== $source['device_id']
            || $sourceDeviceId === $targetDeviceId
            || !is_string($source['signing_public_key'])
        ) {
            throw new ApiError(400, 'The encrypted history package is invalid.');
        }
        $targetQuery = $database->prepare(
            'SELECT device_id FROM resonance_devices
             WHERE device_id = ? AND account_id = ? AND revoked_at IS NULL
               AND encryption_public_key IS NOT NULL',
        );
        $targetQuery->execute([$targetDeviceId, $account['account_id']]);
        if ($targetQuery->fetchColumn() === false) {
            throw new ApiError(404, 'The active target device was not found.');
        }
        $signatureData = "Resonance device history package v1\0"
            . implode("\0", [
                strtolower($account['public_key']),
                $syncId,
                $sourceDeviceId,
                $targetDeviceId,
                $ephemeralKey,
                $nonce,
            ])
            . "\0" . $ciphertext;
        if (
            !function_exists('sodium_crypto_sign_verify_detached')
            || !sodium_crypto_sign_verify_detached(
                hex2bin($signature),
                $signatureData,
                hex2bin($source['signing_public_key']),
            )
        ) {
            throw new ApiError(403, 'The history package signature is invalid.');
        }
        $ciphertextBytes = hex2bin($ciphertext);
        if (!is_string($ciphertextBytes)) {
            throw new ApiError(400, 'The encrypted history package is invalid.');
        }
        $database->beginTransaction();
        try {
            $sourceLock = $database->prepare(
                'SELECT device_id, signing_public_key FROM resonance_devices
                 WHERE device_id = ? AND account_id = ? AND revoked_at IS NULL FOR UPDATE',
            );
            $sourceLock->execute([$sourceDeviceId, $account['account_id']]);
            $lockedSource = $sourceLock->fetch();
            if (!is_array($lockedSource)) {
                throw new ApiError(401, 'The source device has been revoked.');
            }
            if (
                !is_string($lockedSource['signing_public_key'])
                || !hash_equals($source['signing_public_key'], $lockedSource['signing_public_key'])
            ) {
                throw new ApiError(409, 'The source device signing key changed during this request.');
            }
            $targetLock = $database->prepare(
                'SELECT device_id FROM resonance_devices
                 WHERE device_id = ? AND account_id = ? AND revoked_at IS NULL FOR UPDATE',
            );
            $targetLock->execute([$targetDeviceId, $account['account_id']]);
            if ($targetLock->fetchColumn() === false) {
                throw new ApiError(409, 'The target device has been revoked.');
            }
            $existingSync = $database->prepare(
                'SELECT account_id, source_device_id, target_device_id, ephemeral_public_key,
                        nonce, ciphertext, signature
                 FROM resonance_device_history_syncs WHERE sync_id = ? FOR UPDATE',
            );
            $existingSync->execute([$syncId]);
            $existing = $existingSync->fetch();
            if (is_array($existing)) {
                if (
                    $existing['account_id'] !== $account['account_id']
                    || $existing['source_device_id'] !== $sourceDeviceId
                    || $existing['target_device_id'] !== $targetDeviceId
                    || $existing['ephemeral_public_key'] !== $ephemeralKey
                    || $existing['nonce'] !== $nonce
                    || !hash_equals($existing['ciphertext'], $ciphertextBytes)
                    || $existing['signature'] !== $signature
                ) {
                    throw new ApiError(409, 'The history sync identifier has already been used.');
                }
                $database->commit();
                respond_empty(204);
            }
            $insert = $database->prepare(
                'INSERT INTO resonance_device_history_syncs
                 (sync_id, account_id, source_device_id, target_device_id,
                  ephemeral_public_key, nonce, ciphertext, signature,
                  expires_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 7 DAY))',
            );
            $insert->execute([
                $syncId,
                $account['account_id'],
                $sourceDeviceId,
                $targetDeviceId,
                $ephemeralKey,
                $nonce,
                $ciphertextBytes,
                $signature,
            ]);
            $database->commit();
        } catch (Throwable $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            throw $error;
        }
        respond_empty(201);
    }

    if ($method === 'GET' && $path === '/v1/devices/history') {
        $account = account_for_token($database);
        $device = device_for_account($database, $account);
        $database->beginTransaction();
        try {
            $deviceLock = $database->prepare(
                'SELECT device_id FROM resonance_devices
                 WHERE device_id = ? AND account_id = ? AND revoked_at IS NULL FOR UPDATE',
            );
            $deviceLock->execute([$device['device_id'], $account['account_id']]);
            if ($deviceLock->fetchColumn() === false) {
                throw new ApiError(401, 'The registered device identity is invalid or revoked.');
            }
            $purge = $database->prepare(
                'DELETE FROM resonance_device_history_syncs WHERE expires_at <= UTC_TIMESTAMP(6)',
            );
            $purge->execute();
            $history = $database->prepare(
                'SELECT sync_id, source_device_id, target_device_id, ephemeral_public_key, nonce,
                        LOWER(HEX(ciphertext)) AS ciphertext, signature,
                        CAST(created_at AS CHAR) AS created_at
                 FROM resonance_device_history_syncs
                 WHERE account_id = ? AND target_device_id = ?
                   AND expires_at > UTC_TIMESTAMP(6)
                 ORDER BY created_at, sync_id',
            );
            $history->execute([$account['account_id'], $device['device_id']]);
            $result = $history->fetchAll();
            $database->commit();
        } catch (Throwable $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            throw $error;
        }
        respond(200, $result);
    }

    if (
        preg_match('/\A\/v1\/devices\/history\/([a-f0-9]{32})\/ack\z/i', $path, $historyMatches) === 1
        && $method === 'POST'
    ) {
        $account = account_for_token($database);
        $device = device_for_account($database, $account);
        $syncId = strtolower($historyMatches[1]);
        $database->beginTransaction();
        try {
            $deviceLock = $database->prepare(
                'SELECT device_id FROM resonance_devices
                 WHERE device_id = ? AND account_id = ? AND revoked_at IS NULL FOR UPDATE',
            );
            $deviceLock->execute([$device['device_id'], $account['account_id']]);
            if ($deviceLock->fetchColumn() === false) {
                throw new ApiError(401, 'The registered device identity is invalid or revoked.');
            }
            $delete = $database->prepare(
                'DELETE FROM resonance_device_history_syncs
                 WHERE sync_id = ? AND account_id = ? AND target_device_id = ?',
            );
            $delete->execute([$syncId, $account['account_id'], $device['device_id']]);
            if ($delete->rowCount() === 0) {
                $exists = $database->prepare(
                    'SELECT target_device_id FROM resonance_device_history_syncs
                     WHERE sync_id = ? AND account_id = ?',
                );
                $exists->execute([$syncId, $account['account_id']]);
                $targetDeviceId = $exists->fetchColumn();
                if ($targetDeviceId !== false && $targetDeviceId !== $device['device_id']) {
                    throw new ApiError(404, 'The history package was not found for this device.');
                }
            }
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
