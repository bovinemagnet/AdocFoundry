#!/usr/bin/env python3
"""Smoke test of release binaries: each one's --version, a PDF export from the CLI, the
language server's and the MCP server's initialise and shutdown, and the app staying up on a
fixture workspace.

usage: packaging/smoke-test.py <directory with the binaries> [--no-app]

Run from the repository root, which holds the fixtures. Exits non-zero on the first failure.
"""

import json
import os
import subprocess
import sys
import tempfile
import time

FIXTURES = os.path.join("tests", "fixtures")
TIMEOUT = 120


def binary(directory, name):
    path = os.path.join(directory, name + (".exe" if os.name == "nt" else ""))
    if not os.path.isfile(path):
        sys.exit(f"missing binary: {path}")
    return path


def check(condition, message):
    if not condition:
        sys.exit(f"FAILED: {message}")
    print(f"ok: {message}")


def versions(directory):
    for name in ["adocfoundry", "adocfoundry-cli", "adoc-ls", "adoc-mcp"]:
        output = subprocess.run(
            [binary(directory, name), "--version"],
            capture_output=True, text=True, timeout=TIMEOUT,
        )
        check(output.returncode == 0 and output.stdout.startswith(name + " "),
              f"{name} --version: {output.stdout.strip()}")


def pdf_export(directory):
    with tempfile.TemporaryDirectory() as out:
        pdf = os.path.join(out, "article.pdf")
        page = os.path.join(FIXTURES, "pdf", "reference", "article.adoc")
        output = subprocess.run(
            [binary(directory, "adocfoundry-cli"), "pdf", page, "--output", pdf],
            capture_output=True, text=True, timeout=TIMEOUT,
        )
        with open(pdf, "rb") if os.path.isfile(pdf) else open(os.devnull, "rb") as file:
            head = file.read(5)
        check(output.returncode == 0 and head == b"%PDF-",
              f"adocfoundry-cli pdf writes a PDF {output.stderr.strip()}")


def lsp_message(payload):
    body = json.dumps(payload).encode()
    return b"Content-Length: %d\r\n\r\n%s" % (len(body), body)


def language_server(directory):
    root = os.path.abspath(os.path.join(FIXTURES, "references"))
    messages = b"".join([
        lsp_message({"jsonrpc": "2.0", "id": 1, "method": "initialize",
                     "params": {"capabilities": {}, "rootUri": "file://" + root.replace("\\", "/")}}),
        lsp_message({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        lsp_message({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}),
        lsp_message({"jsonrpc": "2.0", "method": "exit"}),
    ])
    output = subprocess.run([binary(directory, "adoc-ls"), "--stdio"], input=messages,
                            capture_output=True, timeout=TIMEOUT)
    check(output.returncode == 0 and b'"serverInfo"' in output.stdout
          and b'"adoc.renderPreview"' in output.stdout,
          "adoc-ls initialises and shuts down")


def mcp_server(directory):
    root = os.path.join(FIXTURES, "references")
    messages = "\n".join(json.dumps(message) for message in [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize",
         "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                    "clientInfo": {"name": "smoke-test", "version": "0"}}},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
    ]) + "\n"
    process = subprocess.Popen([binary(directory, "adoc-mcp"), root], stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, text=True)
    process.stdin.write(messages)
    process.stdin.flush()
    replies = [json.loads(process.stdout.readline()) for _ in range(2)]
    process.stdin.close()
    process.wait(timeout=TIMEOUT)
    tools = [tool["name"] for tool in replies[1]["result"]["tools"]]
    check(replies[0]["result"]["serverInfo"]["name"] == "adoc-mcp" and "read_page" in tools,
          "adoc-mcp initialises, lists its tools and shuts down")


def app(directory):
    root = os.path.join(FIXTURES, "antora-single-component")
    process = subprocess.Popen([binary(directory, "adocfoundry"), root])
    time.sleep(15)
    alive = process.poll() is None
    process.kill()
    process.wait()
    check(alive, "adocfoundry stays up on a fixture workspace")


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    directory = sys.argv[1]
    versions(directory)
    pdf_export(directory)
    language_server(directory)
    mcp_server(directory)
    if "--no-app" not in sys.argv:
        app(directory)


if __name__ == "__main__":
    main()
