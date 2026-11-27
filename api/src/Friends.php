<?php
declare(strict_types=1);

function friend_rows(PDO $database, string $accountId): array
{
    $statement = $database->prepare(
        'SELECT a.public_key, a.username, a.display_name, a.profile_image,
                CASE WHEN p.updated_at >= DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 20 SECOND)
                    THEN p.address ELSE NULL END AS endpoint_ip,
                CASE WHEN p.updated_at >= DATE_SUB(UTC_TIMESTAMP(6), INTERVAL 20 SECOND)
                    THEN p.port ELSE NULL END AS endpoint_port
         FROM resonance_friends f
         JOIN resonance_accounts a ON a.account_id = f.peer_account_id
         LEFT JOIN resonance_peer_endpoints p ON p.account_id = a.account_id
         WHERE f.account_id = ? ORDER BY a.username',
    );
    $statement->execute([$accountId]);
    $rows = $statement->fetchAll();
    foreach ($rows as &$row) {
        $row['profile_image'] = is_string($row['profile_image'])
            ? base64_encode($row['profile_image'])
            : null;
        if ($row['endpoint_port'] !== null) {
            $row['endpoint_port'] = (int) $row['endpoint_port'];
        }
    }
    unset($row);
    return $rows;
}

function friend_request_rows(PDO $database, string $accountId): array
{
    $statement = $database->prepare(
        'SELECT a.public_key, a.username, a.display_name, a.profile_image,
                NULL AS endpoint_ip, NULL AS endpoint_port
         FROM resonance_friend_requests r
         JOIN resonance_accounts a ON a.account_id = r.sender_id
         WHERE r.recipient_id = ? ORDER BY r.created_at, a.username',
    );
    $statement->execute([$accountId]);
    return friend_rows_with_profile_images($statement->fetchAll());
}

function outgoing_friend_request_rows(PDO $database, string $accountId): array
{
    $statement = $database->prepare(
        'SELECT a.public_key, a.username, a.display_name, a.profile_image,
                NULL AS endpoint_ip, NULL AS endpoint_port
         FROM resonance_friend_requests r
         JOIN resonance_accounts a ON a.account_id = r.recipient_id
         WHERE r.sender_id = ? ORDER BY r.created_at, a.username',
    );
    $statement->execute([$accountId]);
    return friend_rows_with_profile_images($statement->fetchAll());
}

function friend_rows_with_profile_images(array $rows): array
{
    foreach ($rows as &$row) {
        $row['profile_image'] = is_string($row['profile_image'])
            ? base64_encode($row['profile_image'])
            : null;
    }
    unset($row);
    return $rows;
}

