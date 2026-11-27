<?php
declare(strict_types=1);

function handle_friend_routes(PDO $database, string $method, string $path): void
{
    if ($method === 'POST' && $path === '/v1/friends') {
        $account = account_for_token($database);
        $username = validate_username(string_field(request_body(), 'username'));
        if ($username === $account['username']) {
            throw new ApiError(400, 'You cannot add yourself as a friend.');
        }
        $peerQuery = $database->prepare('SELECT account_id FROM resonance_accounts WHERE username = ?');
        $peerQuery->execute([$username]);
        $peerAccountId = $peerQuery->fetchColumn();
        if (!is_string($peerAccountId)) {
            throw new ApiError(404, 'No registered account has that username.');
        }
        $database->beginTransaction();
        try {
            $lockAccounts = $database->prepare(
                'SELECT account_id FROM resonance_accounts
                 WHERE account_id IN (?, ?) ORDER BY account_id FOR UPDATE',
            );
            $lockAccounts->execute([$account['account_id'], $peerAccountId]);
            $lockAccounts->fetchAll();

            $friendCheck = $database->prepare(
                'SELECT 1 FROM resonance_friends
                 WHERE account_id = ? AND peer_account_id = ?',
            );
            $friendCheck->execute([$account['account_id'], $peerAccountId]);
            if ($friendCheck->fetchColumn() !== false) {
                throw new ApiError(409, 'You are already friends with this account.');
            }

            $incoming = $database->prepare(
                'SELECT 1 FROM resonance_friend_requests
                 WHERE sender_id = ? AND recipient_id = ? FOR UPDATE',
            );
            $incoming->execute([$peerAccountId, $account['account_id']]);
            if ($incoming->fetchColumn() !== false) {
                $deleteRequests = $database->prepare(
                    'DELETE FROM resonance_friend_requests
                     WHERE (sender_id = ? AND recipient_id = ?)
                        OR (sender_id = ? AND recipient_id = ?)',
                );
                $deleteRequests->execute([
                    $peerAccountId,
                    $account['account_id'],
                    $account['account_id'],
                    $peerAccountId,
                ]);
                $accept = $database->prepare(
                    'INSERT INTO resonance_friends (account_id, peer_account_id)
                     VALUES (?, ?), (?, ?)',
                );
                $accept->execute([
                    $account['account_id'],
                    $peerAccountId,
                    $peerAccountId,
                    $account['account_id'],
                ]);
                $status = 'accepted';
            } else {
                $outgoing = $database->prepare(
                    'SELECT 1 FROM resonance_friend_requests
                     WHERE sender_id = ? AND recipient_id = ?',
                );
                $outgoing->execute([$account['account_id'], $peerAccountId]);
                if ($outgoing->fetchColumn() !== false) {
                    throw new ApiError(409, 'A friend request is already pending.');
                }
                $request = $database->prepare(
                    'INSERT INTO resonance_friend_requests (sender_id, recipient_id) VALUES (?, ?)',
                );
                $request->execute([$account['account_id'], $peerAccountId]);
                $status = 'pending';
            }
            $database->commit();
        } catch (ApiError $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            throw $error;
        } catch (PDOException $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            if ($error->getCode() === '23000') {
                throw new ApiError(409, 'A friend request is already pending.');
            }
            throw $error;
        }
        respond(200, ['status' => $status]);
    }

    if (
        preg_match('/\A\/v1\/friend-requests\/([a-f0-9]{64})\/accept\z/i', $path, $matches) === 1
        && $method === 'POST'
    ) {
        $account = account_for_token($database);
        $publicKey = validate_public_key($matches[1]);
        $peerQuery = $database->prepare(
            'SELECT account_id FROM resonance_accounts WHERE public_key = ?',
        );
        $peerQuery->execute([$publicKey]);
        $peerAccountId = $peerQuery->fetchColumn();
        if (!is_string($peerAccountId)) {
            throw new ApiError(404, 'Friend request not found.');
        }
        $database->beginTransaction();
        try {
            $request = $database->prepare(
                'SELECT 1 FROM resonance_friend_requests
                 WHERE sender_id = ? AND recipient_id = ? FOR UPDATE',
            );
            $request->execute([$peerAccountId, $account['account_id']]);
            if ($request->fetchColumn() === false) {
                throw new ApiError(404, 'Friend request not found.');
            }
            $deleteRequests = $database->prepare(
                'DELETE FROM resonance_friend_requests
                 WHERE (sender_id = ? AND recipient_id = ?)
                    OR (sender_id = ? AND recipient_id = ?)',
            );
            $deleteRequests->execute([
                $peerAccountId,
                $account['account_id'],
                $account['account_id'],
                $peerAccountId,
            ]);
            $accept = $database->prepare(
                'INSERT IGNORE INTO resonance_friends (account_id, peer_account_id)
                 VALUES (?, ?), (?, ?)',
            );
            $accept->execute([
                $account['account_id'],
                $peerAccountId,
                $peerAccountId,
                $account['account_id'],
            ]);
            $database->commit();
        } catch (ApiError $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            throw $error;
        } catch (Throwable $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            throw $error;
        }
        respond_empty(204);
    }

    if (
        preg_match('/\A\/v1\/friend-requests\/([a-f0-9]{64})\z/i', $path, $matches) === 1
        && $method === 'DELETE'
    ) {
        $account = account_for_token($database);
        $publicKey = validate_public_key($matches[1]);
        $deleteRequest = $database->prepare(
            'DELETE r FROM resonance_friend_requests r
             JOIN resonance_accounts a ON (
                 (r.sender_id = ? AND a.account_id = r.recipient_id)
                 OR (r.recipient_id = ? AND a.account_id = r.sender_id)
             )
             WHERE a.public_key = ?',
        );
        $deleteRequest->execute([
            $account['account_id'],
            $account['account_id'],
            $publicKey,
        ]);
        respond_empty(204);
    }

    if (preg_match('/\A\/v1\/friends\/([a-f0-9]{64})\z/i', $path, $matches) === 1 && $method === 'DELETE') {
        $account = account_for_token($database);
        $publicKey = validate_public_key($matches[1]);
        $peerQuery = $database->prepare(
            'SELECT account_id FROM resonance_accounts WHERE public_key = ?',
        );
        $peerQuery->execute([$publicKey]);
        $peerAccountId = $peerQuery->fetchColumn();
        if (is_string($peerAccountId)) {
            $deleteFriend = $database->prepare(
                'DELETE FROM resonance_friends
                 WHERE (account_id = ? AND peer_account_id = ?)
                    OR (account_id = ? AND peer_account_id = ?)',
            );
            $deleteFriend->execute([
                $account['account_id'],
                $peerAccountId,
                $peerAccountId,
                $account['account_id'],
            ]);
            $deleteMessages = $database->prepare(
                'DELETE FROM resonance_relay_messages
                 WHERE (sender_id = ? AND recipient_id = ?)
                    OR (sender_id = ? AND recipient_id = ?)',
            );
            $deleteMessages->execute([
                $account['account_id'],
                $peerAccountId,
                $peerAccountId,
                $account['account_id'],
            ]);
        }
        respond_empty(204);
    }

}
