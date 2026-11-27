<?php
declare(strict_types=1);

function respond(int $status, array|object $body): never
{
    http_response_code($status);
    header('Content-Type: application/json; charset=utf-8');
    header('Cache-Control: no-store');
    echo json_encode($body, JSON_UNESCAPED_SLASHES | JSON_THROW_ON_ERROR);
    exit;
}

function respond_empty(int $status): never
{
    http_response_code($status);
    header('Cache-Control: no-store');
    exit;
}

function request_body(int $maximumBytes = 65536): array
{
    $raw = file_get_contents('php://input');
    if ($raw === false || strlen($raw) > $maximumBytes) {
        throw new ApiError(413, 'Request body is missing or too large.');
    }
    try {
        $body = json_decode($raw, true, 16, JSON_THROW_ON_ERROR);
    } catch (JsonException) {
        throw new ApiError(400, 'Request body must be valid JSON.');
    }
    if (!is_array($body)) {
        throw new ApiError(400, 'Request body must be a JSON object.');
    }
    return $body;
}

function request_header(string $name): ?string
{
    $key = 'HTTP_' . strtoupper(str_replace('-', '_', $name));
    $value = $_SERVER[$key] ?? null;
    return is_string($value) ? trim($value) : null;
}

