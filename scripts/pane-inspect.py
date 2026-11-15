#!/usr/bin/env python3
"""Send one bounded NDJSON request to Pane's private inspection socket."""

import argparse
import base64
import json
from pathlib import Path
import socket
import sys
import time

MAX_REQUEST = 64 * 1024
MAX_RESPONSE = 4 * 1024 * 1024
TIMEOUT = 8.0


def reject_constant(value):
    raise ValueError(f"invalid JSON constant: {value}")


def exchange(directory, request):
    deadline = time.monotonic() + TIMEOUT

    def remaining():
        seconds = deadline - time.monotonic()
        if seconds <= 0:
            raise TimeoutError("inspection request deadline exceeded")
        return seconds

    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
        client.settimeout(remaining())
        client.connect(str(Path(directory) / "control.sock"))
        client.settimeout(remaining())
        client.sendall(request)
        response = bytearray()
        while True:
            client.settimeout(remaining())
            chunk = client.recv(min(65536, MAX_RESPONSE + 1 - len(response)))
            if not chunk:
                raise ValueError("server disconnected before response newline")
            newline = chunk.find(b"\n")
            response.extend(chunk if newline < 0 else chunk[: newline + 1])
            if len(response) > MAX_RESPONSE:
                raise ValueError("response exceeds 4 MiB")
            if newline >= 0:
                remaining()
                return json.loads(response.decode("utf-8"), parse_constant=reject_constant)
            if len(response) == MAX_RESPONSE:
                raise ValueError("response exceeds 4 MiB (including newline)")


def save_image(response, output):
    result = response.get("result")
    image = result.get("image") if isinstance(result, dict) else None
    if not isinstance(image, dict):
        raise ValueError("--output requires a screenshot response with an image")
    if image.get("format") != "png" or image.get("encoding") != "base64":
        raise ValueError("--output requires a base64-encoded PNG image")
    data = image.get("data")
    if not isinstance(data, str):
        raise ValueError("image.data must be a base64 string")
    if len(data) > 4 * ((MAX_RESPONSE + 2) // 3):
        raise ValueError("decoded image exceeds 4 MiB")
    decoded = base64.b64decode(data, validate=True)
    if len(decoded) > MAX_RESPONSE:
        raise ValueError("decoded image exceeds 4 MiB")
    if not decoded.startswith(b"\x89PNG\r\n\x1a\n"):
        raise ValueError("image is missing the PNG signature")

    path = Path(output)
    with path.open("xb") as destination:
        destination.write(decoded)
    # Preserve other response fields without printing the potentially huge image.
    metadata = dict(response)
    metadata["result"] = dict(result)
    metadata["result"]["image"] = {
        key: value for key, value in image.items() if key != "data"
    }
    metadata["output"] = str(path)
    return metadata


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", metavar="DIR", help="private inspection directory")
    parser.add_argument("request", metavar="JSON", help="JSON request (quote it for your shell)")
    parser.add_argument(
        "--output", metavar="FILE", help="save a screenshot PNG to a new file (never overwrite)"
    )
    args = parser.parse_args(argv)
    try:
        value = json.loads(args.request, parse_constant=reject_constant)
        request = (json.dumps(value, separators=(",", ":"), allow_nan=False) + "\n").encode()
        if len(request) > MAX_REQUEST:
            raise ValueError("request exceeds 64 KiB (including newline)")
        response = exchange(args.directory, request)
        if not isinstance(response, dict) or type(response.get("ok")) is not bool:
            raise ValueError("invalid response envelope")
        if response["ok"] and "result" not in response:
            raise ValueError("success response is missing result")
        if not response["ok"] and not isinstance(response.get("error"), str):
            raise ValueError("error response is missing error string")
        if args.output is not None and response["ok"]:
            response = save_image(response, args.output)
        print(json.dumps(response, indent=2, ensure_ascii=True, allow_nan=False))
        return 0 if response["ok"] else 1
    except (OSError, ValueError, RecursionError) as error:
        print(f"pane-inspect: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
