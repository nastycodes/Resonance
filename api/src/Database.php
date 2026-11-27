<?php
declare(strict_types=1);

function database(): PDO
{
    static $connection = null;
    if ($connection instanceof PDO) {
        return $connection;
    }
    if (!extension_loaded('pdo_mysql')) {
        throw new ApiError(503, 'The PHP pdo_mysql extension is required.');
    }

    if (
        !preg_match('/\A[A-Za-z0-9.-]+\z/', DB_HOST)
        || !preg_match('/\A[A-Za-z0-9_]{1,32}\z/', DB_USER)
        || !preg_match('/\A[A-Za-z0-9_]{1,64}\z/', DB_NAME)
        || DB_PORT < 1
        || DB_PORT > 65535
        || DB_PASSWORD === 'CHANGE_ME'
    ) {
        throw new ApiError(503, 'Edit the MySQL connection settings in api/src/Config.php.');
    }
    if (DB_SSL_CA !== '' && (!is_file(DB_SSL_CA) || !is_readable(DB_SSL_CA))) {
        throw new ApiError(503, 'The configured MySQL CA certificate cannot be read.');
    }
    if (DB_SSL_CA === '' && !in_array(strtolower(DB_HOST), ['localhost', '127.0.0.1'], true)) {
        throw new ApiError(503, 'Configure DB_SSL_CA when the MySQL server is not local to the API.');
    }

    $dsn = 'mysql:host=' . DB_HOST . ';port=' . DB_PORT . ';dbname=' . DB_NAME . ';charset=utf8mb4';
    try {
        $options = [
            PDO::ATTR_ERRMODE => PDO::ERRMODE_EXCEPTION,
            PDO::ATTR_DEFAULT_FETCH_MODE => PDO::FETCH_ASSOC,
            PDO::ATTR_EMULATE_PREPARES => false,
            PDO::ATTR_TIMEOUT => 5,
        ];
        if (DB_SSL_CA !== '') {
            if (!defined('PDO::MYSQL_ATTR_SSL_CA') || !defined('PDO::MYSQL_ATTR_SSL_VERIFY_SERVER_CERT')) {
                throw new ApiError(503, 'The PHP MySQL driver must support TLS certificate verification.');
            }
            $options[constant('PDO::MYSQL_ATTR_SSL_CA')] = DB_SSL_CA;
            $options[constant('PDO::MYSQL_ATTR_SSL_VERIFY_SERVER_CERT')] = true;
        }
        $connection = new PDO($dsn, DB_USER, DB_PASSWORD, $options);
        $connection->exec("SET time_zone = '+00:00'");
        initialize_schema($connection);
    } catch (PDOException $error) {
        throw new RuntimeException('Could not connect to or initialize the configured MySQL database.', 0, $error);
    }
    return $connection;
}

function initialize_schema(PDO $database): void
{
    $lockName = 'resonance_schema_migrations';
    $lock = $database->prepare('SELECT GET_LOCK(?, 10)');
    $lock->execute([$lockName]);
    if ((int) $lock->fetchColumn() !== 1) {
        throw new RuntimeException('Could not acquire the database schema migration lock.');
    }

    try {
        initialize_schema_locked($database);
    } finally {
        $release = $database->prepare('SELECT RELEASE_LOCK(?)');
        $release->execute([$lockName]);
        $release->fetchColumn();
    }
}

