<?php
declare(strict_types=1);

function string_field(array $body, string $field): string
{
    if (!isset($body[$field]) || !is_string($body[$field])) {
        throw new ApiError(400, "Field '$field' is required.");
    }
    return $body[$field];
}

function validate_username(string $username): string
{
    $username = strtolower(trim($username));
    if (!preg_match('/\A[A-Za-z0-9_-]{3,32}\z/', $username)) {
        throw new ApiError(400, 'Username must be 3-32 letters, numbers, underscores, or hyphens.');
    }
    return $username;
}

function validate_display_name(string $displayName): string
{
    if (str_contains($displayName, "\0")) {
        throw new ApiError(400, 'Display name must contain 1-64 visible characters.');
    }
    $displayName = trim($displayName);
    $characterCount = preg_match_all('/./us', $displayName);
    if (
        $characterCount === false
        || $characterCount < 1
        || $characterCount > 64
        || preg_match('/\p{Cc}/u', $displayName) === 1
    ) {
        throw new ApiError(400, 'Display name must contain 1-64 visible characters.');
    }
    return $displayName;
}

function validate_password(string $password): void
{
    $characterCount = preg_match_all('/./us', $password);
    if (
        $characterCount === false
        || $characterCount < 10
        || strlen($password) > 1024
        || str_contains($password, "\0")
    ) {
        throw new ApiError(400, 'Password must contain 10-1024 characters.');
    }
}

function validate_invite_code(string $inviteCode): string
{
    $inviteCode = strtolower(trim($inviteCode));
    if (!preg_match('/\A[a-f0-9]{48}\z/', $inviteCode)) {
        throw new ApiError(403, 'Invitation code is invalid or has already been used.');
    }
    foreach (REGISTRATION_INVITE_CODES as $configuredCode) {
        if (is_string($configuredCode) && hash_equals(strtolower($configuredCode), $inviteCode)) {
            return hash('sha256', $inviteCode, true);
        }
    }
    throw new ApiError(403, 'Invitation code is invalid or has already been used.');
}

function validate_public_key(string $publicKey): string
{
    $publicKey = strtolower(trim($publicKey));
    if (!preg_match('/\A[a-f0-9]{64}\z/', $publicKey)) {
        throw new ApiError(400, 'The public identity key is invalid.');
    }
    return $publicKey;
}

function validate_device_name(string $name): string
{
    $name = trim($name);
    $length = preg_match_all('/./us', $name);
    if (
        $length === false
        || $length < 1
        || $length > 80
        || preg_match('/\p{Cc}/u', $name) === 1
    ) {
        throw new ApiError(400, 'Device names must contain 1-80 visible characters.');
    }
    return $name;
}

function device_enrollment_request_transcript(
    string $accountPublicKey,
    string $deviceId,
    string $deviceSecretHash,
    string $signingPublicKey,
    string $encryptionPublicKey,
    string $requestId,
    string $nonce,
    string $deviceName,
): string {
    return "Resonance device enrollment request v1\0"
        . implode("\0", [
            $accountPublicKey,
            $deviceId,
            $deviceSecretHash,
            $signingPublicKey,
            $encryptionPublicKey,
            $requestId,
            $nonce,
            $deviceName,
        ]);
}

function device_enrollment_approval_transcript(
    string $accountPublicKey,
    string $requestId,
    string $deviceId,
    string $signingPublicKey,
    string $encryptionPublicKey,
    string $nonce,
    string $requestSignature,
): string {
    return "Resonance device enrollment approval v1\0"
        . implode("\0", [
            $accountPublicKey,
            $requestId,
            $deviceId,
            $signingPublicKey,
            $encryptionPublicKey,
            $nonce,
            $requestSignature,
        ]);
}

