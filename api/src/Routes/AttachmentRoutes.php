<?php
declare(strict_types=1);

function handle_attachment_routes(PDO $database, string $method, string $path): void
{
    if ($method === 'POST' && $path === '/v1/attachments') {
        $account = account_for_token($database);
        purge_expired_attachments($database);
        if (function_exists('set_time_limit')) {
            if (!set_time_limit(300)) {
                error_log('Resonance could not extend the PHP attachment request timeout.');
            }
        }
        $attachmentId = request_header('X-Resonance-Attachment-Id');
        $recipientPublicKey = request_header('X-Resonance-Recipient');
        $groupId = request_header('X-Resonance-Group');
        if (
            $attachmentId === null
            || preg_match('/\A[a-f0-9]{32}\z/i', $attachmentId) !== 1
            || $recipientPublicKey === null
            || preg_match('/\A[a-f0-9]{64}\z/i', $recipientPublicKey) !== 1
        ) {
            throw new ApiError(400, 'Attachment routing details are invalid.');
        }
        if ($groupId !== null && preg_match('/\A[a-f0-9]{32}\z/i', $groupId) !== 1) {
            throw new ApiError(400, 'The attachment group identifier is invalid.');
        }
        $recipient = $database->prepare(
            'SELECT account_id FROM resonance_accounts WHERE public_key = ?',
        );
        $recipient->execute([strtolower($recipientPublicKey)]);
        $recipientId = $recipient->fetchColumn();
        if (!is_string($recipientId) || $recipientId === $account['account_id']) {
            throw new ApiError(404, 'The attachment recipient was not found.');
        }
        if ($groupId === null) {
            $friend = $database->prepare(
                'SELECT 1 FROM resonance_friends
                 WHERE account_id = ? AND peer_account_id = ?',
            );
            $friend->execute([$account['account_id'], $recipientId]);
            if ($friend->fetchColumn() === false) {
                throw new ApiError(403, 'Attachments can only be sent to a friend.');
            }
        } else {
            $member = $database->prepare(
                'SELECT COUNT(*) FROM resonance_group_members
                 WHERE group_id = ? AND account_id IN (?, ?)',
            );
            $member->execute([strtolower($groupId), $account['account_id'], $recipientId]);
            if ((int) $member->fetchColumn() !== 2) {
                throw new ApiError(403, 'Both attachment participants must belong to the group.');
            }
            $recipientId = null;
        }
        $contents = read_attachment_body();
        $attachmentId = strtolower($attachmentId);
        $path = null;
        $fileCreated = false;
        $database->beginTransaction();
        try {
            $lockSender = $database->prepare(
                'SELECT account_id FROM resonance_accounts WHERE account_id = ? FOR UPDATE',
            );
            $lockSender->execute([$account['account_id']]);
            $quota = $database->prepare(
                'SELECT COALESCE(SUM(content_size), 0) FROM resonance_attachments
                 WHERE sender_id = ? AND expires_at > UTC_TIMESTAMP(6)',
            );
            $quota->execute([$account['account_id']]);
            if ((int) $quota->fetchColumn() + strlen($contents) > MAX_ACTIVE_ATTACHMENT_BYTES_PER_SENDER) {
                throw new ApiError(413, 'The sender has reached the 100 MiB active attachment limit.');
            }
            $path = attachment_file_path($attachmentId);
            if (file_exists($path)) {
                throw new ApiError(409, 'The attachment identifier is already in use.');
            }
            write_attachment_file($path, $contents);
            $fileCreated = true;
            $insert = $database->prepare(
                'INSERT INTO resonance_attachments
                 (attachment_id, sender_id, recipient_id, group_id, content_size, expires_at)
                 VALUES (?, ?, ?, ?, ?, DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 30 DAY))',
            );
            $insert->execute([
                $attachmentId,
                $account['account_id'],
                $recipientId,
                $groupId === null ? null : strtolower($groupId),
                strlen($contents),
            ]);
            $database->commit();
        } catch (Throwable $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            if ($fileCreated && is_string($path) && is_file($path)) {
                unlink($path);
            }
            if ($error instanceof PDOException && $error->getCode() === '23000') {
                throw new ApiError(409, 'The attachment identifier is already in use.');
            }
            throw $error;
        }
        respond(201, ['attachment_id' => $attachmentId, 'expires_in_days' => 30]);
    }

    if (
        $method === 'GET'
        && preg_match('/\A\/v1\/attachments\/([a-f0-9]{32})\z/i', $path, $matches) === 1
    ) {
        $account = account_for_token($database);
        purge_expired_attachments($database);
        if (function_exists('set_time_limit')) {
            if (!set_time_limit(300)) {
                error_log('Resonance could not extend the PHP attachment request timeout.');
            }
        }
        $attachmentId = strtolower($matches[1]);
        $lookup = $database->prepare(
            'SELECT sender_id, recipient_id, group_id, content_size
             FROM resonance_attachments
             WHERE attachment_id = ? AND expires_at > UTC_TIMESTAMP(6)',
        );
        $lookup->execute([$attachmentId]);
        $attachment = $lookup->fetch();
        if (!is_array($attachment)) {
            throw new ApiError(404, 'The attachment was not found or has expired.');
        }
        $authorized = $attachment['sender_id'] === $account['account_id']
            || $attachment['recipient_id'] === $account['account_id'];
        if (is_string($attachment['group_id'])) {
            $member = $database->prepare(
                'SELECT 1 FROM resonance_group_members WHERE group_id = ? AND account_id = ?',
            );
            $member->execute([$attachment['group_id'], $account['account_id']]);
            $authorized = $member->fetchColumn() !== false;
        }
        if (!$authorized) {
            throw new ApiError(403, 'You are not allowed to download this attachment.');
        }
        $file = fopen(attachment_file_path($attachmentId), 'rb');
        if ($file === false) {
            throw new RuntimeException('The stored attachment file is unavailable.');
        }
        http_response_code(200);
        header('Content-Type: application/octet-stream');
        header('Content-Length: ' . (int) $attachment['content_size']);
        header('Cache-Control: no-store');
        header('X-Content-Type-Options: nosniff');
        fpassthru($file);
        fclose($file);
        exit;
    }

}
