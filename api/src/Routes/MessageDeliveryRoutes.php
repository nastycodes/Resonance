<?php
declare(strict_types=1);

function handle_message_delivery_routes(PDO $database, string $method, string $path): void
{
    if ($method === 'GET' && $path === '/v1/messages') {
        $account = account_for_token($database);
        $device = device_for_account($database, $account);
        purge_expired_message_data($database);
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
        $messages = $database->prepare(
            'SELECT a.public_key AS sender_public_key, m.envelope_json, m.message_id,
                    m.queue_sequence,
                    d.lease_token, d.attempt_count
             FROM resonance_relay_messages m
             JOIN resonance_message_device_deliveries d
               ON d.recipient_id = m.recipient_id AND d.message_id = m.message_id
             JOIN resonance_accounts a ON a.account_id = m.sender_id
             LEFT JOIN resonance_friends f
               ON f.account_id = m.recipient_id AND f.peer_account_id = m.sender_id
             LEFT JOIN resonance_group_members gm
               ON gm.group_id = CONVERT(
                   JSON_UNQUOTE(JSON_EXTRACT(m.envelope_json, \'$.group_id\')) USING ascii
               ) COLLATE ascii_bin
              AND gm.account_id = m.recipient_id
             WHERE m.recipient_id = ?
               AND d.device_id = ?
               AND d.delivery_state = \'pending\'
               AND d.attempt_count < ' . RELAY_DELIVERY_MAX_ATTEMPTS . '
               AND (d.next_attempt_at IS NULL OR d.next_attempt_at <= UTC_TIMESTAMP(6))
               AND (d.lease_expires_at IS NULL OR d.lease_expires_at <= UTC_TIMESTAMP(6))
               AND m.expires_at > UTC_TIMESTAMP(6)
               AND (
                   f.peer_account_id IS NOT NULL
                   OR gm.account_id IS NOT NULL
               )
             ORDER BY m.queue_sequence LIMIT 100 FOR UPDATE',
        );
        $messages->execute([$account['account_id'], $device['device_id']]);
        $claimTemplate =
            'UPDATE resonance_message_device_deliveries
             SET lease_token = ?,
                 lease_expires_at = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL '
                . RELAY_DELIVERY_LEASE_SECONDS . ' SECOND),
                 attempt_count = attempt_count + 1,
                 next_attempt_at = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL %d SECOND)
             WHERE recipient_id = ? AND message_id = ? AND device_id = ?
               AND delivery_state = \'pending\'
               AND attempt_count < ' . RELAY_DELIVERY_MAX_ATTEMPTS . '
               AND (next_attempt_at IS NULL OR next_attempt_at <= UTC_TIMESTAMP(6))
               AND (lease_expires_at IS NULL OR lease_expires_at <= UTC_TIMESTAMP(6))';
        $result = [];
        foreach ($messages->fetchAll() as $message) {
            $leaseToken = bin2hex(random_bytes(16));
            $attemptCount = (int) $message['attempt_count'] + 1;
            $delay = relay_delivery_retry_delay($attemptCount);
            $claim = $database->prepare(sprintf($claimTemplate, $delay));
            $claim->execute([
                $leaseToken,
                $account['account_id'],
                $message['message_id'],
                $device['device_id'],
            ]);
            if ($claim->rowCount() !== 1) {
                continue;
            }
            $envelope = json_decode($message['envelope_json'], true, 16, JSON_THROW_ON_ERROR);
            if (!is_array($envelope)) {
                throw new RuntimeException('Stored relay message is invalid.');
            }
            $result[] = [
                'sender_public_key' => $message['sender_public_key'],
                'envelope' => $envelope,
                'lease_token' => $leaseToken,
                'queue_sequence' => (int) $message['queue_sequence'],
            ];
        }
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
        preg_match('/\A\/v1\/messages\/([a-f0-9]{32})\/ack\z/i', $path, $matches) === 1
        && $method === 'POST'
    ) {
        $account = account_for_token($database);
        $device = device_for_account($database, $account);
        $body = request_body();
        $leaseToken = strtolower(string_field($body, 'lease_token'));
        $state = $body['state'] ?? null;
        if (!preg_match('/\A[a-f0-9]{32}\z/', $leaseToken)
            || !in_array($state, ['delivered', 'rejected'], true)
        ) {
            throw new ApiError(400, 'The device delivery acknowledgement is invalid.');
        }
        $messageId = strtolower($matches[1]);
        $database->beginTransaction();
        try {
            $accountLock = $database->prepare(
                'SELECT account_id FROM resonance_accounts WHERE account_id = ? FOR UPDATE',
            );
            $accountLock->execute([$account['account_id']]);
            $deviceLock = $database->prepare(
                'SELECT device_id FROM resonance_devices
                 WHERE device_id = ? AND account_id = ? AND revoked_at IS NULL FOR UPDATE',
            );
            $deviceLock->execute([$device['device_id'], $account['account_id']]);
            if ($deviceLock->fetchColumn() === false) {
                throw new ApiError(401, 'The registered device identity is invalid or revoked.');
            }
            $delivery = $database->prepare(
                'SELECT d.delivery_state, d.lease_token,
                        d.lease_expires_at > UTC_TIMESTAMP(6) AS lease_active,
                        r.expires_at <= UTC_TIMESTAMP(6) AS message_expired
                 FROM resonance_message_device_deliveries
                 d JOIN resonance_message_receipts r
                   ON r.recipient_id = d.recipient_id AND r.message_id = d.message_id
                 WHERE d.recipient_id = ? AND d.message_id = ? AND d.device_id = ? FOR UPDATE',
            );
            $delivery->execute([$account['account_id'], $messageId, $device['device_id']]);
            $deliveryRow = $delivery->fetch();
            if (!is_array($deliveryRow)) {
                throw new ApiError(404, 'The device delivery was not found.');
            }
            if ($deliveryRow['delivery_state'] !== 'pending') {
                if ($deliveryRow['delivery_state'] !== $state) {
                    if (
                        $deliveryRow['delivery_state'] === 'failed'
                        && (int) $deliveryRow['message_expired'] !== 1
                        && in_array($state, ['delivered', 'rejected'], true)
                    ) {
                        $lateResolution = $database->prepare(
                            'UPDATE resonance_message_device_deliveries
                             SET delivery_state = ?, delivered_at = CASE WHEN ? = \'delivered\'
                                 THEN UNIX_TIMESTAMP() ELSE NULL END,
                                 failure_code = CASE WHEN ? = \'rejected\' THEN \'rejected\' ELSE NULL END
                             WHERE recipient_id = ? AND message_id = ? AND device_id = ?
                               AND delivery_state = \'failed\'',
                        );
                        $lateResolution->execute([
                            $state,
                            $state,
                            $state,
                            $account['account_id'],
                            $messageId,
                            $device['device_id'],
                        ]);
                        if ($state === 'delivered') {
                            $receipt = $database->prepare(
                                'UPDATE resonance_message_receipts
                                 SET delivered_at = COALESCE(delivered_at, UNIX_TIMESTAMP()),
                                     delivery_state = \'delivered\', failure_code = NULL
                                 WHERE recipient_id = ? AND message_id = ?',
                            );
                            $receipt->execute([$account['account_id'], $messageId]);
                        } else {
                            refresh_account_delivery_state(
                                $database,
                                $account['account_id'],
                                $messageId,
                            );
                        }
                        $database->commit();
                        respond_empty(204);
                    }
                    throw new ApiError(409, 'The delivery has already reached another terminal state.');
                }
                $database->commit();
                respond_empty(204);
            }
            if (
                !is_string($deliveryRow['lease_token'])
                || !hash_equals($deliveryRow['lease_token'], $leaseToken)
                || (int) $deliveryRow['lease_active'] !== 1
            ) {
                throw new ApiError(409, 'The device delivery lease is stale.');
            }
            $update = $database->prepare(
                'UPDATE resonance_message_device_deliveries
                 SET delivery_state = ?, delivered_at = CASE WHEN ? = \'delivered\'
                     THEN UNIX_TIMESTAMP() ELSE NULL END,
                     failure_code = CASE WHEN ? = \'rejected\' THEN \'rejected\' ELSE NULL END,
                     next_attempt_at = NULL, lease_token = NULL, lease_expires_at = NULL
                 WHERE recipient_id = ? AND message_id = ? AND device_id = ?',
            );
            $update->execute([
                $state,
                $state,
                $state,
                $account['account_id'],
                $messageId,
                $device['device_id'],
            ]);
            if ($state === 'delivered') {
                $receipt = $database->prepare(
                    'UPDATE resonance_message_receipts
                     SET delivered_at = COALESCE(delivered_at, UNIX_TIMESTAMP()),
                         delivery_state = \'delivered\', failure_code = NULL
                     WHERE recipient_id = ? AND message_id = ?',
                );
                $receipt->execute([$account['account_id'], $messageId]);
            } else {
                refresh_account_delivery_state(
                    $database,
                    $account['account_id'],
                    $messageId,
                );
            }
            $cleanup = $database->prepare(
                'DELETE m FROM resonance_relay_messages m
                 WHERE m.recipient_id = ? AND m.message_id = ?
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
            $cleanup->execute([$account['account_id'], $messageId]);
            $database->commit();
        } catch (Throwable $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            throw $error;
        }
        respond_empty(204);
    }

    if (
        preg_match('/\A\/v1\/messages\/([a-f0-9]{32})\/release\z/i', $path, $matches) === 1
        && $method === 'POST'
    ) {
        $account = account_for_token($database);
        $device = device_for_account($database, $account);
        $body = request_body();
        $leaseToken = strtolower(string_field($body, 'lease_token'));
        $failureCode = $body['failure_code'] ?? 'temporary';
        if (
            !preg_match('/\A[a-f0-9]{32}\z/', $leaseToken)
            || !in_array($failureCode, ['temporary', 'missing_mls_state'], true)
        ) {
            throw new ApiError(400, 'The device delivery lease is invalid.');
        }
        $database->beginTransaction();
        try {
            $accountLock = $database->prepare(
                'SELECT account_id FROM resonance_accounts WHERE account_id = ? FOR UPDATE',
            );
            $accountLock->execute([$account['account_id']]);
            $deviceLock = $database->prepare(
                'SELECT device_id FROM resonance_devices
                 WHERE device_id = ? AND account_id = ? AND revoked_at IS NULL FOR UPDATE',
            );
            $deviceLock->execute([$device['device_id'], $account['account_id']]);
            if ($deviceLock->fetchColumn() === false) {
                throw new ApiError(401, 'The registered device identity is invalid or revoked.');
            }
            $delivery = $database->prepare(
                'SELECT delivery_state, lease_token, attempt_count
                 FROM resonance_message_device_deliveries
                 WHERE recipient_id = ? AND message_id = ? AND device_id = ? FOR UPDATE',
            );
            $delivery->execute([
                $account['account_id'],
                strtolower($matches[1]),
                $device['device_id'],
            ]);
            $deliveryRow = $delivery->fetch();
            if (!is_array($deliveryRow)) {
                throw new ApiError(404, 'The device delivery was not found.');
            }
            if ($deliveryRow['delivery_state'] !== 'pending') {
                if ($deliveryRow['delivery_state'] === 'failed') {
                    $database->commit();
                    respond_empty(204);
                }
                throw new ApiError(409, 'The device delivery is already terminal.');
            }
            if (
                !is_string($deliveryRow['lease_token'])
                || !hash_equals($deliveryRow['lease_token'], $leaseToken)
            ) {
                throw new ApiError(409, 'The device delivery lease is stale.');
            }
            $attemptCount = (int) $deliveryRow['attempt_count'];
            if ($attemptCount >= RELAY_DELIVERY_MAX_ATTEMPTS) {
                $release = $database->prepare(
                    'UPDATE resonance_message_device_deliveries
                     SET delivery_state = \'failed\', failure_code = \'retry_limit\',
                         lease_token = NULL, lease_expires_at = NULL, next_attempt_at = NULL
                     WHERE recipient_id = ? AND message_id = ? AND device_id = ?',
                );
                $release->execute([
                    $account['account_id'],
                    strtolower($matches[1]),
                    $device['device_id'],
                ]);
            } else {
                $delay = relay_delivery_retry_delay($attemptCount);
                $release = $database->prepare(
                    'UPDATE resonance_message_device_deliveries
                     SET lease_token = NULL, lease_expires_at = NULL,
                         next_attempt_at = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL '
                        . $delay . ' SECOND),
                         failure_code = ?
                     WHERE recipient_id = ? AND message_id = ? AND device_id = ?',
                );
                $release->execute([
                    $failureCode,
                    $account['account_id'],
                    strtolower($matches[1]),
                    $device['device_id'],
                ]);
            }
            refresh_account_delivery_state(
                $database,
                $account['account_id'],
                strtolower($matches[1]),
            );
            $database->commit();
        } catch (Throwable $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            throw $error;
        }
        respond_empty(204);
    }

    if (
        preg_match('/\A\/v1\/messages\/([a-f0-9]{32})\z/i', $path, $matches) === 1
        && $method === 'DELETE'
    ) {
        throw new ApiError(405, 'Use the authenticated device delivery acknowledgement route.');
    }

}
