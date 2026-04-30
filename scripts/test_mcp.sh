#!/bin/bash
# Test MCP server
ARCHEX=/Users/nihal/project/archex/target/debug/archex
PROJECT=/tmp/nextjs-test

cd $PROJECT

printf '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}\n{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}\n{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_context","arguments":{"file_path":"app/page.tsx"}}}\n' | $ARCHEX serve