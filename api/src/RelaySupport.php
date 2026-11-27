<?php
declare(strict_types=1);

function purge_expired_message_data(PDO $database): void
{
    $database->exec(
        'UPDATE resonance_message_device_deliveries AS d
         SET delivery_state = \'expired\', lease_token = NULL, lease_expires_at = NULL
         WHERE d.delivery_state = \'pending\'
           AND EXISTS (
               SELECT 1 FROM resonance_relay_messages m
               WHERE m.recipient_id = d.recipient_id AND m.message_id = d.message_id
                 AND m.expires_at <= UTC_TIMESTAMP(6)
           )
         LIMIT 1000',
    );
    $database->exec(
        'UPDATE resonance_message_device_deliveries AS d
         SET delivery_state = \'expired\', failure_code = \'expired\',
             lease_token = NULL, lease_expires_at = NULL, next_attempt_at = NULL
         WHERE d.delivery_state = \'pending\'
           AND EXISTS (
               SELECT 1 FROM resonance_message_receipts r
               WHERE r.recipient_id = d.recipient_id AND r.message_id = d.message_id
                 AND r.expires_at <= UTC_TIMESTAMP(6)
           )
         LIMIT 1000',
    );
    $database->exec(
        'UPDATE resonance_message_device_deliveries
         SET delivery_state = \'failed\', failure_code = \'retry_limit\',
             lease_token = NULL, lease_expires_at = NULL, next_attempt_at = NULL
         WHERE delivery_state = \'pending\' AND attempt_count >= '
            . RELAY_DELIVERY_MAX_ATTEMPTS . '
           AND lease_expires_at IS NOT NULL
           AND lease_expires_at <= UTC_TIMESTAMP(6)
           AND (next_attempt_at IS NULL OR next_attempt_at <= UTC_TIMESTAMP(6))
         LIMIT 1000',
    );
    $database->exec(
        'UPDATE resonance_message_receipts
         SET delivery_state = \'expired\', failure_code = \'expired\'
         WHERE delivery_state = \'pending\' AND expires_at <= UTC_TIMESTAMP(6)
         LIMIT 1000',
    );
    $changedDeliveries = $database->query(
        'SELECT recipient_id, message_id FROM resonance_message_device_deliveries
         WHERE delivery_state = \'failed\' AND failure_code = \'retry_limit\'
           AND updated_at >= DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 1 MINUTE)
         LIMIT 1000',
    )->fetchAll();
    foreach ($changedDeliveries as $delivery) {
        refresh_account_delivery_state(
            $database,
            $delivery['recipient_id'],
            $delivery['message_id'],
        );
    }
    $database->exec(
        'DELETE FROM resonance_relay_messages
         WHERE expires_at <= UTC_TIMESTAMP(6) LIMIT 1000',
    );
    $database->exec(
        'DELETE FROM resonance_message_receipts
         WHERE sent_at <= UNIX_TIMESTAMP(DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 90 DAY))
         LIMIT 1000',
    );
    $database->exec(
        'DELETE FROM resonance_message_device_deliveries
         WHERE created_at <= DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 90 DAY)
         LIMIT 1000',
    );
}

function refresh_account_delivery_state(
    PDO $database,
    string $recipientId,
    string $messageId,
): void {
    $query = $database->prepare(
        'SELECT r.delivered_at, r.failure_code,
                r.expires_at <= UTC_TIMESTAMP(6) AS is_expired,
                COUNT(active.device_id) AS active_devices,
                COALESCE(SUM(active.device_id IS NOT NULL AND d.delivery_state = \'pending\'), 0) AS pending_devices,
                COALESCE(SUM(active.device_id IS NOT NULL AND d.delivery_state = \'failed\'), 0) AS failed_devices,
                COALESCE(SUM(active.device_id IS NOT NULL AND d.delivery_state = \'rejected\'), 0) AS rejected_devices,
                COALESCE(SUM(active.device_id IS NOT NULL AND d.delivery_state = \'expired\'), 0) AS expired_devices
         FROM resonance_message_receipts r
         LEFT JOIN resonance_message_device_deliveries d
           ON d.recipient_id = r.recipient_id AND d.message_id = r.message_id
         LEFT JOIN resonance_devices active
           ON active.device_id = d.device_id
          AND active.account_id = r.recipient_id
          AND active.revoked_at IS NULL
         WHERE r.recipient_id = ? AND r.message_id = ?
         GROUP BY r.recipient_id, r.message_id',
    );
    $query->execute([$recipientId, $messageId]);
    $row = $query->fetch();
    if (!is_array($row)) {
        return;
    }
    if ($row['failure_code'] === 'member_removed') {
        $state = 'rejected';
    } elseif ($row['delivered_at'] !== null) {
        $state = 'delivered';
    } elseif ((int) $row['is_expired'] === 1) {
        $state = 'expired';
    } elseif ((int) $row['active_devices'] === 0 || (int) $row['pending_devices'] > 0) {
        $state = 'pending';
    } elseif (
        (int) $row['rejected_devices'] === (int) $row['active_devices']
    ) {
        $state = 'rejected';
    } elseif ((int) $row['failed_devices'] > 0) {
        $state = 'failed';
    } elseif ((int) $row['expired_devices'] > 0) {
        $state = 'expired';
    } else {
        $state = 'pending';
    }
    $update = $database->prepare(
        'UPDATE resonance_message_receipts SET delivery_state = ?
         WHERE recipient_id = ? AND message_id = ? AND delivered_at IS NULL',
    );
    $update->execute([$state, $recipientId, $messageId]);
}

function relay_delivery_retry_delay(int $attemptCount): int
{
    return min(240, 15 * (2 ** max(0, min(4, $attemptCount - 1))));
}

