<?php
declare(strict_types=1);

const DB_HOST = 'localhost';
const DB_PORT = 3306;
const DB_NAME = 'resonance';
const DB_USER = 'resonance';
const DB_PASSWORD = 'CHANGE_ME';
const DB_SSL_CA = '';
const ATTACHMENT_STORAGE_PATH = '';
const RELAY_DELIVERY_MAX_ATTEMPTS = 5;
const RELAY_DELIVERY_LEASE_SECONDS = 90;
const MAX_DEVICE_HISTORY_BYTES = 3145728;
const REGISTRATION_INVITE_CODES = [
    // Add one unique 48-character hexadecimal code per friend.
];


final class ApiError extends RuntimeException
{
    public function __construct(
        public readonly int $status,
        string $message,
    ) {
        parent::__construct($message);
    }
}

