<?php
declare(strict_types=1);

require_once dirname(__DIR__) . '/src/Config.php';
require_once dirname(__DIR__) . '/src/Http.php';
require_once dirname(__DIR__) . '/src/Database.php';
require_once dirname(__DIR__) . '/src/AttachmentStorage.php';
require_once dirname(__DIR__) . '/src/RelaySupport.php';
require_once dirname(__DIR__) . '/src/Validation.php';
require_once dirname(__DIR__) . '/src/Accounts.php';
require_once dirname(__DIR__) . '/src/Friends.php';
require_once dirname(__DIR__) . '/src/Routes/HealthRoutes.php';
require_once dirname(__DIR__) . '/src/Routes/AttachmentRoutes.php';
require_once dirname(__DIR__) . '/src/Routes/AccountRoutes.php';
require_once dirname(__DIR__) . '/src/Routes/GroupRoutes.php';
require_once dirname(__DIR__) . '/src/Routes/MlsRoutes.php';
require_once dirname(__DIR__) . '/src/Routes/EnrollmentRoutes.php';
require_once dirname(__DIR__) . '/src/Routes/DeviceRoutes.php';
require_once dirname(__DIR__) . '/src/Routes/MessageSendRoutes.php';
require_once dirname(__DIR__) . '/src/Routes/ReceiptRoutes.php';
require_once dirname(__DIR__) . '/src/Routes/MessageDeliveryRoutes.php';
require_once dirname(__DIR__) . '/src/Routes/FriendRoutes.php';

function handle_request(string $method, string $path): never
{
    $database = database();
    handle_health_route($database, $method, $path);
    handle_attachment_routes($database, $method, $path);
    handle_account_routes($database, $method, $path);
    handle_group_routes($database, $method, $path);
    handle_mls_routes($database, $method, $path);
    handle_enrollment_routes($database, $method, $path);
    handle_device_routes($database, $method, $path);
    handle_message_send_routes($database, $method, $path);
    handle_receipt_routes($database, $method, $path);
    handle_message_delivery_routes($database, $method, $path);
    handle_friend_routes($database, $method, $path);
    throw new ApiError(404, 'Not found.');
}

try {
    $method = $_SERVER['REQUEST_METHOD'] ?? '';
    $path = $_GET['route'] ?? null;
    if (!is_string($path) || !str_starts_with($path, '/')) {
        throw new ApiError(400, 'A valid route query parameter is required.');
    }
    handle_request($method, $path);
} catch (ApiError $error) {
    respond($error->status, ['error' => $error->getMessage()]);
} catch (Throwable $error) {
    $requestId = bin2hex(random_bytes(8));
    error_log("Resonance account API error [{$requestId}]: " . $error->getMessage());
    respond(500, [
        'error' => 'Account service is temporarily unavailable.',
        'request_id' => $requestId,
    ]);
}
