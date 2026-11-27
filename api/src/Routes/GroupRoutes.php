<?php
declare(strict_types=1);

function handle_group_routes(PDO $database, string $method, string $path): void
{
    if ($method === 'POST' && $path === '/v1/groups') {
        $account = account_for_token($database);
        $body = request_body();
        $groupId = strtolower(string_field($body, 'group_id'));
        if (!preg_match('/\A[a-f0-9]{32}\z/', $groupId)) {
            throw new ApiError(400, 'The group identifier is invalid.');
        }
        $memberKeys = $body['member_public_keys'] ?? null;
        if (!is_array($memberKeys) || count($memberKeys) < 3 || count($memberKeys) > 12) {
            throw new ApiError(400, 'A group must have 3-12 members, including its creator.');
        }
        $memberIds = [];
        $memberPublicKeys = [];
        foreach ($memberKeys as $memberKey) {
            if (!is_string($memberKey)) {
                throw new ApiError(400, 'The group member list is invalid.');
            }
            $memberKey = validate_public_key($memberKey);
            if (isset($memberPublicKeys[$memberKey])) {
                throw new ApiError(400, 'Group members cannot be duplicated.');
            }
            $memberPublicKeys[$memberKey] = true;
            $memberQuery = $database->prepare(
                'SELECT account_id FROM resonance_accounts WHERE public_key = ?',
            );
            $memberQuery->execute([$memberKey]);
            $memberId = $memberQuery->fetchColumn();
            if (!is_string($memberId)) {
                throw new ApiError(404, 'A group member account no longer exists.');
            }
            if ($memberId !== $account['account_id']) {
                $friendship = $database->prepare(
                    'SELECT COUNT(*) FROM resonance_friends
                     WHERE (account_id = ? AND peer_account_id = ?)
                        OR (account_id = ? AND peer_account_id = ?)',
                );
                $friendship->execute([
                    $account['account_id'],
                    $memberId,
                    $memberId,
                    $account['account_id'],
                ]);
                if ((int) $friendship->fetchColumn() !== 2) {
                    throw new ApiError(403, 'Groups can include only your mutual friends.');
                }
            }
            $memberIds[] = $memberId;
        }
        if (!isset($memberPublicKeys[strtolower($account['public_key'])])) {
            throw new ApiError(400, 'The group creator must be a member.');
        }
        $existingGroup = $database->prepare(
            'SELECT 1 FROM resonance_groups WHERE group_id = ?',
        );
        $existingGroup->execute([$groupId]);
        if ($existingGroup->fetchColumn() !== false) {
            throw new ApiError(409, 'That group identifier already exists.');
        }
        $database->beginTransaction();
        try {
            $createGroup = $database->prepare(
                'INSERT INTO resonance_groups (group_id, owner_id) VALUES (?, ?)',
            );
            $createGroup->execute([$groupId, $account['account_id']]);
            $addMember = $database->prepare(
                'INSERT INTO resonance_group_members (group_id, account_id) VALUES (?, ?)',
            );
            foreach ($memberIds as $memberId) {
                $addMember->execute([$groupId, $memberId]);
            }
            $database->commit();
        } catch (Throwable $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            throw $error;
        }
        respond(201, ['group_id' => $groupId]);
    }

    if (
        preg_match('/\A\/v1\/groups\/([a-f0-9]{32})(?:\/members)?\z/i', $path, $groupMatches) === 1
        && $method === 'GET'
        && !str_ends_with($path, '/members')
    ) {
        $account = account_for_token($database);
        $groupId = strtolower($groupMatches[1]);
        $membership = $database->prepare(
            'SELECT 1 FROM resonance_group_members
             WHERE group_id = ? AND account_id = ?',
        );
        $membership->execute([$groupId, $account['account_id']]);
        if ($membership->fetchColumn() === false) {
            throw new ApiError(403, 'Only current group members can read its roster.');
        }
        $group = $database->prepare(
            'SELECT g.membership_version, owner.public_key AS owner_public_key
             FROM resonance_groups g
             JOIN resonance_accounts owner ON owner.account_id = g.owner_id
             WHERE g.group_id = ?',
        );
        $group->execute([$groupId]);
        $groupRow = $group->fetch();
        if (!is_array($groupRow)) {
            throw new ApiError(404, 'The group does not exist.');
        }
        $members = $database->prepare(
            'SELECT a.public_key, a.username, a.display_name
             FROM resonance_group_members m
             JOIN resonance_accounts a ON a.account_id = m.account_id
             WHERE m.group_id = ? ORDER BY a.public_key',
        );
        $members->execute([$groupId]);
        respond(200, [
            'group_id' => $groupId,
            'version' => (int) $groupRow['membership_version'],
            'owner_public_key' => $groupRow['owner_public_key'],
            'members' => $members->fetchAll(),
        ]);
    }

    if (
        preg_match('/\A\/v1\/groups\/([a-f0-9]{32})\/members\z/i', $path, $groupMatches) === 1
        && $method === 'PUT'
    ) {
        $account = account_for_token($database);
        device_for_account($database, $account);
        $groupId = strtolower($groupMatches[1]);
        $body = request_body();
        $operationId = strtolower(string_field($body, 'operation_id'));
        if (!preg_match('/\A[a-f0-9]{32}\z/', $operationId)) {
            throw new ApiError(400, 'The membership operation identifier is invalid.');
        }
        $expectedVersion = $body['expected_version'] ?? null;
        if (!is_int($expectedVersion) || $expectedVersion < 1 || $expectedVersion >= PHP_INT_MAX) {
            throw new ApiError(400, 'The expected group version is invalid.');
        }
        $requestedKeys = $body['member_public_keys'] ?? null;
        if (!is_array($requestedKeys) || count($requestedKeys) < 3 || count($requestedKeys) > 12) {
            throw new ApiError(400, 'A group roster must contain 3-12 members.');
        }
        $requestedKeys = array_map(
            static fn ($publicKey): string => is_string($publicKey)
                ? validate_public_key($publicKey)
                : throw new ApiError(400, 'A group member identity is invalid.'),
            $requestedKeys,
        );
        if (count(array_unique($requestedKeys)) !== count($requestedKeys)) {
            throw new ApiError(400, 'Group members cannot be duplicated.');
        }
        if (!in_array(strtolower($account['public_key']), $requestedKeys, true)) {
            throw new ApiError(400, 'The group owner cannot be removed.');
        }
        $deliveries = $body['deliveries'] ?? null;
        if (!is_array($deliveries) || count($deliveries) < 2 || count($deliveries) > 12) {
            throw new ApiError(400, 'Encrypted membership deliveries are invalid.');
        }
        $requestHash = hash(
            'sha256',
            json_encode(
                [
                    'expected_version' => $expectedVersion,
                    'member_public_keys' => $requestedKeys,
                    'deliveries' => $deliveries,
                ],
                JSON_UNESCAPED_SLASHES | JSON_THROW_ON_ERROR,
            ),
            true,
        );

        $database->beginTransaction();
        try {
            $group = $database->prepare(
                'SELECT g.owner_id, g.membership_version, owner.public_key AS owner_public_key
                 FROM resonance_groups g
                 JOIN resonance_accounts owner ON owner.account_id = g.owner_id
                 WHERE g.group_id = ? FOR UPDATE',
            );
            $group->execute([$groupId]);
            $groupRow = $group->fetch();
            if (!is_array($groupRow)) {
                throw new ApiError(404, 'The group does not exist.');
            }
            if ($groupRow['owner_id'] !== $account['account_id']) {
                throw new ApiError(403, 'Only the group owner can change membership.');
            }
            $existingOperation = $database->prepare(
                'SELECT request_hash, result_version FROM resonance_group_operations
                 WHERE group_id = ? AND operation_id = ? AND owner_id = ?',
            );
            $existingOperation->execute([$groupId, $operationId, $account['account_id']]);
            $operationRow = $existingOperation->fetch();
            if (is_array($operationRow)) {
                if (!hash_equals($operationRow['request_hash'], $requestHash)) {
                    throw new ApiError(409, 'The operation identifier was reused with different data.');
                }
                $database->commit();
                respond(200, ['version' => (int) $operationRow['result_version']]);
            }
            if ((int) $groupRow['membership_version'] !== $expectedVersion) {
                throw new ApiError(409, 'The group roster changed; reload it before retrying.');
            }
            $currentMembers = $database->prepare(
                'SELECT a.account_id, a.public_key
                 FROM resonance_group_members m
                 JOIN resonance_accounts a ON a.account_id = m.account_id
                 WHERE m.group_id = ? ORDER BY a.public_key FOR UPDATE',
            );
            $currentMembers->execute([$groupId]);
            $currentRows = $currentMembers->fetchAll();
            $currentKeys = array_map(
                static fn (array $member): string => strtolower($member['public_key']),
                $currentRows,
            );
            $addedKeys = array_values(array_diff($requestedKeys, $currentKeys));
            $removedKeys = array_values(array_diff($currentKeys, $requestedKeys));
            if (count($addedKeys) + count($removedKeys) !== 1) {
                throw new ApiError(400, 'Each operation must add or remove exactly one member.');
            }

            $addedAccount = null;
            if (count($addedKeys) === 1) {
                $lookup = $database->prepare(
                    'SELECT account_id, public_key FROM resonance_accounts WHERE public_key = ?',
                );
                $lookup->execute([$addedKeys[0]]);
                $addedAccount = $lookup->fetch();
                if (!is_array($addedAccount)) {
                    throw new ApiError(404, 'The new group member account does not exist.');
                }
                $friendship = $database->prepare(
                    'SELECT COUNT(*) FROM resonance_friends
                     WHERE (account_id = ? AND peer_account_id = ?)
                        OR (account_id = ? AND peer_account_id = ?)',
                );
                $friendship->execute([
                    $account['account_id'],
                    $addedAccount['account_id'],
                    $addedAccount['account_id'],
                    $account['account_id'],
                ]);
                if ((int) $friendship->fetchColumn() !== 2) {
                    throw new ApiError(403, 'New group members must be mutual friends of the owner.');
                }
            }
            if (count($removedKeys) === 1 && $removedKeys[0] === strtolower($account['public_key'])) {
                throw new ApiError(400, 'The group owner cannot be removed.');
            }

            $expectedRecipients = array_values(array_filter(
                $currentKeys,
                static fn (string $key): bool => $key !== strtolower($account['public_key']),
            ));
            if (is_array($addedAccount)) {
                $expectedRecipients[] = strtolower($addedAccount['public_key']);
            }
            sort($expectedRecipients);
            $deliveryByRecipient = [];
            $deliveryMessageIds = [];
            foreach ($deliveries as $delivery) {
                if (!is_array($delivery) || !is_string($delivery['recipient_public_key'] ?? null)) {
                    throw new ApiError(400, 'An encrypted membership delivery is invalid.');
                }
                $recipientPublicKey = validate_public_key($delivery['recipient_public_key']);
                if (isset($deliveryByRecipient[$recipientPublicKey])) {
                    throw new ApiError(400, 'A membership recipient was listed more than once.');
                }
                $envelope = validate_relay_envelope(
                    ['envelope' => $delivery['envelope'] ?? null],
                    strtolower($account['public_key']),
                    $recipientPublicKey,
                );
                if (
                    ($envelope['group_id'] ?? null) !== $groupId
                ) {
                    throw new ApiError(400, 'A membership delivery is bound to another group.');
                }
                if (isset($deliveryMessageIds[$envelope['message_id']])) {
                    throw new ApiError(400, 'Membership delivery identifiers must be unique.');
                }
                $deliveryMessageIds[$envelope['message_id']] = true;
                $deliveryByRecipient[$recipientPublicKey] = $envelope;
            }
            $actualRecipients = array_keys($deliveryByRecipient);
            sort($actualRecipients);
            if ($actualRecipients !== $expectedRecipients) {
                throw new ApiError(400, 'Membership deliveries do not match the affected roster.');
            }

            $queueSize = $database->prepare(
                'SELECT COUNT(*) FROM resonance_relay_messages WHERE recipient_id = ?',
            );
            $insertMessage = $database->prepare(
                'INSERT IGNORE INTO resonance_relay_messages
                 (recipient_id, message_id, sender_id, envelope_json, expires_at)
                 VALUES (?, ?, ?, ?, DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 30 DAY))',
            );
            $insertReceipt = $database->prepare(
                'INSERT IGNORE INTO resonance_message_receipts
                 (sender_id, recipient_id, message_id, sent_at, group_id,
                  delivery_state, expires_at)
                 VALUES (?, ?, ?, ?, ?, \'pending\', DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 30 DAY))',
            );
            $insertDeviceDeliveries = $database->prepare(
                'INSERT IGNORE INTO resonance_message_device_deliveries
                 (recipient_id, message_id, device_id)
                 SELECT ?, ?, device_id FROM resonance_devices
                 WHERE account_id = ? AND revoked_at IS NULL',
            );
            foreach ($deliveryByRecipient as $recipientPublicKey => $envelope) {
                $recipientAccount = null;
                foreach ($currentRows as $member) {
                    if (strtolower($member['public_key']) === $recipientPublicKey) {
                        $recipientAccount = $member['account_id'];
                        break;
                    }
                }
                if ($recipientAccount === null && is_array($addedAccount)
                    && strtolower($addedAccount['public_key']) === $recipientPublicKey
                ) {
                    $recipientAccount = $addedAccount['account_id'];
                }
                if (!is_string($recipientAccount)) {
                    throw new ApiError(400, 'A membership delivery recipient is not in the affected roster.');
                }
                $lockRecipient = $database->prepare(
                    'SELECT account_id FROM resonance_accounts WHERE account_id = ? FOR UPDATE',
                );
                $lockRecipient->execute([$recipientAccount]);
                $expireRecipientDeliveries = $database->prepare(
                    'UPDATE resonance_message_device_deliveries d
                     JOIN resonance_relay_messages m
                       ON m.recipient_id = d.recipient_id AND m.message_id = d.message_id
                     SET d.delivery_state = \'expired\',
                         d.lease_token = NULL, d.lease_expires_at = NULL
                     WHERE d.recipient_id = ? AND d.delivery_state = \'pending\'
                       AND m.expires_at <= UTC_TIMESTAMP(6)',
                );
                $expireRecipientDeliveries->execute([$recipientAccount]);
                $deleteExpiredRecipientMessages = $database->prepare(
                    'DELETE FROM resonance_relay_messages
                     WHERE recipient_id = ? AND expires_at <= UTC_TIMESTAMP(6)',
                );
                $deleteExpiredRecipientMessages->execute([$recipientAccount]);
                $queueSize->execute([$recipientAccount]);
                if ((int) $queueSize->fetchColumn() >= 10000) {
                    throw new ApiError(429, 'A recipient message queue is full. Try again later.');
                }
                $insertMessage->execute([
                    $recipientAccount,
                    $envelope['message_id'],
                    $account['account_id'],
                    json_encode($envelope, JSON_UNESCAPED_SLASHES | JSON_THROW_ON_ERROR),
                ]);
                $insertReceipt->execute([
                    $account['account_id'],
                    $recipientAccount,
                    $envelope['message_id'],
                    (int) $envelope['sent_at'],
                    $groupId,
                ]);
                $insertDeviceDeliveries->execute([
                    $recipientAccount,
                    $envelope['message_id'],
                    $recipientAccount,
                ]);
                refresh_account_delivery_state(
                    $database,
                    $recipientAccount,
                    $envelope['message_id'],
                );
            }

            if (count($addedKeys) === 1 && is_array($addedAccount)) {
                $addMember = $database->prepare(
                    'INSERT INTO resonance_group_members (group_id, account_id) VALUES (?, ?)',
                );
                $addMember->execute([$groupId, $addedAccount['account_id']]);
            } else {
                $removeMember = $database->prepare(
                    'DELETE FROM resonance_group_members
                     WHERE group_id = ? AND account_id = ? AND account_id <> ?',
                );
                $removedAccountId = null;
                foreach ($currentRows as $member) {
                    if (strtolower($member['public_key']) === $removedKeys[0]) {
                        $removedAccountId = $member['account_id'];
                        break;
                    }
                }
                if (!is_string($removedAccountId)) {
                    throw new ApiError(409, 'The member to remove is no longer in the group.');
                }
                $removeMember->execute([$groupId, $removedAccountId, $account['account_id']]);
                $rejectRemovedMemberDeliveries = $database->prepare(
                    'UPDATE resonance_message_device_deliveries d
                     JOIN resonance_relay_messages m
                       ON m.recipient_id = d.recipient_id AND m.message_id = d.message_id
                     SET d.delivery_state = \'rejected\',
                         d.lease_token = NULL, d.lease_expires_at = NULL
                     WHERE d.recipient_id = ?
                       AND d.delivery_state = \'pending\'
                       AND JSON_UNQUOTE(JSON_EXTRACT(m.envelope_json, \'$.group_id\')) = ?',
                );
                $rejectRemovedMemberDeliveries->execute([$removedAccountId, $groupId]);
                $rejectRemovedReceipts = $database->prepare(
                    'UPDATE resonance_message_receipts
                     SET delivery_state = \'rejected\', failure_code = \'member_removed\'
                     WHERE recipient_id = ? AND group_id = ? AND delivered_at IS NULL',
                );
                $rejectRemovedReceipts->execute([$removedAccountId, $groupId]);
                $deleteQueuedGroupMessages = $database->prepare(
                    'DELETE FROM resonance_relay_messages
                     WHERE recipient_id = ?
                       AND JSON_UNQUOTE(JSON_EXTRACT(envelope_json, \'$.group_id\')) = ?',
                );
                $deleteQueuedGroupMessages->execute([$removedAccountId, $groupId]);
            }
            $resultVersion = $expectedVersion + 1;
            $updateVersion = $database->prepare(
                'UPDATE resonance_groups SET membership_version = ?
                 WHERE group_id = ? AND owner_id = ? AND membership_version = ?',
            );
            $updateVersion->execute([
                $resultVersion,
                $groupId,
                $account['account_id'],
                $expectedVersion,
            ]);
            if ($updateVersion->rowCount() !== 1) {
                throw new ApiError(409, 'The group roster changed during the operation.');
            }
            $recordOperation = $database->prepare(
                'INSERT INTO resonance_group_operations
                 (group_id, operation_id, owner_id, request_hash, result_version)
                 VALUES (?, ?, ?, ?, ?)',
            );
            $recordOperation->execute([
                $groupId,
                $operationId,
                $account['account_id'],
                $requestHash,
                $resultVersion,
            ]);
            $database->commit();
        } catch (Throwable $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            throw $error;
        }
        respond(200, ['version' => $resultVersion]);
    }

}
