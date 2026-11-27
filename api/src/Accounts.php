<?php
declare(strict_types=1);

function account_response(array $row): array
{
    return [
        'username' => $row['username'],
        'display_name' => $row['display_name'],
        'public_key' => $row['public_key'],
        'profile_image' => isset($row['profile_image']) && is_string($row['profile_image'])
            ? base64_encode($row['profile_image'])
            : null,
    ];
}

function encrypted_identity_for_account(PDO $database, string $accountId): ?array
{
    $statement = $database->prepare(
        'SELECT salt, nonce, encrypted_private_key
         FROM resonance_encrypted_identities WHERE account_id = ?',
    );
    $statement->execute([$accountId]);
    $identity = $statement->fetch();
    return is_array($identity) ? $identity : null;
}

function store_encrypted_identity(PDO $database, string $accountId, array $identity): void
{
    $statement = $database->prepare(
        'INSERT INTO resonance_encrypted_identities (account_id, salt, nonce, encrypted_private_key)
         VALUES (?, ?, ?, ?)
         ON DUPLICATE KEY UPDATE salt = VALUES(salt), nonce = VALUES(nonce),
             encrypted_private_key = VALUES(encrypted_private_key)',
    );
    $statement->execute([
        $accountId,
        $identity['salt'],
        $identity['nonce'],
        $identity['encrypted_private_key'],
    ]);
}

function new_session(PDO $database, string $accountId): string
{
    $database->exec('DELETE FROM resonance_account_sessions WHERE expires_at <= UTC_TIMESTAMP(6)');
    $token = bin2hex(random_bytes(32));
    $statement = $database->prepare(
        'INSERT INTO resonance_account_sessions (token_hash, account_id, expires_at)
         VALUES (?, ?, DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 12 HOUR))',
    );
    $statement->execute([hash('sha256', $token, true), $accountId]);
    return $token;
}

function session_response(PDO $database, string $token, array $account): never
{
    respond(200, [
        'access_token' => $token,
        'account' => account_response($account),
        'encrypted_identity' => encrypted_identity_for_account($database, $account['account_id']),
    ]);
}

function bearer_token(): string
{
    $sessionToken = $_SERVER['HTTP_X_RESONANCE_SESSION'] ?? '';
    if (preg_match('/\A[a-f0-9]{64}\z/i', $sessionToken)) {
        return $sessionToken;
    }

    $authorization = $_SERVER['HTTP_AUTHORIZATION'] ?? $_SERVER['REDIRECT_HTTP_AUTHORIZATION'] ?? '';
    if (!preg_match('/\ABearer ([a-f0-9]{64})\z/i', $authorization, $matches)) {
        throw new ApiError(401, 'A valid session is required.');
    }
    return $matches[1];
}

function account_for_token(PDO $database): array
{
    $statement = $database->prepare(
        'SELECT a.account_id, a.username, a.display_name, a.public_key
         FROM resonance_account_sessions s
         JOIN resonance_accounts a ON a.account_id = s.account_id
         WHERE s.token_hash = ? AND s.expires_at > UTC_TIMESTAMP(6)',
    );
    $statement->execute([hash('sha256', bearer_token(), true)]);
    $account = $statement->fetch();
    if (!is_array($account)) {
        throw new ApiError(401, 'Session is invalid or expired.');
    }
    return $account;
}

function device_for_account(PDO $database, array $account): array
{
    $deviceId = $_SERVER['HTTP_X_RESONANCE_DEVICE_ID'] ?? '';
    $deviceSecret = $_SERVER['HTTP_X_RESONANCE_DEVICE_SECRET'] ?? '';
    if (
        !is_string($deviceId)
        || !preg_match('/\A[a-f0-9]{32}\z/i', $deviceId)
        || !is_string($deviceSecret)
        || !preg_match('/\A[a-f0-9]{64}\z/i', $deviceSecret)
    ) {
        throw new ApiError(401, 'A registered device identity is required.');
    }
    $secretHash = hash('sha256', hex2bin($deviceSecret), true);
    $device = $database->prepare(
        'SELECT device_id, signing_public_key FROM resonance_devices
         WHERE device_id = ? AND account_id = ? AND identity_public_key = ?
           AND device_secret_hash = ? AND revoked_at IS NULL',
    );
    $device->execute([
        strtolower($deviceId),
        $account['account_id'],
        strtolower($account['public_key']),
        $secretHash,
    ]);
    $registered = $device->fetch();
    if (!is_array($registered)) {
        throw new ApiError(401, 'The registered device identity is invalid or revoked.');
    }
    $registeredId = $registered['device_id'];
    $touch = $database->prepare(
        'UPDATE resonance_devices SET last_seen_at = UTC_TIMESTAMP(6)
         WHERE device_id = ? AND account_id = ? AND revoked_at IS NULL',
    );
    $touch->execute([$registeredId, $account['account_id']]);
    return [
        'device_id' => $registeredId,
        'signing_public_key' => $registered['signing_public_key'],
    ];
}

function uuid_v4(): string
{
    $bytes = random_bytes(16);
    $bytes[6] = chr((ord($bytes[6]) & 0x0f) | 0x40);
    $bytes[8] = chr((ord($bytes[8]) & 0x3f) | 0x80);
    $hex = bin2hex($bytes);
    return substr($hex, 0, 8) . '-' . substr($hex, 8, 4) . '-' . substr($hex, 12, 4)
        . '-' . substr($hex, 16, 4) . '-' . substr($hex, 20);
}