function validate_encrypted_identity(array $body): array
{
    if (!isset($body['encrypted_identity']) || !is_array($body['encrypted_identity'])) {
        throw new ApiError(400, 'Encrypted identity data is required.');
    }
    $identity = $body['encrypted_identity'];
    $salt = $identity['salt'] ?? null;
    $nonce = $identity['nonce'] ?? null;
    $encryptedKey = $identity['encrypted_private_key'] ?? null;
    $encryptedKeyLength = is_string($encryptedKey) ? strlen($encryptedKey) : 0;
    if (
        !is_string($salt)
        || !preg_match('/\A[a-f0-9]{32}\z/', $salt)
        || !is_string($nonce)
        || !preg_match('/\A[a-f0-9]{24}\z/', $nonce)
        || !is_string($encryptedKey)
        || $encryptedKeyLength < 64
        || $encryptedKeyLength > 8192
        || $encryptedKeyLength % 2 !== 0
        || !preg_match('/\A[a-f0-9]+\z/', $encryptedKey)
    ) {
        throw new ApiError(400, 'Encrypted identity data is invalid.');
    }
    return [
        'salt' => $salt,
        'nonce' => $nonce,
        'encrypted_private_key' => $encryptedKey,
    ];
}

function validate_setting(string $key, string $value): array
{
    if ($key === 'notifications' && in_array($value, ['true', 'false'], true)) {
        return [$key, $value];
    }
    if ($key === 'presence' && in_array($value, ['Online', 'Away', 'Do not disturb'], true)) {
        return [$key, $value];
    }
    throw new ApiError(400, 'The setting name or value is not supported.');
}

function validate_relay_envelope(array $body, string $senderPublicKey, string $recipientPublicKey): array
{
    $envelope = $body['envelope'] ?? null;
    if (!is_array($envelope)) {
        throw new ApiError(400, 'An encrypted message envelope is required.');
    }
    $messageId = $envelope['message_id'] ?? null;
    $senderId = $envelope['sender_id'] ?? null;
    $recipientId = $envelope['recipient_id'] ?? null;
    $sentAt = $envelope['sent_at'] ?? null;
    $ephemeralKey = $envelope['ephemeral_public_key'] ?? null;
    $nonce = $envelope['nonce'] ?? null;
    $ciphertext = $envelope['ciphertext'] ?? null;
    $signature = $envelope['signature'] ?? null;
    $groupId = $envelope['group_id'] ?? null;
    if (
        !is_string($messageId)
        || !preg_match('/\A[a-f0-9]{32}\z/i', $messageId)
        || !is_string($senderId)
        || !hash_equals($senderPublicKey, strtolower($senderId))
        || !is_string($recipientId)
        || !hash_equals($recipientPublicKey, strtolower($recipientId))
        || !is_string($sentAt)
        || !preg_match('/\A[0-9]{1,20}\z/', $sentAt)
        || !is_string($ephemeralKey)
        || !preg_match('/\A[a-f0-9]{64}\z/i', $ephemeralKey)
        || !is_string($nonce)
        || !preg_match('/\A[a-f0-9]{24}\z/i', $nonce)
        || !is_string($ciphertext)
        || !preg_match('/\A[a-f0-9]{34,60032}\z/i', $ciphertext)
        || !is_string($signature)
        || !preg_match('/\A[a-f0-9]{128}\z/i', $signature)
        || ($groupId !== null && (
            !is_string($groupId)
            || !preg_match('/\A[a-f0-9]{32}\z/i', $groupId)
        ))
    ) {
        throw new ApiError(400, 'The encrypted message envelope is invalid.');
    }
    $validated = [
        'message_id' => strtolower($messageId),
        'sender_id' => strtolower($senderId),
        'recipient_id' => strtolower($recipientId),
        'sent_at' => $sentAt,
        'ephemeral_public_key' => strtolower($ephemeralKey),
        'nonce' => strtolower($nonce),
        'ciphertext' => strtolower($ciphertext),
        'signature' => strtolower($signature),
    ];
    if (is_string($groupId)) {
        $validated['group_id'] = strtolower($groupId);
    }
    return $validated;
}

