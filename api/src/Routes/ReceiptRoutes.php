<?php
declare(strict_types=1);

function handle_receipt_routes(PDO $database, string $method, string $path): void
{
    if ($method === 'GET' && $path === '/v1/messages/receipts') {
        $account = account_for_token($database);
        device_for_account($database, $account);
        purge_expired_message_data($database);
        $receipts = $database->prepare(
            'SELECT r.message_id, a.public_key AS recipient_public_key,
                    r.delivery_state,
                    CAST(r.delivered_at AS CHAR) AS delivered_at,
                    CAST(r.read_at AS CHAR) AS read_at,
                    COALESCE((
                        SELECT MAX(d.attempt_count)
                        FROM resonance_message_device_deliveries d
                        JOIN resonance_devices active ON active.device_id = d.device_id
                        WHERE d.recipient_id = r.recipient_id
                          AND d.message_id = r.message_id AND active.revoked_at IS NULL
                    ), 0) AS attempts,
                    CAST(UNIX_TIMESTAMP((
                        SELECT MIN(d.next_attempt_at)
                        FROM resonance_message_device_deliveries d
                        JOIN resonance_devices active ON active.device_id = d.device_id
                        WHERE d.recipient_id = r.recipient_id
                          AND d.message_id = r.message_id AND active.revoked_at IS NULL
                          AND d.delivery_state = \'pending\'
                    )) AS CHAR) AS next_retry_at,
                    (
                        SELECT COUNT(*) FROM resonance_message_device_deliveries d
                        JOIN resonance_devices active ON active.device_id = d.device_id
                        WHERE d.recipient_id = r.recipient_id
                          AND d.message_id = r.message_id AND active.revoked_at IS NULL
                    ) AS active_devices,
                    (
                        SELECT COUNT(*) FROM resonance_message_device_deliveries d
                        JOIN resonance_devices active ON active.device_id = d.device_id
                        WHERE d.recipient_id = r.recipient_id
                          AND d.message_id = r.message_id AND active.revoked_at IS NULL
                          AND d.delivery_state = \'pending\'
                    ) AS pending_devices,
                    (
                        SELECT COUNT(*) FROM resonance_message_device_deliveries d
                        JOIN resonance_devices active ON active.device_id = d.device_id
                        WHERE d.recipient_id = r.recipient_id
                          AND d.message_id = r.message_id AND active.revoked_at IS NULL
                          AND d.delivery_state = \'delivered\'
                    ) AS delivered_devices,
                    (
                        SELECT COUNT(*) FROM resonance_message_device_deliveries d
                        JOIN resonance_devices active ON active.device_id = d.device_id
                        WHERE d.recipient_id = r.recipient_id
                          AND d.message_id = r.message_id AND active.revoked_at IS NULL
                          AND d.delivery_state = \'failed\'
                    ) AS failed_devices,
                    (
                        SELECT COUNT(*) FROM resonance_message_device_deliveries d
                        JOIN resonance_devices active ON active.device_id = d.device_id
                        WHERE d.recipient_id = r.recipient_id
                          AND d.message_id = r.message_id AND active.revoked_at IS NULL
                          AND d.delivery_state = \'rejected\'
                    ) AS rejected_devices,
                    (
                        SELECT COUNT(*) FROM resonance_message_device_deliveries d
                        JOIN resonance_devices active ON active.device_id = d.device_id
                        WHERE d.recipient_id = r.recipient_id
                          AND d.message_id = r.message_id AND active.revoked_at IS NULL
                          AND d.delivery_state = \'expired\'
                    ) AS expired_devices,
                    COALESCE(r.failure_code, (
                        SELECT d.failure_code
                        FROM resonance_message_device_deliveries d
                        JOIN resonance_devices active ON active.device_id = d.device_id
                        WHERE d.recipient_id = r.recipient_id
                          AND d.message_id = r.message_id AND active.revoked_at IS NULL
                          AND d.failure_code IS NOT NULL
                        ORDER BY d.updated_at DESC LIMIT 1
                    )) AS failure_code,
                    EXISTS(
                        SELECT 1 FROM resonance_devices active
                        WHERE active.account_id = r.recipient_id AND active.revoked_at IS NULL
                    ) AS device_available
             FROM resonance_message_receipts r
             JOIN resonance_accounts a ON a.account_id = r.recipient_id
             WHERE r.sender_id = ?
             ORDER BY r.sent_at DESC, r.message_id DESC, r.recipient_id
             LIMIT 12000',
        );
        $receipts->execute([$account['account_id']]);
        $receiptRows = $receipts->fetchAll();
        foreach ($receiptRows as &$receiptRow) {
            $receiptRow['device_available'] = (bool) $receiptRow['device_available'];
            foreach ([
                'attempts',
                'active_devices',
                'pending_devices',
                'delivered_devices',
                'failed_devices',
                'rejected_devices',
                'expired_devices',
            ] as $numericField) {
                $receiptRow[$numericField] = (int) $receiptRow[$numericField];
            }
        }
        unset($receiptRow);
        respond(200, $receiptRows);
    }

    if ($method === 'GET' && $path === '/v1/messages/receipts/inbound') {
        $account = account_for_token($database);
        device_for_account($database, $account);
        purge_expired_message_data($database);
        $receipts = $database->prepare(
            'SELECT message_id,
                    CAST(delivered_at AS CHAR) AS delivered_at,
                    CAST(read_at AS CHAR) AS read_at
             FROM resonance_message_receipts
             WHERE recipient_id = ?
             ORDER BY sent_at DESC, message_id DESC LIMIT 1000',
        );
        $receipts->execute([$account['account_id']]);
        respond(200, $receipts->fetchAll());
    }

    if (
        preg_match('/\A\/v1\/messages\/([a-f0-9]{32})\/receipt\z/i', $path, $matches) === 1
        && $method === 'POST'
    ) {
        $account = account_for_token($database);
        $device = device_for_account($database, $account);
        $body = request_body();
        $state = $body['state'] ?? null;
        if (!in_array($state, ['delivered', 'read'], true)) {
            throw new ApiError(400, 'The message receipt state is invalid.');
        }
        if ($state === 'delivered') {
            $delivery = $database->prepare(
                'SELECT delivery_state FROM resonance_message_device_deliveries
                 WHERE recipient_id = ? AND message_id = ? AND device_id = ?',
            );
            $delivery->execute([
                $account['account_id'],
                strtolower($matches[1]),
                $device['device_id'],
            ]);
            if ($delivery->fetchColumn() !== 'delivered') {
                throw new ApiError(409, 'This device must acknowledge delivery before sending a receipt.');
            }
        }
        $receipt = $database->prepare(
            $state === 'read'
                ? 'UPDATE resonance_message_receipts
                   SET read_at = COALESCE(read_at, UNIX_TIMESTAMP())
                   WHERE recipient_id = ? AND message_id = ? AND delivered_at IS NOT NULL'
                : 'UPDATE resonance_message_receipts
                   SET delivered_at = COALESCE(delivered_at, UNIX_TIMESTAMP())
                   WHERE recipient_id = ? AND message_id = ?',
        );
        $receipt->execute([$account['account_id'], strtolower($matches[1])]);
        if ($receipt->rowCount() === 0) {
            $exists = $database->prepare(
                'SELECT delivered_at FROM resonance_message_receipts
                 WHERE recipient_id = ? AND message_id = ?',
            );
            $exists->execute([$account['account_id'], strtolower($matches[1])]);
            $deliveredAt = $exists->fetchColumn();
            if ($deliveredAt === false) {
                throw new ApiError(404, 'The message receipt could not be found.');
            }
            if ($state === 'read' && $deliveredAt === null) {
                throw new ApiError(409, 'The message must be delivered before it can be marked read.');
            }
        }
        respond_empty(204);
    }

}