function initialize_schema_locked(PDO $database): void
{
    $database->exec(
        'CREATE TABLE IF NOT EXISTS resonance_schema_migrations (
            version SMALLINT UNSIGNED NOT NULL PRIMARY KEY,
            applied_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4',
    );
    $version = $database->query(
        'SELECT version FROM resonance_schema_migrations ORDER BY version DESC LIMIT 1',
    )->fetchColumn();
    if ((int) $version < 1) {
        $schema = [
        'CREATE TABLE IF NOT EXISTS resonance_accounts (
            account_id CHAR(36) NOT NULL PRIMARY KEY,
            username VARCHAR(32) CHARACTER SET ascii COLLATE ascii_general_ci NOT NULL UNIQUE,
            display_name VARCHAR(64) NOT NULL,
            public_key CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL UNIQUE,
            password_hash VARCHAR(255) NOT NULL,
            created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4',
        'CREATE TABLE IF NOT EXISTS resonance_account_sessions (
            token_hash BINARY(32) NOT NULL PRIMARY KEY,
            account_id CHAR(36) NOT NULL,
            expires_at TIMESTAMP(6) NOT NULL,
            created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
            CONSTRAINT fk_resonance_session_account
                FOREIGN KEY (account_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE,
            INDEX ix_resonance_sessions_expiry (expires_at)
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4',
        'CREATE TABLE IF NOT EXISTS resonance_encrypted_identities (
            account_id CHAR(36) NOT NULL PRIMARY KEY,
            salt CHAR(32) CHARACTER SET ascii NOT NULL,
            nonce CHAR(24) CHARACTER SET ascii NOT NULL,
            encrypted_private_key TEXT CHARACTER SET ascii NOT NULL,
            updated_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                ON UPDATE CURRENT_TIMESTAMP(6),
            CONSTRAINT fk_resonance_identity_account
                FOREIGN KEY (account_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4',
        'CREATE TABLE IF NOT EXISTS resonance_settings (
            account_id CHAR(36) NOT NULL,
            setting_key VARCHAR(64) CHARACTER SET ascii NOT NULL,
            setting_value VARCHAR(255) NOT NULL,
            updated_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                ON UPDATE CURRENT_TIMESTAMP(6),
            PRIMARY KEY (account_id, setting_key),
            CONSTRAINT fk_resonance_setting_account
                FOREIGN KEY (account_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4',
        'CREATE TABLE IF NOT EXISTS resonance_friends (
            account_id CHAR(36) NOT NULL,
            peer_account_id CHAR(36) NOT NULL,
            created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
            PRIMARY KEY (account_id, peer_account_id),
            CONSTRAINT fk_resonance_friend_owner
                FOREIGN KEY (account_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE,
            CONSTRAINT fk_resonance_friend_peer
                FOREIGN KEY (peer_account_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE
        ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4',
        ];
        foreach ($schema as $statement) {
            $database->exec($statement);
        }
        $database->exec('INSERT IGNORE INTO resonance_schema_migrations (version) VALUES (1)');
    }
    if ((int) $version < 2) {
        $database->exec(
            'CREATE TABLE IF NOT EXISTS resonance_peer_endpoints (
                account_id CHAR(36) NOT NULL PRIMARY KEY,
                address VARCHAR(45) CHARACTER SET ascii NOT NULL,
                port SMALLINT UNSIGNED NOT NULL,
                updated_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                    ON UPDATE CURRENT_TIMESTAMP(6),
                CONSTRAINT fk_resonance_endpoint_account
                    FOREIGN KEY (account_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4',
        );
        $friendLinks = $database->query(
            'SELECT account_id, peer_account_id FROM resonance_friends',
        )->fetchAll();
        $reverseLink = $database->prepare(
            'INSERT IGNORE INTO resonance_friends (account_id, peer_account_id) VALUES (?, ?)',
        );
        foreach ($friendLinks as $link) {
            $reverseLink->execute([$link['peer_account_id'], $link['account_id']]);
        }
        $database->exec('DROP TABLE IF EXISTS resonance_messages');
        $database->exec('DROP TABLE IF EXISTS resonance_conversations');
        $database->exec('INSERT IGNORE INTO resonance_schema_migrations (version) VALUES (2)');
    }
    if ((int) $version < 3) {
        $database->exec('DROP TABLE IF EXISTS resonance_communities');
        $database->exec('INSERT IGNORE INTO resonance_schema_migrations (version) VALUES (3)');
    }
    if ((int) $version < 4) {
        $database->exec(
            'CREATE TABLE IF NOT EXISTS resonance_used_invites (
                invite_hash BINARY(32) NOT NULL PRIMARY KEY,
                used_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4',
        );
        $database->exec('INSERT IGNORE INTO resonance_schema_migrations (version) VALUES (4)');
    }
    if ((int) $version < 5) {
        $database->exec(
            'CREATE TABLE IF NOT EXISTS resonance_relay_messages (
                recipient_id CHAR(36) NOT NULL,
                message_id CHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                sender_id CHAR(36) NOT NULL,
                envelope_json MEDIUMTEXT CHARACTER SET ascii NOT NULL,
                created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                PRIMARY KEY (recipient_id, message_id),
                INDEX ix_resonance_relay_recipient_created (recipient_id, created_at),
                CONSTRAINT fk_resonance_relay_recipient
                    FOREIGN KEY (recipient_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE,
                CONSTRAINT fk_resonance_relay_sender
                    FOREIGN KEY (sender_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4',
        );
        $database->exec('INSERT IGNORE INTO resonance_schema_migrations (version) VALUES (5)');
    }
    if ((int) $version < 6) {
        $database->exec(
            'CREATE TABLE IF NOT EXISTS resonance_message_receipts (
                sender_id CHAR(36) NOT NULL,
                recipient_id CHAR(36) NOT NULL,
                message_id CHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                sent_at BIGINT UNSIGNED NOT NULL,
                delivered_at BIGINT UNSIGNED NULL,
                read_at BIGINT UNSIGNED NULL,
                PRIMARY KEY (sender_id, message_id),
                INDEX ix_resonance_receipts_sender_sent (sender_id, sent_at),
                CONSTRAINT fk_resonance_receipts_sender
                    FOREIGN KEY (sender_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE,
                CONSTRAINT fk_resonance_receipts_recipient
                    FOREIGN KEY (recipient_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4',
        );
        $database->exec('INSERT IGNORE INTO resonance_schema_migrations (version) VALUES (6)');
    }
    if ((int) $version < 7) {
        $database->exec(
            'CREATE TABLE IF NOT EXISTS resonance_groups (
                group_id CHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL PRIMARY KEY,
                owner_id CHAR(36) NOT NULL,
                created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                CONSTRAINT fk_resonance_group_owner
                    FOREIGN KEY (owner_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4',
        );
        $database->exec(
            'CREATE TABLE IF NOT EXISTS resonance_group_members (
                group_id CHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                account_id CHAR(36) NOT NULL,
                PRIMARY KEY (group_id, account_id),
                INDEX ix_resonance_group_member_account (account_id),
                CONSTRAINT fk_resonance_group_member_group
                    FOREIGN KEY (group_id) REFERENCES resonance_groups(group_id) ON DELETE CASCADE,
                CONSTRAINT fk_resonance_group_member_account
                    FOREIGN KEY (account_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4',
        );
        $database->exec('INSERT IGNORE INTO resonance_schema_migrations (version) VALUES (7)');
    }
    if ((int) $version < 8) {
        $database->exec(
            'CREATE TABLE IF NOT EXISTS resonance_attachments (
                attachment_id CHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL PRIMARY KEY,
                sender_id CHAR(36) NOT NULL,
                recipient_id CHAR(36) NULL,
                group_id CHAR(32) CHARACTER SET ascii COLLATE ascii_bin NULL,
                content_size INT UNSIGNED NOT NULL,
                created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                expires_at TIMESTAMP(6) NOT NULL,
                INDEX ix_resonance_attachments_sender_expiry (sender_id, expires_at),
                INDEX ix_resonance_attachments_expiry (expires_at),
                CONSTRAINT fk_resonance_attachment_sender
                    FOREIGN KEY (sender_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE,
                CONSTRAINT fk_resonance_attachment_recipient
                    FOREIGN KEY (recipient_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE,
                CONSTRAINT fk_resonance_attachment_group
                    FOREIGN KEY (group_id) REFERENCES resonance_groups(group_id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4',
        );
        $database->exec('INSERT IGNORE INTO resonance_schema_migrations (version) VALUES (8)');
    }
    if ((int) $version < 9) {
        $database->exec(
            'CREATE TABLE IF NOT EXISTS resonance_friend_requests (
                sender_id CHAR(36) NOT NULL,
                recipient_id CHAR(36) NOT NULL,
                created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                PRIMARY KEY (sender_id, recipient_id),
                INDEX ix_resonance_friend_requests_recipient (recipient_id, created_at),
                CONSTRAINT fk_resonance_friend_request_sender
                    FOREIGN KEY (sender_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE,
                CONSTRAINT fk_resonance_friend_request_recipient
                    FOREIGN KEY (recipient_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4',
        );
        $database->exec('INSERT IGNORE INTO resonance_schema_migrations (version) VALUES (9)');
    }
    if ((int) $version < 10) {
        $database->exec(
            'CREATE TABLE IF NOT EXISTS resonance_mls_key_packages (
                account_id CHAR(36) NOT NULL,
                package_id CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                key_package VARCHAR(4096) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                expires_at TIMESTAMP(6) NOT NULL,
                PRIMARY KEY (account_id, package_id),
                INDEX ix_resonance_mls_packages_expiry (account_id, expires_at, created_at),
                CONSTRAINT fk_resonance_mls_package_account
                    FOREIGN KEY (account_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4',
        );
        $database->exec('INSERT IGNORE INTO resonance_schema_migrations (version) VALUES (10)');
    }
    if ((int) $version < 11) {
        $groupVersionColumn = $database->prepare(
            'SELECT COUNT(*) FROM information_schema.COLUMNS
             WHERE TABLE_SCHEMA = DATABASE()
               AND TABLE_NAME = \'resonance_groups\'
               AND COLUMN_NAME = \'membership_version\'',
        );
        $groupVersionColumn->execute();
        if ((int) $groupVersionColumn->fetchColumn() === 0) {
            $database->exec(
                'ALTER TABLE resonance_groups
                 ADD COLUMN membership_version BIGINT UNSIGNED NOT NULL DEFAULT 1',
            );
        }
        $database->exec(
            'CREATE TABLE IF NOT EXISTS resonance_group_operations (
                group_id CHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                operation_id CHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                owner_id CHAR(36) NOT NULL,
                request_hash BINARY(32) NOT NULL,
                result_version BIGINT UNSIGNED NOT NULL,
                created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                PRIMARY KEY (group_id, operation_id),
                CONSTRAINT fk_resonance_group_operation_group
                    FOREIGN KEY (group_id) REFERENCES resonance_groups(group_id) ON DELETE CASCADE,
                CONSTRAINT fk_resonance_group_operation_owner
                    FOREIGN KEY (owner_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4',
        );
        $database->exec('INSERT IGNORE INTO resonance_schema_migrations (version) VALUES (11)');
    }
    if ((int) $version < 12) {
        $queueSequenceColumn = $database->prepare(
            'SELECT COUNT(*) FROM information_schema.COLUMNS
             WHERE TABLE_SCHEMA = DATABASE()
               AND TABLE_NAME = \'resonance_relay_messages\'
               AND COLUMN_NAME = \'queue_sequence\'',
        );
        $queueSequenceColumn->execute();
        if ((int) $queueSequenceColumn->fetchColumn() === 0) {
            $database->exec(
                'ALTER TABLE resonance_relay_messages
                 ADD COLUMN queue_sequence BIGINT UNSIGNED NOT NULL AUTO_INCREMENT UNIQUE FIRST',
            );
        }
        $database->exec('INSERT IGNORE INTO resonance_schema_migrations (version) VALUES (12)');
    }
    if ((int) $version < 13) {
        $database->exec(
            'ALTER TABLE resonance_message_receipts
             DROP PRIMARY KEY,
             ADD PRIMARY KEY (recipient_id, message_id)',
        );
        $database->exec('INSERT IGNORE INTO resonance_schema_migrations (version) VALUES (13)');
    }
    if ((int) $version < 14) {
        $relayExpiryColumn = $database->prepare(
            'SELECT COUNT(*) FROM information_schema.COLUMNS
             WHERE TABLE_SCHEMA = DATABASE()
               AND TABLE_NAME = \'resonance_relay_messages\'
               AND COLUMN_NAME = \'expires_at\'',
        );
        $relayExpiryColumn->execute();
        if ((int) $relayExpiryColumn->fetchColumn() === 0) {
            $database->exec(
                'ALTER TABLE resonance_relay_messages
                 ADD COLUMN expires_at TIMESTAMP(6) NULL',
            );
        }
        $database->exec(
            'UPDATE resonance_relay_messages
             SET expires_at = DATE_ADD(created_at, INTERVAL 30 DAY)
             WHERE expires_at IS NULL',
        );
        $relayExpiryNullable = $database->query(
            'SELECT IS_NULLABLE FROM information_schema.COLUMNS
             WHERE TABLE_SCHEMA = DATABASE()
               AND TABLE_NAME = \'resonance_relay_messages\'
               AND COLUMN_NAME = \'expires_at\'',
        )->fetchColumn();
        if ($relayExpiryNullable === 'YES') {
            $database->exec(
                'ALTER TABLE resonance_relay_messages
                 MODIFY COLUMN expires_at TIMESTAMP(6) NOT NULL',
            );
        }
        $relayExpiryIndex = $database->prepare(
            'SELECT COUNT(*) FROM information_schema.STATISTICS
             WHERE TABLE_SCHEMA = DATABASE()
               AND TABLE_NAME = \'resonance_relay_messages\'
               AND INDEX_NAME = \'ix_resonance_relay_expiry\'',
        );
        $relayExpiryIndex->execute();
        if ((int) $relayExpiryIndex->fetchColumn() === 0) {
            $database->exec(
                'ALTER TABLE resonance_relay_messages
                 ADD INDEX ix_resonance_relay_expiry (expires_at)',
            );
        }
        $database->exec('INSERT IGNORE INTO resonance_schema_migrations (version) VALUES (14)');
    }
    if ((int) $version < 15) {
        $database->exec(
            'CREATE TABLE IF NOT EXISTS resonance_devices (
                device_id CHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL PRIMARY KEY,
                account_id CHAR(36) NOT NULL,
                identity_public_key CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                device_secret_hash BINARY(32) NOT NULL,
                created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                last_seen_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                revoked_at TIMESTAMP(6) NULL,
                INDEX ix_resonance_devices_account_active (account_id, revoked_at),
                CONSTRAINT fk_resonance_device_account
                    FOREIGN KEY (account_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4',
        );
        $database->exec(
            'CREATE TABLE IF NOT EXISTS resonance_message_device_deliveries (
                recipient_id CHAR(36) NOT NULL,
                message_id CHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                device_id CHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                delivery_state ENUM(\'pending\', \'delivered\', \'rejected\', \'revoked\', \'expired\')
                    NOT NULL DEFAULT \'pending\',
                lease_token CHAR(32) CHARACTER SET ascii COLLATE ascii_bin NULL,
                lease_expires_at TIMESTAMP(6) NULL,
                delivered_at BIGINT UNSIGNED NULL,
                created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                updated_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6)
                    ON UPDATE CURRENT_TIMESTAMP(6),
                PRIMARY KEY (recipient_id, message_id, device_id),
                INDEX ix_resonance_device_delivery_poll
                    (device_id, delivery_state, lease_expires_at, recipient_id, message_id),
                INDEX ix_resonance_device_delivery_expiry (created_at),
                CONSTRAINT fk_resonance_device_delivery_device
                    FOREIGN KEY (device_id) REFERENCES resonance_devices(device_id) ON DELETE CASCADE,
                CONSTRAINT fk_resonance_device_delivery_recipient
                    FOREIGN KEY (recipient_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4',
        );
        $database->exec('INSERT IGNORE INTO resonance_schema_migrations (version) VALUES (15)');
    }
    if ((int) $version < 16) {
        $receiptColumns = [
            'group_id' => 'CHAR(32) CHARACTER SET ascii COLLATE ascii_bin NULL',
            'delivery_state' => 'ENUM(\'pending\', \'delivered\', \'failed\', \'expired\', \'rejected\') NOT NULL DEFAULT \'pending\'',
            'expires_at' => 'TIMESTAMP(6) NULL',
            'failure_code' => 'VARCHAR(32) CHARACTER SET ascii COLLATE ascii_bin NULL',
        ];
        foreach ($receiptColumns as $column => $definition) {
            $exists = $database->prepare(
                'SELECT COUNT(*) FROM information_schema.COLUMNS
                 WHERE TABLE_SCHEMA = DATABASE()
                   AND TABLE_NAME = \'resonance_message_receipts\'
                   AND COLUMN_NAME = ?',
            );
            $exists->execute([$column]);
            if ((int) $exists->fetchColumn() === 0) {
                $database->exec(
                    "ALTER TABLE resonance_message_receipts ADD COLUMN `$column` $definition",
                );
            }
        }
        $receiptIndex = $database->prepare(
            'SELECT COUNT(*) FROM information_schema.STATISTICS
             WHERE TABLE_SCHEMA = DATABASE()
               AND TABLE_NAME = \'resonance_message_receipts\'
               AND INDEX_NAME = \'ix_resonance_receipts_group_message\'',
        );
        $receiptIndex->execute();
        if ((int) $receiptIndex->fetchColumn() === 0) {
            $database->exec(
                'ALTER TABLE resonance_message_receipts
                 ADD INDEX ix_resonance_receipts_group_message (sender_id, group_id, message_id)',
            );
        }
        $deviceColumns = [
            'attempt_count' => 'INT UNSIGNED NOT NULL DEFAULT 0',
            'next_attempt_at' => 'TIMESTAMP(6) NULL',
            'failure_code' => 'VARCHAR(32) CHARACTER SET ascii COLLATE ascii_bin NULL',
        ];
        foreach ($deviceColumns as $column => $definition) {
            $exists = $database->prepare(
                'SELECT COUNT(*) FROM information_schema.COLUMNS
                 WHERE TABLE_SCHEMA = DATABASE()
                   AND TABLE_NAME = \'resonance_message_device_deliveries\'
                   AND COLUMN_NAME = ?',
            );
            $exists->execute([$column]);
            if ((int) $exists->fetchColumn() === 0) {
                $database->exec(
                    "ALTER TABLE resonance_message_device_deliveries ADD COLUMN `$column` $definition",
                );
            }
        }
        $database->exec(
            'ALTER TABLE resonance_message_device_deliveries
             MODIFY COLUMN delivery_state
             ENUM(\'pending\', \'delivered\', \'failed\', \'rejected\', \'revoked\', \'expired\')
             NOT NULL DEFAULT \'pending\'',
        );
        $database->exec(
            'UPDATE resonance_message_receipts r
             LEFT JOIN resonance_relay_messages m
               ON m.recipient_id = r.recipient_id AND m.message_id = r.message_id
             SET r.group_id = JSON_UNQUOTE(JSON_EXTRACT(m.envelope_json, \'$.group_id\')),
                 r.delivery_state = CASE
                     WHEN r.delivered_at IS NOT NULL THEN \'delivered\' ELSE \'pending\' END
             WHERE r.expires_at IS NULL',
        );
        $database->exec(
            'UPDATE resonance_message_receipts
             SET expires_at = DATE_ADD(FROM_UNIXTIME(sent_at), INTERVAL 30 DAY)
             WHERE expires_at IS NULL',
        );
        $database->exec(
            'ALTER TABLE resonance_message_receipts
             MODIFY COLUMN expires_at TIMESTAMP(6) NOT NULL',
        );
        $database->exec('INSERT IGNORE INTO resonance_schema_migrations (version) VALUES (16)');
    }
    if ((int) $version < 17) {
        $deviceColumns = [
            'device_name' => 'VARCHAR(80) NOT NULL DEFAULT \'Resonance desktop\'',
            'signing_public_key' => 'CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NULL',
            'encryption_public_key' => 'CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NULL',
        ];
        foreach ($deviceColumns as $column => $definition) {
            $exists = $database->prepare(
                'SELECT COUNT(*) FROM information_schema.COLUMNS
                 WHERE TABLE_SCHEMA = DATABASE()
                   AND TABLE_NAME = \'resonance_devices\'
                   AND COLUMN_NAME = ?',
            );
            $exists->execute([$column]);
            if ((int) $exists->fetchColumn() === 0) {
                $database->exec(
                    "ALTER TABLE resonance_devices ADD COLUMN `$column` $definition",
                );
            }
        }
        $database->exec(
            'CREATE TABLE IF NOT EXISTS resonance_device_enrollments (
                request_id CHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL PRIMARY KEY,
                account_id CHAR(36) NOT NULL,
                device_id CHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                device_name VARCHAR(80) NOT NULL,
                device_secret_hash BINARY(32) NOT NULL,
                signing_public_key CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                encryption_public_key CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                nonce CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                request_signature CHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                enrollment_state ENUM(\'pending\', \'authorized\', \'expired\')
                    NOT NULL DEFAULT \'pending\',
                approved_by_device_id CHAR(32) CHARACTER SET ascii COLLATE ascii_bin NULL,
                created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                expires_at TIMESTAMP(6) NOT NULL,
                approved_at TIMESTAMP(6) NULL,
                INDEX ix_resonance_device_enrollment_pending
                    (account_id, enrollment_state, expires_at),
                INDEX ix_resonance_device_enrollment_device (account_id, device_id),
                CONSTRAINT fk_resonance_device_enrollment_account
                    FOREIGN KEY (account_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4',
        );
        $database->exec(
            'CREATE TABLE IF NOT EXISTS resonance_device_history_syncs (
                sync_id CHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL PRIMARY KEY,
                account_id CHAR(36) NOT NULL,
                source_device_id CHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                target_device_id CHAR(32) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                ephemeral_public_key CHAR(64) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                nonce CHAR(24) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                ciphertext MEDIUMBLOB NOT NULL,
                signature CHAR(128) CHARACTER SET ascii COLLATE ascii_bin NOT NULL,
                created_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6),
                expires_at TIMESTAMP(6) NOT NULL,
                INDEX ix_resonance_device_history_target (target_device_id, expires_at),
                CONSTRAINT fk_resonance_device_history_account
                    FOREIGN KEY (account_id) REFERENCES resonance_accounts(account_id) ON DELETE CASCADE,
                CONSTRAINT fk_resonance_device_history_source
                    FOREIGN KEY (source_device_id) REFERENCES resonance_devices(device_id) ON DELETE CASCADE,
                CONSTRAINT fk_resonance_device_history_target_device
                    FOREIGN KEY (target_device_id) REFERENCES resonance_devices(device_id) ON DELETE CASCADE
            ) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4',
        );
        $database->exec('INSERT IGNORE INTO resonance_schema_migrations (version) VALUES (17)');
    }
    if ((int) $version < 18) {
        $profileImageColumn = $database->prepare(
            'SELECT COUNT(*) FROM information_schema.COLUMNS
             WHERE TABLE_SCHEMA = DATABASE()
               AND TABLE_NAME = \'resonance_accounts\'
               AND COLUMN_NAME = \'profile_image\'',
        );
        $profileImageColumn->execute();
        if ((int) $profileImageColumn->fetchColumn() === 0) {
            $database->exec(
                'ALTER TABLE resonance_accounts ADD COLUMN profile_image MEDIUMBLOB NULL',
            );
        }
        $database->exec('INSERT IGNORE INTO resonance_schema_migrations (version) VALUES (18)');
    }
}

