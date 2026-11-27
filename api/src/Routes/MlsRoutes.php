<?php
declare(strict_types=1);

function handle_mls_routes(PDO $database, string $method, string $path): void
{
    if ($method === 'PUT' && $path === '/v1/mls/key-packages') {
        $account = account_for_token($database);
        $body = request_body();
        $packages = $body['key_packages'] ?? null;
        if (!is_array($packages) || count($packages) < 1 || count($packages) > 10) {
            throw new ApiError(400, 'Publish between 1 and 10 MLS key packages.');
        }
        $validated = [];
        $packageNumber = 0;
        foreach ($packages as $package) {
            $packageNumber++;
            if (!is_string($package)) {
                throw new ApiError(400, "MLS key package {$packageNumber} must be a string.");
            }
            $packageLength = strlen($package);
            if (
                $packageLength < 64
                || $packageLength > 4096
                || $packageLength % 2 !== 0
            ) {
                throw new ApiError(
                    400,
                    "MLS key package {$packageNumber} has {$packageLength} hex characters; expected 64-4096.",
                );
            }
            if (preg_match('/\A[a-f0-9]+\z/i', $package) !== 1) {
                throw new ApiError(
                    400,
                    "MLS key package {$packageNumber} contains non-hexadecimal characters.",
                );
            }
            $package = strtolower($package);
            $packageId = hash('sha256', hex2bin($package));
            if (isset($validated[$packageId])) {
                throw new ApiError(400, 'MLS key packages must be unique.');
            }
            $validated[$packageId] = $package;
        }
        $database->beginTransaction();
        try {
            $delete = $database->prepare(
                'DELETE FROM resonance_mls_key_packages WHERE account_id = ?',
            );
            $delete->execute([$account['account_id']]);
            $insert = $database->prepare(
                'INSERT INTO resonance_mls_key_packages
                 (account_id, package_id, key_package, expires_at)
                 VALUES (?, ?, ?, DATE_ADD(UTC_TIMESTAMP(6), INTERVAL 14 DAY))',
            );
            foreach ($validated as $packageId => $package) {
                $insert->execute([$account['account_id'], $packageId, $package]);
            }
            $database->commit();
        } catch (Throwable $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            throw $error;
        }
        respond_empty(204);
    }

    if ($method === 'POST' && $path === '/v1/mls/key-packages/claim') {
        $account = account_for_token($database);
        $body = request_body();
        $publicKeys = $body['account_public_keys'] ?? null;
        if (!is_array($publicKeys) || count($publicKeys) < 1 || count($publicKeys) > 11) {
            throw new ApiError(400, 'Request between 1 and 11 peer MLS key packages.');
        }
        $publicKeys = array_map(
            static fn ($publicKey): string => is_string($publicKey)
                ? validate_public_key($publicKey)
                : throw new ApiError(400, 'A peer identity key is invalid.'),
            $publicKeys,
        );
        if (count(array_unique($publicKeys)) !== count($publicKeys)) {
            throw new ApiError(400, 'A peer MLS key package was requested more than once.');
        }

        $database->beginTransaction();
        try {
            $friendship = $database->prepare(
                'SELECT COUNT(*) FROM resonance_friends
                 WHERE (account_id = ? AND peer_account_id = ?)
                    OR (account_id = ? AND peer_account_id = ?)',
            );
            $lookupAccount = $database->prepare(
                'SELECT account_id, public_key FROM resonance_accounts WHERE public_key = ?',
            );
            $claimPackage = $database->prepare(
                'SELECT package_id, key_package FROM resonance_mls_key_packages
                 WHERE account_id = ? AND expires_at > UTC_TIMESTAMP(6)
                 ORDER BY created_at, package_id LIMIT 1 FOR UPDATE',
            );
            $deletePackage = $database->prepare(
                'DELETE FROM resonance_mls_key_packages
                 WHERE account_id = ? AND package_id = ?',
            );
            $claimed = [];
            foreach ($publicKeys as $publicKey) {
                $lookupAccount->execute([$publicKey]);
                $peer = $lookupAccount->fetch();
                if (!is_array($peer)) {
                    throw new ApiError(404, 'A peer account no longer exists.');
                }
                $friendship->execute([
                    $account['account_id'],
                    $peer['account_id'],
                    $peer['account_id'],
                    $account['account_id'],
                ]);
                if ((int) $friendship->fetchColumn() !== 2) {
                    throw new ApiError(403, 'MLS key packages are available only to mutual friends.');
                }
                $claimPackage->execute([$peer['account_id']]);
                $package = $claimPackage->fetch();
                if (!is_array($package)) {
                    throw new ApiError(409, 'A peer has no available MLS key package.');
                }
                $deletePackage->execute([$peer['account_id'], $package['package_id']]);
                $claimed[] = [
                    'account_public_key' => $peer['public_key'],
                    'key_package' => $package['key_package'],
                ];
            }
            $database->commit();
        } catch (Throwable $error) {
            if ($database->inTransaction()) {
                $database->rollBack();
            }
            throw $error;
        }
        respond(200, ['key_packages' => $claimed]);
    }

}
