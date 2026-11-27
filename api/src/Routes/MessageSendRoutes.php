<?php
declare(strict_types=1);

function handle_message_send_routes(PDO $database, string $method, string $path): void
{
    if ($method === 'POST' && $path === '/v1/messages') {
        $account = account_for_token($database);
        device_for_account($database, $account);
        purge_expired_message_data($database);
        $body = request_body();
        $recipientPublicKey = validate_public_key(string_field($body, 'recipient_public_key'));
        $recipientQuery = $database->prepare(
            'SELECT account_id, public_key FROM resonance_accounts WHERE public_key = ?',
        );
        $recipientQuery->execute([$recipientPublicKey]);
        $recipient = $recipientQuery->fetch();
        if (!is_array($recipient)) {
            throw new ApiError(404, 'The message recipient account no longer exists.');
        }
        $groupId = $body['group_id'] ?? null;
        if ($groupId === null) {
            $friendship = $database->prepare(
                'SELECT COUNT(*) FROM resonance_friends
                 WHERE (account_id = ? AND peer_account_id = ?)
                    OR (account_id = ? AND peer_account_id = ?)',
            );
            $friendship->execute([
                $account['account_id'],
                $recipient['account_id'],
                $recipient['account_id'],
                $account['account_id'],
            ]);
            if ((int) $friendship->fetchColumn() !== 2) {
                throw new ApiError(403, 'Messages can only be relayed between mutual friends.');
            }
        } else {
            if (!is_string($groupId) || !preg_match('/\A[a-f0-9]{32}\z/i', $groupId)) {
                throw new ApiError(400, 'The relay group identifier is invalid.');
            }
            $groupId = strtolower($groupId);
            $database->beginTransaction();
            $groupLock = $database->prepare(
                'SELECT group_id FROM resonance_groups WHERE group_id = ? FOR UPDATE',
            );
            $groupLock->execute([strtolower($groupId)]);
            if ($groupLock->fetchColumn() === false) {
                throw new ApiError(404, 'The group does not exist.');
            }
            $membership = $database->prepare(
                'SELECT COUNT(*) FROM resonance_group_members
                 WHERE group_id = ? AND account_id IN (?, ?)',
            );
            $membership->execute([
                strtolower($groupId),
                $account['account_id'],
                $recipient['account_id'],
            ]);
            if ((int) $membership->fetchColumn() !== 2) {
                throw new ApiError(403, 'Messages can only be relayed between group members.');
            }
        }
        $envelope = validate_relay_envelope(
            $body,
            $account['public_key'],
            $recipient['public_key'],
        );
        if (($envelope['group_id'] ?? null) !== ($groupId === null ? null : strtolower($groupId))) {
            throw new ApiError(400, 'The relay group identifier does not match the encrypted envelope.');
        }
        $ownsTransaction = !$database->inTransaction();
        if ($ownsTransaction) {
            $database->beginTransaction();
        }
        try {
            $lockRecipient = $database->prepare(
                'SELECT account_id FROM resonance_accounts WHERE account_id = ? FOR UPDATE',
            );
            $lockRecipient->execute([$recipient['account_id']]);
            if ($lockRecipient->fetchColumn() === false) {
                throw new ApiError(404, 'The message recipient account no longer exists.');
            }
            $expireRecipientDeliveries = $database->prepare(
                'UPDATE resonance_message_device_deliveries d
                 JOIN resonance_relay_messages m
                   ON m.recipient_id = d.recipient_id AND m.message_id = d.message_id
                 SET d.delivery_state = \'expired\',
                     d.lease_token = NULL, d.lease_expires_at = NULL
                 WHERE d.recipient_id = ? AND d.delivery_state = \'pending\'
                   AND m.expires_at <= UTC_TIMESTAMP(6)',
            );
            $expireRecipientDeliveries->execute([$recipient['account_id']]);
            $deleteExpiredRecipientMessages = $database->prepare(
                'DELETE FROM resonance_relay_messages
                 WHERE recipient_id = ? AND expires_at <= UTC_TIMESTAMP(6)',
            );
            $deleteExpiredRecipientMessages->execute([$recipient['account_id']]);

            $existing = $database->prepare(
                'SELECT sender_id FROM resonance_relay_messages
                 WHERE recipient_id = ? AND message_id = ? FOR UPDATE',
            );
            $existing->execute([$recipient['account_id'], $envelope['message_id']]);
            $existingSender = $existing->fetchColumn();
            if ($existingSender !== false && $existingSender !== $account['account_id']) {
                throw new ApiError(409, 'The message identifier is already in use.');
            }
            if ($existingSender === false) {
                $queueSize = $database->prepare(
                    'SELECT COUNT(*) FROM resonance_relay_messages WHERE recipient_id = ?',
                );
                $queueSize->execute([$recipient['account_id']]);
                if ((int) $queueSize->fetchColumn() >= 10000) {
                    throw new ApiError(429, 'The recipient message queue is full. Try again later.');
                }
                $insert = $database->prepare(
                    'INSERT INTO resonance_relay_messages
                     (recipient_id, message_id, sender_id, envelope_json, expires_at)
                     VALUES (?, ?, ?, ?, DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 30 DAY))',
                );
                $insert->execute([
                    $recipient['account_id'],
                    $envelope['message_id'],
                    $account['account_id'],
                    json_encode($envelope, JSON_UNESCAPED_SLASHES | JSON_THROW_ON_ERROR),
                ]);
            }
            $receipt = $database->prepare(
                'INSERT IGNORE INTO resonance_message_receipts
                 (sender_id, recipient_id, message_id, sent_at, group_id,
                  delivery_state, expires_at)
                 VALUES (?, ?, ?, ?, ?, \'pending\', DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 30 DAY))',
            );
            $receipt->execute([
                $account['account_id'],
                $recipient['account_id'],
                $envelope['message_id'],
                (int) $envelope['sent_at'],
                $groupId,
            ]);
            $resetReceipt = $database->prepare(
                'UPDATE resonance_message_receipts
                 SET delivery_state = \'pending\', failure_code = NULL,
                     expires_at = DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 30 DAY)
                 WHERE recipient_id = ? AND message_id = ? AND sender_id = ?
                   AND delivery_state = \'failed\'',
            );
            $resetReceipt->execute([
                $recipient['account_id'],
                $envelope['message_id'],
                $account['account_id'],
            ]);
            $resetDeviceDeliveries = $database->prepare(
                'UPDATE resonance_message_device_deliveries d
                 JOIN resonance_devices active ON active.device_id = d.device_id
                 SET d.delivery_state = \'pending\', d.attempt_count = 0,
                     d.next_attempt_at = NULL, d.failure_code = NULL,
                     d.lease_token = NULL, d.lease_expires_at = NULL
                 WHERE d.recipient_id = ? AND d.message_id = ?
                   AND active.revoked_at IS NULL AND d.delivery_state = \'failed\'',
            );
            $resetDeviceDeliveries->execute([
                $recipient['account_id'],
                $envelope['message_id'],
            ]);
            $deviceDeliveries = $database->prepare(
                'INSERT IGNORE INTO resonance_message_device_deliveries
                 (recipient_id, message_id, device_id)
                 SELECT ?, ?, device_id FROM resonance_devices
                 WHERE account_id = ? AND revoked_at IS NULL',
            );
            $deviceDeliveries->execute([
                $recipient['account_id'],
                $envelope['message_id'],
                $recipient['account_id'],
            ]);
            refresh_account_delivery_state(
                $database,
                $recipient['account_id'],
                $envelope['message_id'],
            );
            if ($ownsTransaction) {
                $database->commit();
            }
        } catch (Throwable $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            throw $error;
        }
        if ($groupId !== null) {
            $database->commit();
        }
        respond_empty(202);
    }

}
