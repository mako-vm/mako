#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
COMPOSE_DIR="$SCRIPT_DIR/compose-test"

export DOCKER_HOST="unix://$HOME/.mako/docker.sock"

echo "=== Docker Compose Integration Test ==="
echo ""

# Check Mako is running
if ! docker info > /dev/null 2>&1; then
    echo "ERROR: Mako/Docker is not running. Start with: mako start"
    exit 1
fi

echo "1. Starting compose stack..."
docker compose -f "$COMPOSE_DIR/docker-compose.yml" up -d

echo "2. Waiting for services..."
sleep 5

echo "3. Checking containers..."
docker compose -f "$COMPOSE_DIR/docker-compose.yml" ps

echo "4. Testing nginx (port 9080)..."
HTTP_CODE=$(curl -s -o /dev/null -w "%{http_code}" http://localhost:9080 2>&1 || echo "000")
if [ "$HTTP_CODE" = "200" ]; then
    echo "   nginx: OK (HTTP $HTTP_CODE)"
else
    echo "   nginx: FAIL (HTTP $HTTP_CODE)"
fi

echo "5. Testing redis (port 6379)..."
REDIS_PONG=$(echo "PING" | nc -w 2 localhost 6379 2>/dev/null || echo "FAIL")
if echo "$REDIS_PONG" | grep -q "PONG"; then
    echo "   redis: OK (PONG)"
else
    echo "   redis: FAIL ($REDIS_PONG)"
fi

echo "6. Cleaning up..."
docker compose -f "$COMPOSE_DIR/docker-compose.yml" down

echo ""
echo "=== Compose test complete ==="
