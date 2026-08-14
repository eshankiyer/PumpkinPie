"""Small Minecraft RCON client used by the differential test runner."""

from __future__ import annotations

import socket
import struct
from dataclasses import dataclass


class RconError(RuntimeError):
    """Raised when the server rejects or truncates an RCON exchange."""


@dataclass(frozen=True)
class Packet:
    request_id: int
    packet_type: int
    payload: str


class Rcon:
    def __init__(self, host: str, port: int, password: str, timeout: float = 5.0):
        self._address = (host, port)
        self._password = password
        self._timeout = timeout
        self._socket: socket.socket | None = None
        self._request_id = 0
        self._buffer = bytearray()

    def __enter__(self) -> "Rcon":
        try:
            self._socket = socket.create_connection(self._address, self._timeout)
            self._socket.settimeout(self._timeout)
            self._request_id = 1
            response = self._send_and_read_one(3, self._password)
            if response.packet_type != 2 or response.request_id != self._request_id:
                raise RconError("RCON authentication failed")
            return self
        except Exception:
            if self._socket is not None:
                self._socket.close()
                self._socket = None
            raise

    def __exit__(self, *_exc: object) -> None:
        if self._socket is not None:
            self._socket.close()
            self._socket = None

    def command(self, command: str) -> str:
        request_id = self._request_id + 1
        end_id = request_id + 1
        self._request_id = end_id
        packets = self._send_and_read_until(request_id, 2, command, end_id)
        responses = [packet for packet in packets if packet.request_id == request_id]
        if any(packet.packet_type != 0 for packet in responses):
            raise RconError("RCON command returned an unexpected packet type")
        return "".join(packet.payload for packet in responses)

    def _send_and_read_one(self, packet_type: int, payload: str) -> Packet:
        self._send_packet(self._request_id, packet_type, payload)
        return self._read_packet()

    def _send_and_read_until(self, request_id: int, packet_type: int, payload: str, end_id: int) -> list[Packet]:
        if self._socket is None:
            raise RconError("RCON connection is not open")
        self._send_packet(request_id, packet_type, payload)
        # Wait for the first response before sending the marker. Vanilla's
        # RCON reader rejects coalesced request frames, while a command may
        # still produce several response packets after its first one.
        first = self._read_packet()
        if first.request_id != request_id or first.packet_type != 0:
            raise RconError("RCON response had an unexpected ID or type")
        packets = [first]
        self._send_packet(end_id, packet_type, "time query gametime")
        # The marker terminates the response. The server may emit multiple
        # packets for the command, including an empty packet for no output.
        while True:
            packet = self._read_packet()
            if packet.request_id not in (request_id, end_id) or packet.packet_type != 0:
                raise RconError("RCON response had an unexpected ID or type")
            packets.append(packet)
            if packet.request_id == end_id:
                break
        return packets

    def _send_packet(self, request_id: int, packet_type: int, payload: str) -> None:
        if self._socket is None:
            raise RconError("RCON connection is not open")
        body = struct.pack("<ii", request_id, packet_type)
        body += payload.encode("utf-8") + b"\x00\x00"
        self._socket.sendall(struct.pack("<i", len(body)) + body)

    def _read_packet(self) -> Packet:
        if self._socket is None:
            raise RconError("RCON connection is not open")
        while len(self._buffer) < 4:
            chunk = self._socket.recv(4096)
            if not chunk:
                raise RconError("RCON connection closed mid-packet")
            self._buffer.extend(chunk)
        (size,) = struct.unpack("<i", self._buffer[:4])
        del self._buffer[:4]
        if size < 10 or size > 10 * 1024 * 1024:
            raise RconError(f"invalid RCON packet size {size}")
        while len(self._buffer) < size:
            chunk = self._socket.recv(4096)
            if not chunk:
                raise RconError("RCON connection closed mid-packet")
            self._buffer.extend(chunk)
        data = bytes(self._buffer[:size])
        del self._buffer[:size]
        request_id, packet_type = struct.unpack("<ii", data[:8])
        if data[-2:] != b"\x00\x00":
            raise RconError("RCON packet is missing its terminators")
        try:
            payload = data[8:-2].decode("utf-8")
        except UnicodeDecodeError as error:
            raise RconError("RCON payload is not valid UTF-8") from error
        return Packet(request_id, packet_type, payload)
