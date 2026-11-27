<?php
declare(strict_types=1);

const MAX_ATTACHMENT_BYTES = 26214400;
const MAX_ACTIVE_ATTACHMENT_BYTES_PER_SENDER = 104857600;

function attachment_storage_directory(): string
{
    $configured = getenv('RESONANCE_ATTACHMENT_STORAGE');
    $directory = is_string($configured) && $configured !== ''
        ? $configured
        : (ATTACHMENT_STORAGE_PATH !== ''
            ? ATTACHMENT_STORAGE_PATH
            : dirname(__DIR__) . '/storage/attachments');
    if (
        !str_starts_with($directory, DIRECTORY_SEPARATOR)
        && preg_match('/\A[A-Za-z]:[\\\\\/]/', $directory) !== 1
    ) {
        throw new RuntimeException('The private attachment directory must be an absolute path.');
    }
    if (!is_dir($directory) && !mkdir($directory, 0700, true) && !is_dir($directory)) {
        throw new RuntimeException('Could not create the private attachment directory.');
    }
    if (is_link($directory) || !is_writable($directory)) {
        throw new RuntimeException('The private attachment directory is not writable or is a symbolic link.');
    }
    if (!chmod($directory, 0700)) {
        throw new RuntimeException('Could not restrict access to the private attachment directory.');
    }
    return $directory;
}

function attachment_file_path(string $attachmentId): string
{
    return attachment_storage_directory() . DIRECTORY_SEPARATOR . $attachmentId . '.bin';
}

function read_attachment_body(): string
{
    $length = $_SERVER['CONTENT_LENGTH'] ?? null;
    if (is_string($length) && ctype_digit($length) && (int) $length > MAX_ATTACHMENT_BYTES) {
        throw new ApiError(413, 'The attachment exceeds the 25 MiB limit.');
    }
    $input = fopen('php://input', 'rb');
    if ($input === false) {
        throw new RuntimeException('Could not read the attachment request body.');
    }
    $body = '';
    while (!feof($input)) {
        $chunk = fread($input, min(8192, MAX_ATTACHMENT_BYTES + 1 - strlen($body)));
        if ($chunk === false) {
            fclose($input);
            throw new RuntimeException('Could not read the attachment request body.');
        }
        $body .= $chunk;
        if (strlen($body) > MAX_ATTACHMENT_BYTES) {
            fclose($input);
            throw new ApiError(413, 'The attachment exceeds the 25 MiB limit.');
        }
    }
    fclose($input);
    if (strlen($body) < 16) {
        throw new ApiError(400, 'The attachment is empty or too small to be encrypted data.');
    }
    return $body;
}

function write_attachment_file(string $path, string $contents): void
{
    $file = fopen($path, 'xb');
    if ($file === false) {
        if (file_exists($path)) {
            throw new ApiError(409, 'The attachment identifier is already in use.');
        }
        throw new RuntimeException('Could not create the attachment file.');
    }
    try {
        $offset = 0;
        $length = strlen($contents);
        while ($offset < $length) {
            $written = fwrite($file, $contents, $length - $offset);
            if ($written === false || $written === 0) {
                throw new RuntimeException('Could not write the attachment file.');
            }
            $offset += $written;
        }
        if (!fflush($file)) {
            throw new RuntimeException('Could not flush the attachment file.');
        }
    } catch (Throwable $error) {
        fclose($file);
        if (is_file($path)) {
            unlink($path);
        }
        throw $error;
    }
    fclose($file);
    if (!chmod($path, 0600)) {
        unlink($path);
        throw new RuntimeException('Could not restrict access to the attachment file.');
    }
}

function purge_expired_attachments(PDO $database): void
{
    $expired = $database->query(
        'SELECT attachment_id FROM resonance_attachments
         WHERE expires_at <= UTC_TIMESTAMP(6) LIMIT 100',
    )->fetchAll(PDO::FETCH_COLUMN);
    foreach ($expired as $attachmentId) {
        if (is_string($attachmentId)) {
            $path = attachment_file_path($attachmentId);
            if (is_file($path) && !unlink($path)) {
                throw new RuntimeException('Could not remove an expired attachment file.');
            }
        }
    }
    if ($expired !== []) {
        $placeholders = implode(',', array_fill(0, count($expired), '?'));
        $delete = $database->prepare(
            "DELETE FROM resonance_attachments
             WHERE attachment_id IN ($placeholders) AND expires_at <= UTC_TIMESTAMP(6)",
        );
        $delete->execute($expired);
    }
}

