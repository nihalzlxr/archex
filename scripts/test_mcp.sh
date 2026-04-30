#!/bin/bash
# Test MCP server
ARCHEX=/Users/nihal/project/archex/target/debug/archex
PROJECT=/Users/nihal/work/lerniq

cd "$PROJECT"

{
    echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}'
    echo '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}'
    echo '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_context","arguments":{"file_path":"src/app/api/auth/route.ts"}}}'
} | $ARCHEX serve