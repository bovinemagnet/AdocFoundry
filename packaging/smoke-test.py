#!/usr/bin/env python3
"""Smoke test of release binaries: each one's --version, a PDF export from the CLI, the
language server's and the MCP server's initialise and shutdown, and the app staying up on a
fixture workspace.

usage: packaging/smoke-test.py <directory with the binaries> [--no-app] [--expect-signed]

With --expect-signed it also checks the signatures: on macOS every binary passes
`codesign --verify --strict` with the hardened runtime and a Developer ID, and the app bundle
around them passes Gatekeeper (`spctl`); on Windows every executable's Authenticode signature
is valid.

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
    for name in BINARIES:
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


BINARIES = ["adocfoundry", "adocfoundry-cli", "adoc-ls", "adoc-mcp"]


def run_quietly(command):
    """`command`'s exit status and its output and errors together."""
    output = subprocess.run(command, capture_output=True, text=True, timeout=TIMEOUT)
    return output.returncode, output.stdout + output.stderr


def signature_problems(directory, platform=sys.platform, run=run_quietly):
    """What is wrong with the signatures of the binaries in `directory`: empty when every one
    is signed as a release must be. `run` runs a command, giving its status and output."""
    problems = []
    if platform == "darwin":
        for name in BINARIES:
            path = os.path.join(directory, name)
            status, output = run(["codesign", "--verify", "--strict", "--verbose=2", path])
            if status != 0:
                problems.append(f"{name}: codesign --verify failed: {output.strip()}")
                continue
            status, output = run(["codesign", "--display", "--verbose=2", path])
            if "Authority=Developer ID Application" not in output:
                problems.append(f"{name}: not signed with a Developer ID")
            if "runtime" not in output:
                problems.append(f"{name}: no hardened runtime")
        # The binaries are in AdocFoundry.app/Contents/MacOS.
        bundle = os.path.dirname(os.path.dirname(os.path.abspath(directory)))
        status, output = run(["spctl", "--assess", "--type", "execute", "-vv", bundle])
        if status != 0:
            problems.append(f"Gatekeeper rejects {bundle}: {output.strip()}")
    elif platform == "win32":
        for name in BINARIES:
            path = os.path.join(directory, name + ".exe")
            status, output = run(["powershell", "-NoProfile", "-Command",
                                  f"(Get-AuthenticodeSignature -LiteralPath '{path}').Status"])
            if status != 0 or output.strip() != "Valid":
                problems.append(f"{name}.exe: Authenticode signature is {output.strip() or 'missing'}")
    else:
        problems.append(f"no signature check on {platform}")
    return problems


def signatures(directory):
    problems = signature_problems(directory)
    for problem in problems:
        print(f"  {problem}")
    check(not problems, "the binaries are signed")


def main():
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    directory = sys.argv[1]
    if "--expect-signed" in sys.argv:
        signatures(directory)
    versions(directory)
    pdf_export(directory)
    language_server(directory)
    mcp_server(directory)
    if "--no-app" not in sys.argv:
        app(directory)


if __name__ == "__main__":
    main()
