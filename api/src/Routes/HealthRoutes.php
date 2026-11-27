<?php
declare(strict_types=1);

function handle_health_route(PDO $database, string $method, string $path): void
{
    if ($method === 'GET' && $path === '/health') {
        $database->query('SELECT 1')->fetchColumn();
        http_response_code(200);
        header('Content-Type: text/plain; charset=utf-8');
        echo 'ok';
        exit;
    }

}
