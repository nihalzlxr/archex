#!/bin/bash
# Test MCP server
ARCHEX=/Users/nihal/project/archex/target/debug/archex
PROJECT=/Users/nihal/work/lerniq

cd "$PROJECT"

# Create temp file with all messages
TEMP=$(mktemp)
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test"}}}' > "$TEMP"
echo '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' >> "$TEMP"
echo '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_context","arguments":{"file_path":"src/app/api/auth/route.ts"}}}' >> "$TEMP"

cat "$TEMP" | $ARCHEX serve
rm "$TEMP"