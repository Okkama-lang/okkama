#!/usr/bin/env python3
"""The text helper of examples/port/text.ok in Python, written from docs/port-protocol.md alone (#1081).

    OKKAMA_TEXT_HELPER=examples/port/text_helper.py okkama run examples/port/text.ok

It prints what the Rust helper (okkama-rs/crates/okkama-port/examples/text_helper.rs) prints. Only the Python
standard library is used, and only JSON: the hello does not offer CBOR.
"""
import json
import struct
import sys
import zlib

PROTOCOL = 1
LIMIT = 16 << 20


def read_frame(stream):
    """The next frame's body, or None when the input ends between frames."""
    head = stream.read(4)
    if not head:
        return None
    if len(head) < 4:
        raise ValueError("the input ended inside a frame's length")
    (size,) = struct.unpack(">I", head)
    if size == 0 or size > LIMIT:
        raise ValueError("a frame of %d bytes" % size)
    body = stream.read(size)
    if len(body) < size:
        raise ValueError("the input ended inside a frame")
    return body


def write_frames(stream, bodies):
    for body in bodies:
        stream.write(struct.pack(">I", len(body)) + body)
    stream.flush()


def message(fields):
    return b"J" + json.dumps(fields, separators=(",", ":")).encode()


def answer(method, payload, data):
    """The payload and attachments of a reply, or raise LookupError for an unknown method."""
    if method == "upper":
        return {"text": payload["text"].upper()}, []
    if method == "words":
        words = payload["text"].split()
        longest = ""
        for word in words:
            if len(word) >= len(longest):
                longest = word
        return {"words": len(words), "longest": longest}, []
    if method == "checksum":
        return {"crc32": zlib.crc32(data), "size": len(data)}, []
    if method == "reverse":
        return None, [data[::-1]]
    raise LookupError(method)


def main():
    source, sink = sys.stdin.buffer, sys.stdout.buffer
    write_frames(sink, [message({"kind": "hello", "protocol": PROTOCOL, "codecs": ["json"]})])
    hello = read_frame(source)
    if hello is None or hello[:1] != b"J" or json.loads(hello[1:]).get("protocol") != PROTOCOL:
        sys.exit("text_helper.py: not a port of protocol %d" % PROTOCOL)
    while True:
        frame = read_frame(source)
        if frame is None:
            return
        if frame[:1] != b"J":
            sys.exit("text_helper.py: only JSON messages are accepted")
        request = json.loads(frame[1:])
        attachments = [read_frame(source)[1:] for _ in range(request.get("attachments", 0))]
        if request.get("kind") != "request":
            continue
        reply = {"kind": "reply", "id": request["id"]}
        extra = []
        try:
            payload, extra = answer(request["method"], request.get("payload"), b"".join(attachments))
            if payload is not None:
                reply["payload"] = payload
            if extra:
                reply["attachments"] = len(extra)
        except LookupError:
            reply = {"kind": "failure", "id": request["id"], "code": "unknown_method",
                     "message": "no method %r" % request["method"]}
        except (KeyError, TypeError, AttributeError) as error:
            reply = {"kind": "failure", "id": request["id"], "code": "invalid_payload", "message": str(error)}
        write_frames(sink, [message(reply)] + [b"B" + data for data in extra])


if __name__ == "__main__":
    main()
