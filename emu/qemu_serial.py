#!/usr/bin/env python3
"""Round 7d: QEMU ESP32-S3 serial, as a person at a terminal would use it.

Connects to QEMU's UART0 (a TCP socket), copies everything to a log file,
and when the firmware prints "MO script-ready" types a button press ("E3")
and, 200 ms later, a packet ("Pscript"). Exits when the firmware prints
`done_marker`, or after `timeout` seconds.

Usage: qemu_serial.py <port> <log> <done_marker> <timeout_s>
"""
import socket, sys, time

port, log, marker, timeout = int(sys.argv[1]), sys.argv[2], sys.argv[3].encode(), float(sys.argv[4])
deadline = time.time() + timeout
s = None
while s is None and time.time() < deadline:
    try:
        s = socket.create_connection(("127.0.0.1", port), timeout=1)
    except OSError:
        time.sleep(0.2)
if s is None:
    sys.exit("could not connect to QEMU's serial port")
s.settimeout(0.05)
buf = b""
script = []  # (time, bytes) still to type
with open(log, "wb") as f:
    while time.time() < deadline:
        try:
            d = s.recv(4096)
            if not d:
                break
            f.write(d)
            f.flush()
            buf += d
        except socket.timeout:
            pass
        if b"MO script-ready" in buf and not script and b"\x00typed" not in buf:
            now = time.time()
            script = [(now + 0.1, b"E3\n"), (now + 0.3, b"Pscript\n")]
            buf += b"\x00typed"
        while script and time.time() >= script[0][0]:
            s.sendall(script.pop(0)[1])
        if marker in buf:
            break
